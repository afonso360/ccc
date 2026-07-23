use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

#[cfg(unix)]
use std::sync::OnceLock;

static NEXT_REGISTRATION: AtomicU64 = AtomicU64::new(0);
static TEMPORARIES: Mutex<BTreeMap<u64, TrackedPath>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy)]
enum TemporaryKind {
    File,
    Directory,
}

struct TrackedPath {
    path: PathBuf,
    kind: TemporaryKind,
}

struct Registration {
    id: Option<u64>,
    path: PathBuf,
    kind: TemporaryKind,
}

impl Registration {
    fn insert(
        path: PathBuf,
        kind: TemporaryKind,
        temporaries: &mut BTreeMap<u64, TrackedPath>,
    ) -> Self {
        let id = NEXT_REGISTRATION.fetch_add(1, Ordering::Relaxed);
        temporaries.insert(
            id,
            TrackedPath {
                path: path.clone(),
                kind,
            },
        );
        Self {
            id: Some(id),
            path,
            kind,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        if let Some(id) = self.id.take() {
            lock_temporaries().remove(&id);
        }
    }

    fn rename_to(&mut self, destination: &Path) -> io::Result<()> {
        let mut temporaries = lock_temporaries();
        fs::rename(&self.path, destination)?;
        if let Some(id) = self.id.take() {
            temporaries.remove(&id);
        }
        Ok(())
    }

    fn replace_directory_with_backup(
        &mut self,
        destination: &Path,
        backup: &mut Self,
    ) -> io::Result<()> {
        let mut temporaries = lock_temporaries();
        fs::remove_dir(&backup.path)?;
        fs::rename(destination, &backup.path)?;
        if let Err(publish_error) = fs::rename(&self.path, destination) {
            if let Err(restore_error) = fs::rename(&backup.path, destination) {
                return Err(io::Error::new(
                    publish_error.kind(),
                    format!(
                        "cannot publish replacement ({publish_error}) or restore prior directory ({restore_error})"
                    ),
                ));
            }
            return Err(publish_error);
        }
        if let Some(id) = self.id.take() {
            temporaries.remove(&id);
        }
        Ok(())
    }

    fn set_kind(&mut self, kind: TemporaryKind, temporaries: &mut BTreeMap<u64, TrackedPath>) {
        self.kind = kind;
        if let Some(id) = self.id
            && let Some(tracked) = temporaries.get_mut(&id)
        {
            tracked.kind = kind;
        }
    }

    fn cleanup(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };
        let mut temporaries = lock_temporaries();
        cleanup_path(&self.path, self.kind);
        temporaries.remove(&id);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// A collision-resistant temporary file tracked until it is removed or
/// deliberately promoted to its final destination.
///
/// This is public only so the command-line driver and link packager can share
/// one process-wide signal handler and cleanup registry.
#[doc(hidden)]
pub struct RegisteredTemporaryFile {
    registration: Registration,
}

impl RegisteredTemporaryFile {
    /// Creates and registers `path` atomically with respect to signal cleanup.
    pub fn create(path: PathBuf) -> io::Result<(File, Self)> {
        ensure_signal_cleanup()?;
        let mut temporaries = lock_temporaries();
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let registration = Registration::insert(path, TemporaryKind::File, &mut temporaries);
        drop(temporaries);
        Ok((file, Self { registration }))
    }

    /// Returns the path owned by this registration.
    pub fn path(&self) -> &Path {
        self.registration.path()
    }

    /// Replaces the registered placeholder with an empty directory while
    /// retaining the same drop and signal-cleanup ownership.
    pub fn replace_with_directory(&mut self) -> io::Result<()> {
        let mut temporaries = lock_temporaries();
        fs::remove_file(self.registration.path())?;
        fs::create_dir(self.registration.path())?;
        self.registration
            .set_kind(TemporaryKind::Directory, &mut temporaries);
        Ok(())
    }

    /// Promotes the tracked path while holding the cleanup-registry lock across
    /// both the rename and removal of its registration.
    pub fn rename_to(&mut self, destination: &Path) -> io::Result<()> {
        self.registration.rename_to(destination)
    }

    /// Replaces an existing directory while the cleanup-registry lock covers
    /// displacement, publication, and registration removal.
    pub fn replace_directory_with_backup(
        &mut self,
        destination: &Path,
        backup: &mut Self,
    ) -> io::Result<()> {
        self.registration
            .replace_directory_with_backup(destination, &mut backup.registration)
    }

    /// Stops tracking a file after it has been atomically promoted.
    pub fn disarm(&mut self) {
        self.registration.disarm();
    }
}

pub(crate) struct RegisteredTemporaryDirectory {
    registration: Registration,
}

impl RegisteredTemporaryDirectory {
    pub(crate) fn create(path: PathBuf) -> io::Result<Self> {
        ensure_signal_cleanup()?;
        let mut temporaries = lock_temporaries();
        fs::create_dir(&path)?;
        let registration = Registration::insert(path, TemporaryKind::Directory, &mut temporaries);
        drop(temporaries);
        Ok(Self { registration })
    }

    pub(crate) fn path(&self) -> &Path {
        self.registration.path()
    }

    pub(crate) fn cleanup(&mut self) {
        self.registration.cleanup();
    }
}

fn cleanup_path(path: &Path, kind: TemporaryKind) {
    match kind {
        TemporaryKind::File => {
            let _ = fs::remove_file(path);
        }
        TemporaryKind::Directory => {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn lock_temporaries() -> MutexGuard<'static, BTreeMap<u64, TrackedPath>> {
    TEMPORARIES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(unix)]
fn ensure_signal_cleanup() -> io::Result<()> {
    static INSTALLATION: OnceLock<Result<(), String>> = OnceLock::new();
    match INSTALLATION.get_or_init(install_signal_cleanup) {
        Ok(()) => Ok(()),
        Err(message) => Err(io::Error::other(message.clone())),
    }
}

#[cfg(unix)]
fn install_signal_cleanup() -> Result<(), String> {
    use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGHUP, SIGINT, SIGQUIT, SIGTERM])
        .map_err(|error| format!("cannot install temporary-path signal cleanup: {error}"))?;
    std::thread::Builder::new()
        .name("ccc-temporary-cleanup".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                {
                    let mut temporaries = lock_temporaries();
                    for tracked in temporaries.values() {
                        cleanup_path(&tracked.path, tracked.kind);
                    }
                    temporaries.clear();
                }

                if signal_hook::low_level::emulate_default_handler(signal).is_err() {
                    signal_hook::low_level::exit(128 + signal);
                }
            }
        })
        .map_err(|error| format!("cannot start temporary-path signal cleanup: {error}"))?;
    Ok(())
}

#[cfg(not(unix))]
fn ensure_signal_cleanup() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "ccc-{name}-{}-{}",
            std::process::id(),
            NEXT_REGISTRATION.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn file_drop_and_disarm_have_opposite_ownership() {
        let removed = path("registered-file");
        let (file, temporary) = RegisteredTemporaryFile::create(removed.clone()).unwrap();
        drop(file);
        drop(temporary);
        assert!(!removed.exists());

        let preserved = path("disarmed-file");
        let (file, mut temporary) = RegisteredTemporaryFile::create(preserved.clone()).unwrap();
        drop(file);
        temporary.disarm();
        drop(temporary);
        assert!(preserved.exists());
        fs::remove_file(preserved).unwrap();
    }

    #[test]
    fn registered_file_can_become_a_tracked_directory() {
        let path = path("registered-converted-directory");
        let (file, mut temporary) = RegisteredTemporaryFile::create(path.clone()).unwrap();
        drop(file);
        temporary.replace_with_directory().unwrap();
        fs::write(path.join("nested"), b"temporary").unwrap();
        drop(temporary);
        assert!(!path.exists());
    }

    #[test]
    fn promotion_removes_registration_before_cleanup_can_observe_destination() {
        let source = path("registered-promotion-source");
        let destination = path("registered-promotion-destination");
        let (file, mut temporary) = RegisteredTemporaryFile::create(source.clone()).unwrap();
        drop(file);
        let registration = temporary.registration.id.unwrap();
        temporary.rename_to(&destination).unwrap();

        assert!(!source.exists());
        assert!(!lock_temporaries().contains_key(&registration));
        assert!(destination.exists());
        fs::remove_file(destination).unwrap();
    }

    #[test]
    fn directory_replacement_keeps_only_the_displaced_tree_registered() {
        let destination = path("registered-replacement-destination");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("old"), b"old").unwrap();

        let replacement_path = path("registered-replacement-new");
        let (file, mut replacement) =
            RegisteredTemporaryFile::create(replacement_path.clone()).unwrap();
        drop(file);
        replacement.replace_with_directory().unwrap();
        fs::write(replacement_path.join("new"), b"new").unwrap();

        let backup_path = path("registered-replacement-backup");
        let (file, mut backup) = RegisteredTemporaryFile::create(backup_path.clone()).unwrap();
        drop(file);
        backup.replace_with_directory().unwrap();
        let replacement_registration = replacement.registration.id.unwrap();
        let backup_registration = backup.registration.id.unwrap();

        replacement
            .replace_directory_with_backup(&destination, &mut backup)
            .unwrap();

        let temporaries = lock_temporaries();
        assert!(!temporaries.contains_key(&replacement_registration));
        assert!(temporaries.contains_key(&backup_registration));
        drop(temporaries);
        assert_eq!(fs::read(destination.join("new")).unwrap(), b"new");
        assert_eq!(fs::read(backup_path.join("old")).unwrap(), b"old");

        drop(replacement);
        drop(backup);
        assert!(destination.exists());
        assert!(!backup_path.exists());
        fs::remove_dir_all(destination).unwrap();
    }

    #[test]
    fn directory_drop_removes_its_complete_workspace() {
        let path = path("registered-directory");
        let temporary = RegisteredTemporaryDirectory::create(path.clone()).unwrap();
        fs::write(path.join("nested"), b"temporary").unwrap();
        drop(temporary);
        assert!(!path.exists());
    }
}
