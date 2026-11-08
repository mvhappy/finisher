//! The on-demand whole-file BLAKE3 (plan 5D Decision 38): opened through [`open_for_read`] and
//! read through the library's [`Throttle`]. It runs only on request (`library.lookup` with
//! `hashCandidates`, `Libraries.hash`), never in a sweep.

use std::io::Read;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::budget::Throttle;
use crate::probe::{CANCELLED, CHANGED, FileRef, PRESENT, io_refusal, refusal};

const CHUNK: usize = 256 * 1024;

/// What the `library.hash` kind reports for one file; `hash` is `blake3:<hex>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HashOutcome {
    pub file: i64,
    pub hash: Option<String>,
    /// As [`crate::probe::ProbeOutcome::state`]: `present`, or why there is no hash.
    pub state: String,
    pub code: Option<String>,
}

impl HashOutcome {
    fn failed(file: i64, state: &str, code: &str) -> Self {
        HashOutcome {
            file,
            hash: None,
            state: state.into(),
            code: Some(code.into()),
        }
    }
}

/// The BLAKE3 of `f`'s every byte. A file that is not `expect`'s size when read to its end
/// changed while it was read: `changed`, no hash.
pub fn hash_file(f: &FileRef<'_>, throttle: &Throttle) -> HashOutcome {
    let file = match f.open() {
        Ok(file) => file,
        Err(e) => {
            let (state, code) = refusal(&e);
            return HashOutcome::failed(f.file, state, code);
        }
    };
    let mut reader = throttle.reader(file);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buf[..n]);
                total += n as u64;
            }
            Err(_) if throttle.is_cancelled() => {
                return HashOutcome::failed(f.file, CANCELLED, "CANCELLED");
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => {
                let (state, code) = io_refusal(&e);
                return HashOutcome::failed(f.file, state, code);
            }
        }
    }
    if total != f.expect.size {
        return HashOutcome::failed(f.file, CHANGED, "FILE_CHANGED");
    }
    HashOutcome {
        file: f.file,
        hash: Some(format!("blake3:{}", hasher.finalize().to_hex())),
        state: PRESENT.into(),
        code: None,
    }
}
