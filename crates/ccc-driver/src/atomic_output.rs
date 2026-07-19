use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use ccc_link::RegisteredTemporaryFile;

static OUTPUT_ID: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` only after all bytes have been written successfully.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut pending = PendingOutput::create(path)?;
    pending.write_all(contents)?;
    pending.commit(path)
}

/// A same-directory output that replaces its destination only on commit.
pub(crate) struct PendingOutput {
    temporary: RegisteredTemporaryFile,
    file: Option<File>,
}

/// A same-directory tree that is invisible at its destination until commit.
pub(crate) struct PendingDirectory {
    temporary: RegisteredTemporaryFile,
}

impl PendingOutput {
    pub(crate) fn create(destination: &Path) -> io::Result<Self> {
        let directory = destination.parent().unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output");

        for _ in 0..100 {
            let id = OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".{file_name}.ccc-{}-{id}.tmp", std::process::id()));
            match RegisteredTemporaryFile::create(path) {
                Ok((file, temporary)) => {
                    return Ok(Self {
                        temporary,
                        file: Some(file),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a collision-free temporary output",
        ))
    }

    fn write_all(&mut self, contents: &[u8]) -> io::Result<()> {
        self.file
            .as_mut()
            .expect("pending output is open for driver writes")
            .write_all(contents)
    }

    /// Closes the collision-resistant placeholder so an external tool can
    /// replace its contents while the destination remains untouched.
    pub(crate) fn prepare_external_write(&mut self) -> io::Result<&Path> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        Ok(self.temporary.path())
    }

    pub(crate) fn commit(mut self, destination: &Path) -> io::Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        } else {
            File::open(self.temporary.path())?.sync_all()?;
        }
        self.temporary.rename_to(destination)
    }
}

impl PendingDirectory {
    pub(crate) fn create(destination: &Path) -> io::Result<Self> {
        let directory = destination.parent().unwrap_or_else(|| Path::new("."));
        let file_name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output");

        for _ in 0..100 {
            let id = OUTPUT_ID.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".{file_name}.ccc-{}-{id}.tmp", std::process::id()));
            match RegisteredTemporaryFile::create(path) {
                Ok((file, mut temporary)) => {
                    drop(file);
                    temporary.replace_with_directory()?;
                    return Ok(Self { temporary });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a collision-free temporary directory",
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        self.temporary.path()
    }

    pub(crate) fn commit(mut self, destination: &Path) -> io::Result<()> {
        if let Ok(metadata) = fs::symlink_metadata(destination) {
            if metadata.is_dir() {
                let mut previous = Self::create(destination)?;
                self.temporary
                    .replace_directory_with_backup(destination, &mut previous.temporary)?;
                drop(previous);
                return Ok(());
            } else {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "directory output destination exists and is not a directory",
                ));
            }
        }
        self.temporary.rename_to(destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_an_output_after_the_complete_write() {
        let directory = std::env::temp_dir().join(format!(
            "ccc-atomic-output-{}-{}",
            std::process::id(),
            OUTPUT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("result.d");
        fs::write(&output, "old").unwrap();

        write_atomic(&output, b"new contents").unwrap();

        assert_eq!(fs::read_to_string(&output).unwrap(), "new contents");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dropping_an_external_output_preserves_the_destination() {
        let directory = std::env::temp_dir().join(format!(
            "ccc-atomic-external-{}-{}",
            std::process::id(),
            OUTPUT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("program");
        fs::write(&output, "old executable").unwrap();

        let mut pending = PendingOutput::create(&output).unwrap();
        let temporary = pending.prepare_external_write().unwrap().to_path_buf();
        fs::write(&temporary, "broken executable").unwrap();
        drop(pending);

        assert_eq!(fs::read_to_string(&output).unwrap(), "old executable");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn directory_output_is_published_only_on_commit() {
        let directory = std::env::temp_dir().join(format!(
            "ccc-atomic-directory-{}-{}",
            std::process::id(),
            OUTPUT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("program.dSYM");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("old"), "old").unwrap();

        let pending = PendingDirectory::create(&output).unwrap();
        fs::write(pending.path().join("new"), "new").unwrap();
        assert!(output.join("old").is_file());
        pending.commit(&output).unwrap();

        assert!(!output.join("old").exists());
        assert_eq!(fs::read_to_string(output.join("new")).unwrap(), "new");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dropping_a_pending_directory_preserves_the_destination() {
        let directory = std::env::temp_dir().join(format!(
            "ccc-atomic-directory-drop-{}-{}",
            std::process::id(),
            OUTPUT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let output = directory.join("program.dSYM");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("old"), "old").unwrap();

        let pending = PendingDirectory::create(&output).unwrap();
        fs::write(pending.path().join("incomplete"), "incomplete").unwrap();
        drop(pending);

        assert_eq!(fs::read_to_string(output.join("old")).unwrap(), "old");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
