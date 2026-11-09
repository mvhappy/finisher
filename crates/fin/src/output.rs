//! The output contract: exactly one JSON document on stdout, stable exit codes, nothing on
//! stderr on success. Errors are `{ "error": { code, message, details, fix? } }` (spec §19).

use std::io::Write;

use fin_core::CoreError;
use serde_json::{Value, json};

pub const OK: i32 = 0;
pub const USAGE: i32 = 2;
pub const VALIDATION: i32 = 3;
pub const CONFLICT: i32 = 4;
pub const UNAVAILABLE: i32 = 5;
pub const INTERNAL: i32 = 6;
/// "This needs a different principal, usually a person" (spec §3.5): an agent must not retry.
pub const DENIED: i32 = 7;
/// "A person has been asked; wait, then repeat" (sub-project 7 §7.3, §15): never a denial.
pub const PENDING: i32 = 8;

// The code tables, one per exit (split out when 6B-1 merged integration, to keep this file
// under the size limit; `exit_code` below reads them).
#[path = "output_codes.rs"]
mod codes;
pub use codes::DENIED_CODES;
use codes::*;

/// The one place error codes become exit codes.
pub fn exit_code(code: &str) -> i32 {
    match code {
        c if DENIED_CODES.contains(&c) => DENIED,
        c if PENDING_CODES.contains(&c) => PENDING,
        c if USAGE_CODES.contains(&c) => USAGE,
        c if CONFLICT_CODES.contains(&c) || JOB_RESULT_CODES.contains(&c) => CONFLICT,
        c if UNAVAILABLE_CODES.contains(&c) => UNAVAILABLE,
        c if VALIDATION_CODES.contains(&c) => VALIDATION,
        _ => INTERNAL,
    }
}

pub fn usage(message: impl Into<String>, details: Value, fix: impl Into<String>) -> CoreError {
    CoreError::new("USAGE", message)
        .with_details(details)
        .with_fix(fix)
}

pub fn internal(message: impl Into<String>, details: Value) -> CoreError {
    CoreError::new("INTERNAL", message).with_details(details)
}

pub fn unavailable(details: Value) -> CoreError {
    CoreError::new("SERVER_UNAVAILABLE", "the Finisher server is not reachable")
        .with_details(details)
        .with_fix("start finisherd")
}

fn print(doc: &Value) {
    let mut out = std::io::stdout().lock();
    // A closed stdout (e.g. `| head`) is not worth a second error; the exit code still tells.
    let _ = writeln!(out, "{doc}");
    let _ = out.flush();
}

static STREAMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// From here the command's answer is JSONL lines ([`stream_line`], none at all when empty):
/// [`ok`] prints no document of its own.
pub fn streaming() {
    STREAMED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Prints one JSONL line of a streamed answer (`fin diagnostics export`).
pub fn stream_line(line: &Value) {
    print(line);
}

/// Prints a success document and returns exit code 0.
pub fn ok(doc: &Value) -> i32 {
    if !STREAMED.load(std::sync::atomic::Ordering::Relaxed) {
        print(doc);
    }
    OK
}

/// Prints the error document and returns its exit code.
pub fn error(e: &CoreError) -> i32 {
    print(&json!({ "error": e }));
    exit_code(&e.code)
}

#[cfg(test)]
#[path = "output_tests.rs"]
mod tests;
