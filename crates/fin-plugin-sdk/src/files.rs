//! Job files for cold kinds (8C onward; plan 8B Decision 5, feature `files`): checked reads of
//! `in/` files and declared-size writes to `out/`, under `fin-shm`'s rules (no link at any
//! segment, a regular file with one link, a size cap). Rings and their slot messages are
//! `fin-shm`'s own types, re-exported. Command jobs have no files.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

use fin_core::CoreError;
use fin_jobs::{FileGrant, FileMode};
use fin_plugin_api::protocol::PluginError;
pub use fin_shm::{RingReader, RingSpec, RingWriter, SlotState};
use serde_json::Value;

use crate::host::Host;

/// `PLUGIN_OUTPUT_INVALID {reason}` (spec §13).
pub const PLUGIN_OUTPUT_INVALID: &str = "PLUGIN_OUTPUT_INVALID";

impl Host {
    /// The job's mapped files (`start.files`).
    pub fn files(&self) -> &[FileGrant] {
        &self.files
    }

    /// The mapped file `name`.
    pub fn file(&self, name: &str) -> Option<&FileGrant> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// A `fin-shm` refusal (`SHM_REFUSED {file, reason}`) as a [`PluginError`].
pub fn error_from_core(e: CoreError) -> PluginError {
    let details = match e.details {
        Value::Object(m) => m,
        _ => Default::default(),
    };
    PluginError {
        code: e.code,
        message: e.message,
        details,
        fix: e.fix,
    }
}

fn mode_error(grant: &FileGrant, want: &str) -> PluginError {
    PluginError::new(
        "PLUGIN_PERMISSION_DENIED",
        format!("the file {:?} is not granted for {want}", grant.name),
    )
    .with("file", grant.name.clone())
}

/// The whole of an input file, at most `cap` bytes (and at most its granted size).
pub fn read_input(grant: &FileGrant, cap: u64) -> Result<Vec<u8>, PluginError> {
    if grant.mode == FileMode::Write {
        return Err(mode_error(grant, "reading"));
    }
    let cap = grant.bytes.map_or(cap, |b| b.min(cap));
    let file = fin_shm::checked_open(&grant.path, cap).map_err(error_from_core)?;
    let mut out = Vec::new();
    file.take(cap).read_to_end(&mut out).map_err(|e| {
        PluginError::new("IO_ERROR", e.to_string()).with("file", grant.name.clone())
    })?;
    Ok(out)
}

/// An output file that must receive exactly its declared size.
pub struct OutputFile {
    file: File,
    name: String,
    path: PathBuf,
    declared: u64,
    written: u64,
}

/// Creates the output `grant` for exactly `declared` bytes (a grant with a size must declare
/// that size).
pub fn create_output(grant: &FileGrant, declared: u64) -> Result<OutputFile, PluginError> {
    if grant.mode == FileMode::Read {
        return Err(mode_error(grant, "writing"));
    }
    if grant.bytes.is_some_and(|b| b != declared) {
        return Err(size_error(
            &grant.name,
            "size",
            "the declared size is not the granted one",
        ));
    }
    let file = fin_shm::create_output(&grant.path).map_err(error_from_core)?;
    Ok(OutputFile {
        file,
        name: grant.name.clone(),
        path: grant.path.clone(),
        declared,
        written: 0,
    })
}

fn size_error(name: &str, reason: &str, message: &str) -> PluginError {
    PluginError::new(PLUGIN_OUTPUT_INVALID, message)
        .with("reason", reason)
        .with("file", name)
}

impl OutputFile {
    /// Appends `bytes`; writing past the declared size is `PLUGIN_OUTPUT_INVALID {size}`.
    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), PluginError> {
        if self.written + bytes.len() as u64 > self.declared {
            return Err(size_error(
                &self.name,
                "size",
                "the write passes the declared size",
            ));
        }
        self.file.write_all(bytes).map_err(|e| self.io(&e))?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    /// Syncs and closes the file; fewer bytes than declared is
    /// `PLUGIN_OUTPUT_INVALID {truncated}`.
    pub fn finish(self) -> Result<PathBuf, PluginError> {
        if self.written != self.declared {
            return Err(size_error(
                &self.name,
                "truncated",
                "fewer bytes than declared",
            ));
        }
        self.file.sync_all().map_err(|e| self.io(&e))?;
        Ok(self.path)
    }

    fn io(&self, e: &std::io::Error) -> PluginError {
        PluginError::new("IO_ERROR", e.to_string()).with("file", self.name.clone())
    }
}
