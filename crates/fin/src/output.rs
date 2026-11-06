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
    // A name the rules refuse: an empty or whitespace-only project name, or a branch or
    // checkpoint name outside `[A-Za-z0-9._-]{1,64}` (4B Decision 11).
    "INVALID_NAME",
    // Stored history that this code cannot or will not read (sub-project 4).
    "DB_TOO_NEW",
    "NOT_A_PROJECT_DB",
    // `fin project verify` found a problem (`details.failed`, `details.report`).
    "VERIFY_FAILED",
    // A project file or segment naming a hash algorithm this build does not know.
    "UNSUPPORTED_HASH",
    // A damaged database with no way to rebuild it (no rev 0 anywhere).
    "DB_CORRUPT",
    // A manual rebuild that would drop records the folder no longer holds; a project
    // folder written under another genesis than the project file's.
    "REBUILD_WOULD_LOSE",
    "GENESIS_MISMATCH",
    "NEWER_FORMAT",
    "INVALID_FORMAT",
    "REPLAY_FAILED",
    "CLIP_OVERLAP",
    "CLIP_EMPTY",
    "CLIP_NEGATIVE",
    "INVALID_ID",
    "BINDINGS_MISMATCH",
    // A stored (migrated) record too large for a segment: export stops at it.
    "RECORD_TOO_LARGE",
    // A copy of a project folder opened by the server that uses the original (plan
    // Decision 6), or one whose original cannot be reached to tell.
    "PROJECT_COPY",
    "PROJECT_ORIGINAL_UNREACHABLE",
    // Branches (sub-project 4B): a display name an active or merged branch holds; a write to
    // an archived branch (`fix`: restore it).
    "NAME_TAKEN",
    "BRANCH_ARCHIVED",
    // History (sub-project 4B): the author has nothing to undo or redo on the branch.
    "NOTHING_TO_UNDO",
    "NOTHING_TO_REDO",
];

/// Codes that mean "the project moved under you; read it again and retry" (exit 4).
/// `SEGMENT_COLLISION`: the project folder holds another file under the name this server's
/// export needs (the folder changed under it); the export retries under another name.
/// `PROJECT_READ_ONLY`: the project cannot take edits now (`details.reason`: `CHAIN_FORK`,
/// `CONFLICT_COPY`, `NEWER_FORMAT`, `IMPORT_ERROR`, `LEASE_LOST`, `LEASE_HELD`,
/// `CATCHING_UP`).
/// `LEASE_TAKEOVER_REFUSED`: `fin project takeover` found a live holder, a sync folder without
/// `--confirm`, or a holder not yet confirmed stopped (`details.reason`).
/// `LEASE_LOST`: this server's lease failed its fencing check (writes surface it as
/// `PROJECT_READ_ONLY` with reason `LEASE_LOST`).
/// `HISTORY_REWOUND`: history the caller saw on `details.branch` from `details.rev` on was
/// set aside (an applied line lost to a newer lease epoch, or a local tail diverged); read
/// the branch again from before `details.rev`.
/// `TX_SET_ASIDE`: an `--idem-key` names a transaction that committed once but was set aside
/// since (`details.reason`); its edit is not in the project now.
const CONFLICT_CODES: &[&str] = &[
    "CONFLICT",
    "BASE_REV_AHEAD",
    "SEGMENT_COLLISION",
    "PROJECT_READ_ONLY",
    "LEASE_TAKEOVER_REFUSED",
    "LEASE_LOST",
    "HISTORY_REWOUND",
    "TX_SET_ASIDE",
];

/// Codes that mean "this request can never succeed as sent" (exit 2, like `USAGE`).
/// `TX_TOO_LARGE`: a transaction whose record would not fit a segment; split it.
const USAGE_CODES: &[&str] = &["USAGE", "TX_TOO_LARGE"];

/// Codes that mean "unavailable, in use, or try again later" (exit 5): no usable server, or
/// a resource another process holds (`DB_IN_USE`: a rebuild needs the project database to
/// itself). `SERVER_TIMEOUT` is fin's own: the server
/// accepted the connection but did not answer in time. `ENGINE_CLOSED`, `ENGINE_BUSY` (a
/// verify, rebuild, compaction or takeover has the project for now),
/// `SERVER_SHUTTING_DOWN` and `CANCELLED` (the server interrupted the request, e.g. while
/// stopping) are transient: retrying against a (re)started server can succeed.
const UNAVAILABLE_CODES: &[&str] = &[
    "SERVER_UNAVAILABLE",
    "SERVER_RUNNING",
    "SERVER_START_FAILED",
    "SERVER_TIMEOUT",
    "ENGINE_CLOSED",
    "ENGINE_BUSY",
    "SERVER_SHUTTING_DOWN",
    "CANCELLED",
    "DB_IN_USE",
];

/// The one place error codes become exit codes.
pub fn exit_code(code: &str) -> i32 {
    match code {
        c if USAGE_CODES.contains(&c) => USAGE,
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
        assert_eq!(exit_code("TX_TOO_LARGE"), 2);
        assert_eq!(exit_code("CONFLICT"), 4);
        assert_eq!(exit_code("SEGMENT_COLLISION"), 4);
        assert_eq!(exit_code("RECORD_TOO_LARGE"), 3);
        assert_eq!(exit_code("BASE_REV_AHEAD"), 4);
        assert_eq!(exit_code("PROJECT_READ_ONLY"), 4);
        assert_eq!(exit_code("LEASE_TAKEOVER_REFUSED"), 4);
        assert_eq!(exit_code("LEASE_LOST"), 4);
        assert_eq!(exit_code("HISTORY_REWOUND"), 4);
        assert_eq!(exit_code("TX_SET_ASIDE"), 4);
        assert_eq!(exit_code("UNSUPPORTED_HASH"), 3);
        assert_eq!(exit_code("VERIFY_FAILED"), 3);
        assert_eq!(exit_code("DB_IN_USE"), 5);
        for code in [
            "SERVER_UNAVAILABLE",
            "SERVER_RUNNING",
            "SERVER_START_FAILED",
            "SERVER_TIMEOUT",
            "ENGINE_CLOSED",
            "ENGINE_BUSY",
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
            "INVALID_NAME",
            "IDEM_KEY_REUSED",
            "DB_TOO_NEW",
            "NOT_A_PROJECT_DB",
            "NEWER_FORMAT",
            "INVALID_FORMAT",
            "REPLAY_FAILED",
            "CLIP_OVERLAP",
            "CLIP_EMPTY",
            "CLIP_NEGATIVE",
            "INVALID_ID",
            "BINDINGS_MISMATCH",
            "PROJECT_COPY",
            "PROJECT_ORIGINAL_UNREACHABLE",
            "NAME_TAKEN",
            "BRANCH_ARCHIVED",
            "NOTHING_TO_UNDO",
            "NOTHING_TO_REDO",
        ] {
            assert_eq!(exit_code(code), 3, "{code}");
        }
        for code in VALIDATION_CODES {
            assert_eq!(exit_code(code), 3, "{code}");
        }
    }
}
