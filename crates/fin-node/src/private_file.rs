//! A new file only this user can read, for a secret written once (5A Task 9, review of Tasks
//! 8–9, M2: `finisherd agent mint --out` and `mcp setup --out`): `createPrivateFile(path)`.
//!
//! - **Created private:** `fin-registry`'s `private::create_new` (spec §5.6): `O_EXCL` and mode
//!   0600 on Unix; on Windows `CREATE_NEW` with a protected owner-only DACL
//!   `O:<user>D:P(A;;FA;;;<user>)(A;;FA;;;SY)` in the same call, so the file is never reachable
//!   through its folder's ACL, not even before the secret is written.
//! - **Checked before anything is written:** on Unix `fstat` of the open file: a regular file
//!   with one link, owned by this process's effective user, no group or other bit. On Windows
//!   its owner and DACL (`private::check`: a FAT or exFAT volume has no DACL and is refused). A
//!   failed check removes the file and refuses (`PRIVATE_FILE_UNSAFE`).
//! - **Written whole:** `commit(text)` writes every byte, `fsync`s and closes; `discard()`
//!   closes and removes the file. Either may be called once; the other is then a no-op.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_registry::private;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::json;

use crate::{guarded, reject};

fn refuse(path: &Path, code: &'static str, why: String, problems: Vec<String>) -> Error {
    reject(
        CoreError::new(code, format!("{}: {why}", path.display()))
            .with_details(json!({ "path": path.display().to_string(), "problems": problems })),
    )
}

/// A file made by [`create_private_file`], open for its one write.
#[napi]
pub struct PrivateFile {
    path: PathBuf,
    file: Option<File>,
}

#[napi]
impl PrivateFile {
    /// The file's path.
    #[napi(getter)]
    pub fn path(&self) -> String {
        self.path.display().to_string()
    }

    /// Writes all of `text`, flushes it to disk and closes the file. The file is removed again
    /// when any step fails.
    #[napi(catch_unwind)]
    pub fn commit(&mut self, text: String) -> Result<()> {
        let Some(mut file) = self.file.take() else {
            return Err(refuse(
                &self.path,
                "INTERNAL",
                "the file was already written or removed".into(),
                vec![],
            ));
        };
        let path = self.path.clone();
        guarded(
            move || {
                let written = file
                    .write_all(text.as_bytes())
                    .and_then(|()| file.sync_all());
                drop(file);
                written.map_err(|e| {
                    let _ = std::fs::remove_file(&path);
                    refuse(&path, "IO_ERROR", format!("cannot write: {e}"), vec![])
                })
            },
            || {},
        )
    }

    /// Closes and removes the file (nothing when it was written already).
    #[napi]
    pub fn discard(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Creates the new file `path`, private to this user from creation, and checks the open file
/// (module docs). An existing path is `ALREADY_EXISTS`; one that is not private is removed and
/// `PRIVATE_FILE_UNSAFE`.
#[napi(catch_unwind)]
pub fn create_private_file(path: String) -> Result<PrivateFile> {
    guarded(
        || {
            let path = PathBuf::from(path);
            let file = private::create_new(&path).map_err(|e| {
                let code = if e.kind() == std::io::ErrorKind::AlreadyExists {
                    "ALREADY_EXISTS"
                } else {
                    "IO_ERROR"
                };
                refuse(&path, code, format!("cannot create: {e}"), vec![])
            })?;
            let problems = match imp::problems(&file, &path) {
                Ok(p) => p,
                Err(e) => vec![format!("cannot inspect: {e}")],
            };
            if !problems.is_empty() {
                drop(file);
                let _ = std::fs::remove_file(&path);
                return Err(refuse(
                    &path,
                    "PRIVATE_FILE_UNSAFE",
                    "another account could read this file".into(),
                    problems,
                ));
            }
            Ok(PrivateFile {
                path,
                file: Some(file),
            })
        },
        || {},
    )
}

#[cfg(unix)]
mod imp {
    use std::fs::File;
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    /// What `fstat` of the open file says is wrong.
    pub fn problems(file: &File, _path: &Path) -> std::io::Result<Vec<String>> {
        let m = file.metadata()?;
        let mut out = Vec::new();
        if !m.file_type().is_file() {
            out.push("not a regular file".to_string());
        }
        if m.nlink() != 1 {
            out.push(format!("links {}", m.nlink()));
        }
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        if m.uid() != me {
            out.push(format!("owner uid {}", m.uid()));
        }
        if m.mode() & 0o077 != 0 {
            out.push(format!("mode {:04o}", m.mode() & 0o7777));
        }
        Ok(out)
    }
}

#[cfg(windows)]
mod imp {
    use std::fs::File;
    use std::path::Path;

    use fin_registry::private::{self, Kind};

    /// What the owner and DACL of the new file say is wrong.
    pub fn problems(_file: &File, path: &Path) -> std::io::Result<Vec<String>> {
        Ok(private::check(path, Kind::File)?
            .map(|u| u.problems)
            .unwrap_or_default())
    }
}
