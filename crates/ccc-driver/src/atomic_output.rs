use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static OUTPUT_ID: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` only after all bytes have been written successfully.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut pending = PendingOutput::create(path)?;
    pending.write_all(contents)?;
    pending.commit(path)
}

/// A same-directory output that replaces its destination only on commit.
pub(crate) struct PendingOutput {
    path: PathBuf,
    file: Option<File>,
    committed: bool,
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
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        committed: false,
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
        Ok(&self.path)
    }

    pub(crate) fn commit(mut self, destination: &Path) -> io::Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        } else {
            File::open(&self.path)?.sync_all()?;
        }
        fs::rename(&self.path, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PendingOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
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
}
