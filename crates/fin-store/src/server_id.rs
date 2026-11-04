//! This server's identity (plan Decision 5): a lowercase ULID in `<server-data>/server-id`,
//! created on first use. Segment headers carry it as `writerId`; the lease (Task 9) names its
//! probe file after it.
//!
//! The file is written to a temp file (mode 0600 on unix), fsynced, then hard-linked into
//! place, which never replaces: when two processes race on first use, one link wins and both
//! read the winner's id.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use crate::{StoreError, fsio, names};

/// The file under the server data dir.
pub const SERVER_ID_FILE: &str = "server-id";

/// Reads `<server_data>/server-id`, creating it (and `server_data`) on first use.
/// A file that does not hold a ULID is `IO_ERROR` (`InvalidData`): replacing it would give the
/// server a new identity behind the user's back.
pub fn server_id(server_data: &Path) -> Result<String, StoreError> {
    let path = server_data.join(SERVER_ID_FILE);
    let io = StoreError::io(&path);
    match read(&path) {
        Ok(id) => return Ok(id),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io(e)),
    }
    fs::create_dir_all(server_data).map_err(StoreError::io(server_data))?;
    let id = ulid::Ulid::new().to_string().to_ascii_lowercase();
    match create(server_data, &path, &id) {
        Ok(()) => Ok(id),
        // Another process created it first: its id wins.
        Err(e) if e.kind() == ErrorKind::AlreadyExists => read(&path).map_err(io),
        Err(e) => Err(io(e)),
    }
}

fn read(path: &Path) -> io::Result<String> {
    let text = fs::read_to_string(path)?;
    let id = text.trim();
    if id.len() == 26 && ulid::Ulid::from_string(id).is_ok() {
        Ok(id.to_ascii_lowercase())
    } else {
        Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{} does not hold a ULID", path.display()),
        ))
    }
}

fn create(dir: &Path, path: &Path, id: &str) -> io::Result<()> {
    let tmp = dir.join(names::temp_name());
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tmp)?;
        file.write_all(format!("{id}\n").as_bytes())?;
        fsio::sync_file(&file)?;
        drop(file);
        fs::hard_link(&tmp, path)?;
        fsio::sync_dir(dir)
    })();
    let _ = fs::remove_file(&tmp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_id_is_stable() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        let a = server_id(&data).unwrap();
        assert_eq!(a.len(), 26);
        assert_eq!(a, a.to_ascii_lowercase());
        assert_eq!(server_id(&data).unwrap(), a);
        let leftovers: Vec<_> = fs::read_dir(&data).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp file is left");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(data.join(SERVER_ID_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn damaged_server_id_is_an_error_not_a_new_identity() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(SERVER_ID_FILE), "not a ulid").unwrap();
        assert_eq!(server_id(tmp.path()).unwrap_err().code(), "IO_ERROR");
    }
}
