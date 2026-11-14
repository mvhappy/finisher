//! Archive sources (4C Decisions 30 and 32): a verified zip (its own `sha256` and `size`)
//! holding catalog files as members. A member is extracted to a `.~fin-<id>.part` temp,
//! never past its catalog size, and placed only if it hashes to its catalog SHA-256.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use crate::cache::FontCache;
use crate::http::Failure;

fn archive_failure(message: String) -> Failure {
    Failure::new("archive", message, false)
}

/// Extracts `member` (`size` bytes, SHA-256 `sha256`) from the zip at `zip` into the cache.
pub(crate) fn extract(
    cache: &FontCache,
    zip: &Path,
    member: &str,
    sha256: &str,
    size: u64,
) -> Result<(), Failure> {
    let file = File::open(zip).map_err(|e| Failure::new("io", e.to_string(), false))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| archive_failure(format!("{}: not a zip: {e}", zip.display())))?;
    let mut entry = archive
        .by_name(member)
        .map_err(|e| archive_failure(format!("{member}: {e}")))?;
    if entry.size() != size {
        return Err(Failure::new(
            "sizeMismatch",
            format!("{member} is {} bytes, not {size}", entry.size()),
            false,
        ));
    }
    let temp = cache.temp_path();
    let copied = (|| -> io::Result<u64> {
        let mut out = File::create(&temp)?;
        let n = io::copy(&mut (&mut entry).take(size + 1), &mut out)?;
        out.flush()?;
        out.sync_all()?;
        Ok(n)
    })();
    match copied {
        Ok(n) if n == size => {}
        Ok(n) => {
            let _ = fs::remove_file(&temp);
            return Err(Failure::new(
                "sizeMismatch",
                format!("{member} inflated to {n} bytes, not {size}"),
                false,
            ));
        }
        Err(e) => {
            let _ = fs::remove_file(&temp);
            return Err(archive_failure(format!("{member}: {e}")));
        }
    }
    cache.place(sha256, &temp).map_err(|e| {
        if e.kind() == io::ErrorKind::InvalidData {
            Failure::new(
                "hashMismatch",
                format!("{member} does not have SHA-256 {sha256}"),
                false,
            )
        } else {
            Failure::new("io", e.to_string(), false)
        }
    })
}
