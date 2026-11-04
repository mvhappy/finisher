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

/// Codes that mean "the request was well-formed but rejected by validation" (exit 3).
const VALIDATION_CODES: &[&str] = &[
    "INVALID_TIME",
    "TIME_OVERFLOW",
    "NOT_FOUND",
    "INVALID_RANGE",
    "INSERT_INSIDE_CLIP",
    "TRACK_KIND_MISMATCH",
    "UNBOUND_PLACEHOLDER",
    "DUPLICATE_PLACEHOLDER",
    "SCHEMA_VERSION",
    "INVALID_OP",
    "DUPLICATE_ID",
    "ASSET_IN_USE",
    "ASSET_NOT_FOUND",
    "ASSET_UNREADABLE",
    "UNSUPPORTED_CONTAINER",
    "PROJECT_NOT_FOUND",
    "PROJECT_EXISTS",
    "PROJECT_INVALID",
    "BRANCH_NOT_FOUND",
    "PROJECT_DB_MISSING",
    "PROJECT_DB_MISMATCH",
    "SERVER_DATA_UNSAFE",
    "IDEM_KEY_REUSED",
    "PROJECT_DIR_UNWRITABLE",
    // Stored history that this code cannot or will not read (sub-project 4).
    "NEWER_FORMAT",
    "INVALID_FORMAT",
    "REPLAY_FAILED",
    "CLIP_OVERLAP",
    "BINDINGS_MISMATCH",
];

/// Codes that mean "the project moved under you; read it again and retry" (exit 4).
const CONFLICT_CODES: &[&str] = &["CONFLICT", "BASE_REV_AHEAD"];

/// Codes that mean "no usable server" (exit 5). `SERVER_TIMEOUT` is fin's own: the server
/// accepted the connection but did not answer in time. `ENGINE_CLOSED`,
/// `SERVER_SHUTTING_DOWN` and `CANCELLED` (the server interrupted the request, e.g. while
/// stopping) are transient: retrying against a (re)started server can succeed.
const UNAVAILABLE_CODES: &[&str] = &[
    "SERVER_UNAVAILABLE",
    "SERVER_RUNNING",
    "SERVER_START_FAILED",
    "SERVER_TIMEOUT",
    "ENGINE_CLOSED",
    "SERVER_SHUTTING_DOWN",
    "CANCELLED",
];

/// The one place error codes become exit codes.
pub fn exit_code(code: &str) -> i32 {
    match code {
        "USAGE" => USAGE,
        c if CONFLICT_CODES.contains(&c) => CONFLICT,
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

/// Prints a success document and returns exit code 0.
pub fn ok(doc: &Value) -> i32 {
    print(doc);
    OK
}

/// Prints the error document and returns its exit code.
pub fn error(e: &CoreError) -> i32 {
    print(&json!({ "error": e }));
    exit_code(&e.code)
}

/// Turns panics into an `INTERNAL` error document (exit 6) instead of a stderr-only trace.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = match info.payload().downcast_ref::<&str>() {
            Some(s) => (*s).to_string(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "panic".to_string()),
        };
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()));
        let e = internal(
            format!("fin crashed: {message}"),
            json!({ "location": location }),
        )
        .with_fix("report this as a bug in fin");
        std::process::exit(error(&e));
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_the_contract() {
        assert_eq!(exit_code("USAGE"), 2);
        assert_eq!(exit_code("CONFLICT"), 4);
        assert_eq!(exit_code("BASE_REV_AHEAD"), 4);
        for code in [
            "SERVER_UNAVAILABLE",
            "SERVER_RUNNING",
            "SERVER_START_FAILED",
            "SERVER_TIMEOUT",
            "ENGINE_CLOSED",
            "SERVER_SHUTTING_DOWN",
            "CANCELLED",
        ] {
            assert_eq!(exit_code(code), 5, "{code}");
        }
        for code in [
            "INTERNAL",
            "SOMETHING_NEW",
            "PROTOCOL_ERROR",
            "PROTOCOL_MISMATCH",
            "DB_ERROR",
            "IO_ERROR",
        ] {
            assert_eq!(exit_code(code), 6, "{code}");
        }
        for code in [
            "PROJECT_NOT_FOUND",
            "PROJECT_EXISTS",
            "PROJECT_INVALID",
            "BRANCH_NOT_FOUND",
            "PROJECT_DB_MISSING",
            "PROJECT_DB_MISMATCH",
            "SERVER_DATA_UNSAFE",
            "PROJECT_DIR_UNWRITABLE",
            "IDEM_KEY_REUSED",
            "NEWER_FORMAT",
            "INVALID_FORMAT",
            "REPLAY_FAILED",
            "CLIP_OVERLAP",
            "BINDINGS_MISMATCH",
        ] {
            assert_eq!(exit_code(code), 3, "{code}");
        }
        for code in VALIDATION_CODES {
            assert_eq!(exit_code(code), 3, "{code}");
        }
    }
}
