//! Atomic replacement for small local settings/history files.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

/// Finish and sync a temporary file beside the destination before replacing it.
/// A failed write never truncates the previous version of the user's data.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing filename"))?;
    let temporary = parent.join(format!(".{}-{}.tmp", name.to_string_lossy(), uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        // The replacement succeeded. A directory sync failure must not report
        // a failed save after the new contents are already visible.
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            log::warn!("Could not sync data directory after atomic save: {}", error);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_complete_file_without_leaving_temporary_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"previous complete settings").unwrap();
        write_atomic(&path, b"new complete settings").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new complete settings");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn failed_replacement_preserves_destination_and_removes_its_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("occupied");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("original"), b"keep me").unwrap();
        assert!(write_atomic(&path, b"new data").is_err());
        assert_eq!(fs::read(path.join("original")).unwrap(), b"keep me");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
