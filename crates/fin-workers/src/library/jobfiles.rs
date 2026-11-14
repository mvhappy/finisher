//! The job's own files: the granted `in/*.ndjson` inputs. These are paths the host handed
//! this process (never library paths), so they are the one place the family opens a file
//! by path; `scripts/clippy-allow.json` lists the module.
#![allow(clippy::disallowed_methods)]

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use fin_jobs::JobError;
use fin_library::KnownDirs;
use fin_library::params::FileLine;

fn input_invalid(what: &str, e: impl std::fmt::Display) -> JobError {
    JobError::new("JOB_INPUT_INVALID", format!("{what}: {e}"))
}

/// `in/dirs.ndjson`.
pub fn known_dirs(path: &Path, generation: u64) -> Result<KnownDirs, JobError> {
    let f = File::open(path).map_err(|e| input_invalid("in/dirs.ndjson", e))?;
    KnownDirs::from_ndjson(BufReader::new(f), generation)
        .map_err(|e| input_invalid("in/dirs.ndjson", e))
}

/// The lines of `path` (`in/files.ndjson`) whose id is in `wanted`, in file order.
pub fn file_lines(path: &Path, wanted: &HashSet<i64>) -> Result<Vec<FileLine>, JobError> {
    let f = File::open(path).map_err(|e| input_invalid("in/files.ndjson", e))?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let line = line.map_err(|e| input_invalid("in/files.ndjson", e))?;
        if line.trim().is_empty() {
            continue;
        }
        let l: FileLine = serde_json::from_str(&line)
            .map_err(|e| input_invalid(&format!("in/files.ndjson line {}", i + 1), e))?;
        if wanted.contains(&l.id) {
            out.push(l);
        }
    }
    Ok(out)
}
