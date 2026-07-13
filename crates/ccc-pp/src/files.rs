use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FileIdentity(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedFile {
    pub path: PathBuf,
    pub source: String,
    pub identity: FileIdentity,
}

/// Supplies include candidates. Search ordering stays inside the preprocessor.
pub trait FileProvider {
    fn read(&self, path: &Path) -> io::Result<LoadedFile>;

    fn exists(&self, path: &Path) -> bool {
        self.read(path).is_ok()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FsFileProvider;

impl FileProvider for FsFileProvider {
    fn read(&self, path: &Path) -> io::Result<LoadedFile> {
        let bytes = fs::read(path)?;
        let source = String::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let identity = file_identity(path, &canonical)?;
        Ok(LoadedFile {
            path: path.to_path_buf(),
            source,
            identity,
        })
    }
}

#[cfg(unix)]
fn file_identity(path: &Path, _canonical: &Path) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path)?;
    Ok(FileIdentity(format!(
        "{}:{}",
        metadata.dev(),
        metadata.ino()
    )))
}

#[cfg(not(unix))]
fn file_identity(_path: &Path, canonical: &Path) -> io::Result<FileIdentity> {
    Ok(FileIdentity(canonical.to_string_lossy().into_owned()))
}

impl fmt::Display for FileIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn preserves_the_requested_candidate_path() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ccc-pp-path-{}-{nonce}", std::process::id()));
        fs::write(&path, "42\n").unwrap();
        let loaded = FsFileProvider.read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(loaded.path, path);
    }
}
