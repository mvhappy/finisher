//! This server's identity (plan Decision 5): a lowercase ULID in `<server-data>/server-id`,
//! created on first use. Segment headers carry it as `writerId`; the lease (Task 9) names its
//! probe file after it.
//!
//! The file is written to a temp file (mode 0600 on unix), fsynced, then moved into place by
//! `fsio`'s no-replace rename (a hard link where supported, else the kernel's no-replace
//! rename): when two processes race on first use, one wins and both read the winner's id.
//!
//! # Clones ([`server_identity`])
//!
//! A copy of the server data (`cp -a`, a backup restored beside the original, a cloned VM)
//! must not be the same server: it would take the original's lease as its own through the
//! restart shortcut, and the two would write in turns. So `server-id.origin` records, beside
//! the id, the identity of the server-data directory (`identity::dir_identity`) and the OS
//! machine id (`identity::machine_id`). When either differs from what the files say, this is
//! a clone: a **new** id is minted (and the reason reported), and the clone follows like any
//! other server. A server data moved to another filesystem gets a new id too, harmlessly.
//! Without a readable machine id or directory identity a clone cannot be told apart, so the
//! restart shortcut is off ([`ServerIdentity::shortcut_ok`]). The check runs under an OS lock
//! on `server-id.lock`, so racing processes of one server data mint at most once.

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
    server_identity(server_data).map(|i| i.id)
}

/// This server's id, renewed when the server data is a clone (module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerIdentity {
    pub id: String,
    /// Why the id was just renewed (the server data is a copy, or moved machines).
    pub renewed: Option<String>,
    /// The same as a closed word (`otherMachine` or `otherDirectory`), for audit rows.
    pub renewed_reason: Option<&'static str>,
    /// A clone would have been detected: the directory identity and machine id are known.
    pub shortcut_ok: bool,
}

const ORIGIN_FILE: &str = "server-id.origin";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Origin {
    server_id: String,
    dir: Option<String>,
    machine: Option<String>,
}

/// [`server_id`], checking that the server data is the one the id was made in (module docs).
pub fn server_identity(server_data: &Path) -> Result<ServerIdentity, StoreError> {
    fs::create_dir_all(server_data).map_err(StoreError::io(server_data))?;
    let lock_path = server_data.join("server-id.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(StoreError::io(&lock_path))?;
    let _ = lock.lock(); // best effort: without it, only racing first uses could both mint
    let mut id = read_or_create(server_data)?;
    let dir = crate::identity::dir_identity(server_data);
    let machine = crate::identity::machine_id();
    let origin_path = server_data.join(ORIGIN_FILE);
    let origin: Option<Origin> = fs::read(&origin_path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let mut renewed = None;
    let mut renewed_reason = None;
    if let Some(o) = &origin
        && o.server_id == id
    {
        let moved_dir = o.dir.is_some() && dir.is_some() && o.dir != dir;
        let other_machine = o.machine.is_some() && machine.is_some() && o.machine != machine;
        if moved_dir || other_machine {
            renewed_reason = Some(if other_machine {
                "otherMachine"
            } else {
                "otherDirectory"
            });
            renewed = Some(if other_machine {
                "the server data was made on another machine (a copy or a cloned machine): this is a new server".to_string()
            } else {
                "the server data directory is not the one this server id was made in (a copy, or moved to another disk): this is a new server".to_string()
            });
            id = ulid::Ulid::new().to_string().to_ascii_lowercase();
            let path = server_data.join(SERVER_ID_FILE);
            crate::paths::write_atomic(&path, format!("{id}\n").as_bytes())
                .map_err(StoreError::io(&path))?;
        }
    }
    let now = Origin {
        server_id: id.clone(),
        dir: dir.clone(),
        machine: machine.clone(),
    };
    if origin.as_ref() != Some(&now) {
        let bytes = serde_json::to_vec(&now)?;
        crate::paths::write_atomic(&origin_path, &bytes).map_err(StoreError::io(&origin_path))?;
    }
    Ok(ServerIdentity {
        id,
        renewed,
        renewed_reason,
        shortcut_ok: dir.is_some() && machine.is_some(),
    })
}

fn read_or_create(server_data: &Path) -> Result<String, StoreError> {
    let path = server_data.join(SERVER_ID_FILE);
    let io = StoreError::io(&path);
    match read(&path) {
        Ok(id) => return Ok(id),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(io(e)),
    }
    fs::create_dir_all(server_data).map_err(StoreError::io(server_data))?;
    let id = ulid::Ulid::new().to_string().to_ascii_lowercase();
    let _ = fs::create_dir_all(server_data);
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
        fsio::rename_noreplace(&tmp, path)?;
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
        let leftovers: Vec<_> = fs::read_dir(&data)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with(".~fin-"))
            .collect();
        assert!(leftovers.is_empty(), "no temp file is left: {leftovers:?}");
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

#[cfg(test)]
mod clone_tests {
    use super::*;
    use crate::identity::set_machine_id_for_test;

    #[test]
    fn another_machine_or_directory_mints_a_new_id() {
        let tmp = tempfile::tempdir().unwrap();
        let data = tmp.path().join("data");
        set_machine_id_for_test(Some("machine-1"));
        let first = server_identity(&data).unwrap();
        assert!(first.renewed.is_none() && first.shortcut_ok);
        assert_eq!(server_identity(&data).unwrap(), first, "stable");
        // The same files on a cloned machine.
        set_machine_id_for_test(Some("machine-2"));
        let clone = server_identity(&data).unwrap();
        assert_ne!(clone.id, first.id);
        assert!(clone.renewed.unwrap().contains("another machine"));
        assert_eq!(server_identity(&data).unwrap().renewed, None, "once");
        // A copy of the directory on the same machine.
        let copy = tmp.path().join("copy");
        fs::create_dir(&copy).unwrap();
        for f in [SERVER_ID_FILE, ORIGIN_FILE] {
            fs::copy(data.join(f), copy.join(f)).unwrap();
        }
        let c = server_identity(&copy).unwrap();
        assert_ne!(c.id, server_id(&data).unwrap());
        assert!(c.renewed.is_some());
        // Unknown machine id: nothing to compare, so no restart shortcut.
        set_machine_id_for_test(None);
        assert!(!server_identity(&data).unwrap().shortcut_ok);
    }

    #[test]
    fn a_server_data_from_before_the_origin_file_keeps_its_id() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join(SERVER_ID_FILE),
            "01m6kvqb80p491nh8nfcg5bx1s\n",
        )
        .unwrap();
        let i = server_identity(tmp.path()).unwrap();
        assert_eq!(
            (i.id.as_str(), i.renewed),
            ("01m6kvqb80p491nh8nfcg5bx1s", None)
        );
    }
}
