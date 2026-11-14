//! `asset.probe` (and, in sub-project 6, `asset.add`) of a catalogued library file (plan 5D
//! Task 13, Decisions 20 and 47): the file is opened once, through [`open_for_read`] with the
//! catalogued raw path and `expect`, and `fin_probe::probe_with` reads the fingerprint and the
//! probe from that handle. `probe(path)` is never used for a library file: it would open by
//! name, follow links, and read a placeholder's data into place.
//!
//! The opener's refusals cannot travel through `probe_with`'s `io::Result` without losing
//! their kind, so they are kept out of band and answered as their own codes:
//! `FILE_CLOUD_ONLY`, `FILE_CHANGED`, `FILE_NOT_FOUND` and `PATH_NOT_ALLOWED` (a link or a
//! path leaving the root). An I/O failure is `probe_with`'s own (`ASSET_UNREADABLE`).

use std::cell::Cell;
use std::io;

use fin_core::CoreError;
use fin_probe::Probe;
use serde_json::json;

use crate::probe::FileRef;
use crate::types::OpenError;

/// The error `asset.probe` answers for an opener refusal; `None` for an I/O failure (the
/// caller's probe error then names it).
pub fn refusal(e: &OpenError) -> Option<CoreError> {
    Some(match e {
        OpenError::CloudOnly => CoreError::new(
            "FILE_CLOUD_ONLY",
            "the file is a cloud placeholder whose data is not on this machine",
        )
        .with_fix("make the file available offline in its sync client, then try again"),
        OpenError::Changed => CoreError::new(
            "FILE_CHANGED",
            "the file changed since the library catalogued it",
        )
        .with_fix("scan the library's folder, then try again"),
        OpenError::NotFound => CoreError::new("FILE_NOT_FOUND", "the file no longer exists")
            .with_details(json!({ "reason": "gone" })),
        OpenError::Escapes | OpenError::NameSurrogate => CoreError::new(
            "PATH_NOT_ALLOWED",
            "the path leaves the library or goes through a link",
        ),
        OpenError::Io(_) => return None,
    })
}

/// The probe of `file`, opened only through [`FileRef::open`] (`open_for_read`).
pub fn probe_asset(file: &FileRef<'_>) -> Result<Probe, CoreError> {
    let refused: Cell<Option<CoreError>> = Cell::new(None);
    let probed = fin_probe::probe_with(file.display, |_| match file.open() {
        Ok(f) => Ok(f),
        Err(OpenError::Io(e)) => Err(e),
        Err(other) => {
            refused.set(refusal(&other));
            Err(io::Error::other("the opener refused the file"))
        }
    });
    match refused.take() {
        Some(e) => Err(e),
        None => probed,
    }
}
