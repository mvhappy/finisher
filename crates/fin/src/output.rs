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
    // A media file `fin-container` refuses (sub-project 6 spec §17), beside
    // UNSUPPORTED_CONTAINER: what the file is, not how the request was made.
    "UNSUPPORTED_TRANSFORM",
    "MEDIA_CORRUPT",
    "MEDIA_LIMIT",
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
    // A path the server got was relative or had `.`/`..` segments (`details.field`).
    "INVALID_PATH",
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
    // Edits (sub-project 4C): an item op on a locked track (Decision 26); an overwrite
    // strictly inside one item (split it first); a roll with no touching neighbour; a link of
    // an item in a group already; a slide with no room; a ripple move inside the item itself;
    // a transition without the source it reads past the cut, at a cut that has one, or left
    // invalid (replay).
    "TRACK_LOCKED",
    "OVERWRITE_SPLITS_ITEM",
    "NOT_ADJACENT",
    "ALREADY_LINKED",
    "SLIDE_BLOCKED",
    "MOVE_INTO_ITSELF",
    // A link group's moves meeting on one track (4C Tasks 9–10 review).
    "LINK_LEGS_OVERLAP",
    "TRANSITION_NO_HANDLES",
    "TRANSITION_EXISTS",
    "TRANSITION_INVALID",
    // Structure (sub-project 4C Task 12): a track removal of a track holding items without
    // `withItems`; a rate change of a timeline holding items; a removal of a timeline a nested
    // item shows. Settings: an unknown path; a value of the wrong shape.
    "TRACK_NOT_EMPTY",
    "TIMELINE_NOT_EMPTY",
    "TIMELINE_NESTED",
    "PARAM_UNKNOWN",
    "PARAM_TYPE",
    // Settings and animation (sub-project 4C Tasks 11 and 14): a graphic slot its template
    // lacks; a static set of a path keyframes animate; a keyframe on a path that cannot be
    // animated.
    "SLOT_UNKNOWN",
    "PARAM_ANIMATED",
    "PARAM_NOT_ANIMATABLE",
    // Library (sub-project 4C Task 13): a removal of a bin holding assets or bins; a bin
    // moved into itself or a bin inside it.
    "BIN_NOT_EMPTY",
    "BIN_CYCLE",
    // Evaluation (sub-project 4C Tasks 15–16): a timeline nesting itself or nesting more than
    // 16 deep (the root counted); nested items holding more than 10,000 layers at once.
    "NESTED_CYCLE",
    "NESTED_TOO_LARGE",
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
    // Fonts (sub-project 4C): a file `fin_text::font_info` cannot read as a font; a SHA-256,
    // family or entry the font catalog does not list; a catalog breaking a gate rule
    // (`details.rule`, `details.path`).
    "FONT_INVALID",
    "FONT_NOT_IN_CATALOG",
    "FONT_CATALOG_INVALID",
    // Captions (sub-project 4C Task 18): a removal of a style a caption uses; a pin of a face
    // already pinned to another file (`fix`: repin); an unpin of a face a style or caption
    // uses; an SRT file breaking the format (`details.line`).
    "STYLE_IN_USE",
    "FONT_PINNED",
    "FONT_IN_USE",
    "SRT_INVALID",
    // A caption laid out from its pinned fonts overflows its style's lines or safe zone
    // (sub-project 4C Tasks 19–20: checked once at the end of a transaction).
    "CAPTION_OVERFLOW",
    // `diagnostics.record` refused an event (plan D, Decision 33): seen only by direct callers.
    "EVENT_KIND_UNKNOWN",
    "EVENT_INVALID",
    "EVENT_TOO_LARGE",
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
/// `CONSENT_UI_ONLY` (plan D, Decision 14b): turning `training` on is a person's choice (the
/// `fix` names the ways in).
const USAGE_CODES: &[&str] = &["USAGE", "TX_TOO_LARGE", "CONSENT_UI_ONLY"];

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
    // The server's control database (`control.db`) was locked past its busy timeout.
    "CONTROL_DB_BUSY",
    // `fin fonts fetch` could not download a font (`details.reason`, `details.retryAt`); in
    // other replies it is a warning, not an error.
    "FONT_FETCH_FAILED",
    // The font cache or catalog is unusable, so `fin fonts …` can fetch nothing until it is
    // repaired; in other replies it is a warning that never blocks edits.
    "FONTS_DAMAGED",
    // `fin diagnostics …` (plan D, Decision 33): the journal is suspended or the server data is
    // unwritable; `settings.json` does not parse (or its `privacy` section breaks the journal's
    // rules) and a consent change would overwrite it; the consent change could not be written
    // (the file is unchanged). Each `fix` names the reason.
    "JOURNAL_UNAVAILABLE",
    "SETTINGS_INVALID",
    "SETTINGS_WRITE_FAILED",
    // The compositor's GPU (sub-project 6 spec §17): no adapter, not even a software one; the
    // device was lost; an allocation failed. 5C retries the last two on a fresh worker.
    "GPU_UNAVAILABLE",
    "GPU_DEVICE_LOST",
    "GPU_OUT_OF_MEMORY",
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

/// Turns panics into an `INTERNAL` error document (exit 6) instead of a stderr-only trace.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Best effort and within its budget (`diag`); then the usual document and exit 6.
        crate::diag::report_panic(info);
        let message = crate::diag::panic_message(info);
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
        for code in [
            "UNSUPPORTED_CONTAINER",
            "UNSUPPORTED_TRANSFORM",
            "MEDIA_CORRUPT",
            "MEDIA_LIMIT",
        ] {
            assert_eq!(exit_code(code), 3, "{code}");
        }
        assert_eq!(exit_code("VERIFY_FAILED"), 3);
        assert_eq!(exit_code("DB_IN_USE"), 5);
        assert_eq!(exit_code("CONTROL_DB_BUSY"), 5);
        assert_eq!(exit_code("CONSENT_UI_ONLY"), 2);
        assert_eq!(exit_code("JOURNAL_UNAVAILABLE"), 5);
        assert_eq!(exit_code("SETTINGS_INVALID"), 5);
        assert_eq!(exit_code("SETTINGS_WRITE_FAILED"), 5);
        assert_eq!(exit_code("EVENT_KIND_UNKNOWN"), 3);
        assert_eq!(exit_code("EVENT_INVALID"), 3);
        assert_eq!(exit_code("EVENT_TOO_LARGE"), 3);
        assert_eq!(exit_code("FONT_FETCH_FAILED"), 5);
        for code in ["GPU_UNAVAILABLE", "GPU_DEVICE_LOST", "GPU_OUT_OF_MEMORY"] {
            assert_eq!(exit_code(code), 5, "{code}");
        }
        assert_eq!(exit_code("FONTS_DAMAGED"), 5);
        assert_eq!(exit_code("TRACK_LOCKED"), 3);
        assert_eq!(exit_code("OVERWRITE_SPLITS_ITEM"), 3);
        assert_eq!(exit_code("NOT_ADJACENT"), 3);
        for code in [
            "ALREADY_LINKED",
            "SLIDE_BLOCKED",
            "MOVE_INTO_ITSELF",
            "LINK_LEGS_OVERLAP",
            "TRANSITION_NO_HANDLES",
            "TRANSITION_EXISTS",
            "TRANSITION_INVALID",
            "TRACK_NOT_EMPTY",
            "TIMELINE_NOT_EMPTY",
            "TIMELINE_NESTED",
            "PARAM_UNKNOWN",
            "PARAM_TYPE",
            "SLOT_UNKNOWN",
            "PARAM_ANIMATED",
            "PARAM_NOT_ANIMATABLE",
            "BIN_NOT_EMPTY",
            "BIN_CYCLE",
            "NESTED_CYCLE",
            "NESTED_TOO_LARGE",
        ] {
            assert_eq!(exit_code(code), 3, "{code}");
        }
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
            "INVALID_PATH",
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
            "FONT_INVALID",
            "FONT_NOT_IN_CATALOG",
            "FONT_CATALOG_INVALID",
        ] {
            assert_eq!(exit_code(code), 3, "{code}");
        }
        for code in VALIDATION_CODES {
            assert_eq!(exit_code(code), 3, "{code}");
        }
    }
}
