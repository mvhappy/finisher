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
use fin_plugin_api::protocol::errors::codes::{
    PLUGIN_COMMAND_FAILED, PLUGIN_OUTPUT_INVALID, PLUGIN_PERMISSION_DENIED, PLUGIN_PROTOCOL,
};
pub use fin_shm::{RingReader, RingSpec, RingWriter, SlotState};

use crate::host::Host;

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

/// A `fin-shm` refusal (`SHM_REFUSED {file, reason}`) of the job file `name`, by whose fault
/// it is: an input that is not what the host granted is `PLUGIN_PROTOCOL {reason, file}`, an
/// output the plugin cannot make is `PLUGIN_OUTPUT_INVALID {reason, file}` (spec §13); `reason`
/// is `size | link | notRegular | truncated` (other refusals keep `fin-shm`'s name), an I/O
/// failure is `PLUGIN_COMMAND_FAILED {reason: io, file}`. The file is named by its grant, never
/// by its path.
fn error_from_shm(e: CoreError, name: &str, input: bool) -> PluginError {
    let shm = e.details["reason"].as_str().unwrap_or("io");
    let reason = match shm {
        "overCap" | "sizeMismatch" => "size",
        "link" | "hardLinked" => "link",
        "changed" => "truncated",
        "io" | "busy" => {
            return PluginError::new(PLUGIN_COMMAND_FAILED, format!("{name}: an I/O error"))
                .with("reason", "io")
                .with("file", name);
        }
        other => other,
    };
    let code = if input {
        PLUGIN_PROTOCOL
    } else {
        PLUGIN_OUTPUT_INVALID
    };
    let mut out = PluginError::new(code, format!("the job file {name:?} is refused: {reason}"))
        .with("reason", reason)
        .with("file", name);
    out.fix = e.fix;
    out
}

/// `PLUGIN_PERMISSION_DENIED {perm: fs, access, file}`: the grant's mode does not allow
/// `access` (`read` or `write`).
fn mode_error(grant: &FileGrant, access: &str) -> PluginError {
    PluginError::new(
        PLUGIN_PERMISSION_DENIED,
        format!("the file {:?} is not granted for {access}", grant.name),
    )
    .with("perm", "fs")
    .with("access", access)
    .with("file", grant.name.clone())
}

/// An I/O failure on the job file `name`: `PLUGIN_COMMAND_FAILED {reason: io, file}` (the
/// grant's name, never its path).
fn io_error(name: &str, e: std::io::Error) -> PluginError {
    PluginError::from(e).with("file", name)
}

/// The whole of an input file, at most `cap` bytes, and exactly its granted size when the
/// grant has one. A file over `cap`, or not its granted size, is `PLUGIN_PROTOCOL {reason:
/// size}` (a `cap` below the granted size therefore always fails); a link `{reason: link}`, a
/// non-regular file `{reason: notRegular}`.
pub fn read_input(grant: &FileGrant, cap: u64) -> Result<Vec<u8>, PluginError> {
    if grant.mode == FileMode::Write {
        return Err(mode_error(grant, "read"));
    }
    let cap = grant.bytes.map_or(cap, |b| b.min(cap));
    let file = fin_shm::checked_open(&grant.path, cap)
        .map_err(|e| error_from_shm(e, &grant.name, true))?;
    let mut out = Vec::new();
    file.take(cap)
        .read_to_end(&mut out)
        .map_err(|e| io_error(&grant.name, e))?;
    if grant.bytes.is_some_and(|b| b != out.len() as u64) {
        return Err(PluginError::new(
            PLUGIN_PROTOCOL,
            format!("the input {:?} is not its granted size", grant.name),
        )
        .with("reason", "size")
        .with("file", grant.name.clone()));
    }
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
        return Err(mode_error(grant, "write"));
    }
    if grant.bytes.is_some_and(|b| b != declared) {
        return Err(size_error(
            &grant.name,
            "size",
            "the declared size is not the granted one",
        ));
    }
    let file =
        fin_shm::create_output(&grant.path).map_err(|e| error_from_shm(e, &grant.name, false))?;
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
        self.file
            .write_all(bytes)
            .map_err(|e| io_error(&self.name, e))?;
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
        self.file.sync_all().map_err(|e| io_error(&self.name, e))?;
        Ok(self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn io_failures_are_command_failures() {
        let e = io_error("pcm.bin", std::io::Error::other("device gone"));
        assert_eq!(e.code, "PLUGIN_COMMAND_FAILED");
        assert_eq!(
            serde_json::Value::Object(e.details),
            serde_json::json!({ "reason": "io", "file": "pcm.bin" })
        );
    }
}
