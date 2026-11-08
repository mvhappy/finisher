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

/// Codes that mean denied (exit 7, spec §3.5 and §4.13): the server would not accept this
/// caller, or this caller would not accept the server (`SERVER_UNTRUSTED`).
pub const DENIED_CODES: &[&str] = &[
    "UNAUTHENTICATED",
    "SERVER_UNTRUSTED",
    "SCOPE_DENIED",
    "HUMAN_ONLY",
    "PATH_NOT_ALLOWED",
    "BRANCH_PROTECTED",
    "BRANCH_NOT_ALLOWED",
    "CREDENTIAL_REVOKED",
    // A credential that expired (review of 5A Tasks 8–9, L4): the server proved itself, then
    // refused it; only the owner can mint a new one.
    "CREDENTIAL_EXPIRED",
    "APPROVAL_REQUIRED",
    "CONSOLE_NEEDS_TTY",
    "CONFIRMATION_REQUIRED",
    // Plugin registries (sub-project 8A Task 9a, spec §13): a person must look. The root is not
    // the pinned one, or signed metadata did not verify (`TUF_*`: signatures, thresholds,
    // rollback, expiry beyond `maxStaleDays`, delegations, a target that is not what was signed).
    "REGISTRY_UNTRUSTED",
    "TUF_SIGNATURE",
    "TUF_THRESHOLD",
    "TUF_ROLLBACK",
    "TUF_EXPIRED",
    "TUF_DELEGATION",
    "TUF_TARGET_MISMATCH",
    // Plugins (8A Tasks 9b, 10, 12c): a version a signed advisory revoked; a bundled plugin
    // whose files are not what the app was built with; a plan approved without accepting its
    // copyleft terms or its third-party code.
    "PLUGIN_REVOKED",
    "PLUGIN_BUNDLED_MISMATCH",
    "COPYLEFT_NOT_ACCEPTED",
    "THIRD_PARTY_CODE_NOT_ACCEPTED",
    // Agent sessions (7A-2 Task 6b): a session's token, money or wall-clock budget is used up
    // (`details.kind`, `used`, `limit`); a person raises it (`finisherd agent approve`).
    "BUDGET_EXCEEDED",
    // Agent sessions (sub-project 7 §15; 7A-2 Task 12): a person refused the approval; a
    // command rule (`details.rule`, `layer`) or the session's mode (`details.mode`, `command`)
    // forbids it; a hook blocked it (`details.hook`, `reason`); a subagent too deep (7B).
    "APPROVAL_DENIED",
    "RULE_DENIED",
    "MODE_FORBIDS",
    "HOOK_BLOCKED",
    "DEPTH_EXCEEDED",
];

/// Codes that mean "a person has been asked" (exit 8, sub-project 7 §7.3): the `ask` stage
/// opened `details.approval`, or `fin approval wait` timed out on it. Wait, then repeat.
const PENDING_CODES: &[&str] = &["APPROVAL_PENDING"];

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
    // A look's or export's destination is not writable or lies inside `.finisher/` or
    // `assets/` (spec §17): the request names it.
    "EXPORT_DESTINATION",
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
    // A frame the server could not take: not JSON, an unknown RPC, a payload its schema refuses
    // (5A Decision 13).
    "INVALID_REQUEST",
    // Jobs (5C-1 Decision 41): a kind no family registered; params its Rust type refuses
    // (`details.errors`); a job id the server does not know; a job host call on a group or
    // host the caller should know is gone (`details.reason`: `unknownGroup`, `groupClosed`,
    // `shutDown`), a caller's bug rather than something to retry.
    "JOB_KIND_UNKNOWN",
    "JOB_PARAMS_INVALID",
    "JOB_NOT_FOUND",
    "JOBHOST_STATE",
    // Agent sessions (7A-2 Task 3b): a session id the server does not know.
    "SESSION_NOT_FOUND",
    // Agent sessions (7A-2 Task 6b): no adapter for the harness on this server; an approval
    // id the server does not know.
    "HARNESS_UNKNOWN",
    "APPROVAL_NOT_FOUND",
    // Agent sessions (sub-project 7 §15; 7A-2 Task 12): a role, model or saved prompt the
    // server does not know (`MODEL_UNKNOWN {reason: noTools}` for a model without tools); a mode
    // above the role's ceiling; a bad `apply` list entry; an attachment too large or of a type
    // the model cannot take; the model catalogue or `agents.models` broken (`details`).
    "ROLE_UNKNOWN",
    "MODE_INVALID",
    "MODEL_UNKNOWN",
    "SAVED_PROMPT_UNKNOWN",
    "APPLY_INVALID",
    "ASSET_TOO_LARGE",
    "ASSET_TYPE_UNSUPPORTED",
    "CATALOGUE_INVALID",
    // The built-in agent's workspace tools (7A-2 Task 8b): a file that would pass the per-file
    // cap; a file `workspace_write {mode: create}` finds already there.
    "FILE_TOO_LARGE",
    "ALREADY_EXISTS",
    // The secret store (7A Task 5): no key with that id; a value it will not store (empty,
    // too long, not text); a store file that does not parse; a store folder or file that is not
    // private (`details.reason`).
    "SECRET_NOT_FOUND",
    "SECRET_INVALID",
    "SECRET_CORRUPT",
    "SECRET_STORE_UNSAFE",
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
    // Registries and plugins (sub-project 8A Task 9a): no such registry or plugin; a registry
    // whose metadata does not parse, breaks a size cap or the layout (`details.reason`).
    "REGISTRY_NOT_FOUND",
    "REGISTRY_INVALID",
    "PLUGIN_NOT_FOUND",
    // Packages and manifests (8A Tasks 2–3): a manifest or package that breaks the rules
    // (`details.path`, `details.reason`), a contribution this host does not consume, an unsafe
    // or oversized package, a host API, platform or Node version it does not fit.
    "PLUGIN_MANIFEST_INVALID",
    "PLUGIN_CONTRIBUTION_UNSUPPORTED",
    "PACKAGE_UNSAFE",
    "PACKAGE_TOO_LARGE",
    "PLUGIN_HOST_API",
    "PLUGIN_PLATFORM",
    "PLUGIN_NODE_VERSION",
    // The lifecycle (8A Task 9b): a plan gone or expired, or no longer what is installed;
    // nothing installed under that id; that version already installed; an older version
    // without `--allow-downgrade`; a bundled plugin (it updates with the app).
    "PLAN_EXPIRED",
    "PLAN_MISMATCH",
    "PLUGIN_NOT_INSTALLED",
    "PLUGIN_ALREADY_INSTALLED",
    "PLUGIN_DOWNGRADE_REFUSED",
    "PLUGIN_BUNDLED",
    // Libraries (sub-project 5D Decision 51): a root that cannot be a library, one that
    // overlaps another, a library or name the server does not know or already has, a file
    // or a path the catalog cannot answer (`details.fileIds` for an ambiguous one), a search
    // cursor that ran out or never was, a query the search refuses, a library that opens
    // nothing, two names the volume folds into one key (a warning).
    "LIBRARY_ROOT_INVALID",
    "LIBRARY_OVERLAP",
    "LIBRARY_NOT_FOUND",
    "LIBRARY_NAME_TAKEN",
    "FILE_NOT_FOUND",
    "PATH_AMBIGUOUS",
    "CURSOR_EXPIRED",
    "CURSOR_INVALID",
    "SEARCH_QUERY_INVALID",
    "LIBRARY_METADATA_ONLY",
    "NAME_COLLISION",
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
    // A registry with this id exists (8A Task 9a).
    "REGISTRY_EXISTS",
    // The server holds as many live agent and MCP credentials as it keeps (5A Task 9; review
    // of Tasks 8–9, L4): the server's state refuses it, not the caller (only the owner mints),
    // and waiting changes nothing: revoke some, then mint again. A conflict, not exit 5.
    "CREDENTIAL_LIMIT",
    // Libraries (5D): a file whose data is not on this machine (a placeholder; `hydrate` is
    // the owner's call) and one that differs from its catalogued row (rescan, then retry).
    "FILE_CLOUD_ONLY",
    "FILE_CHANGED",
    // Another live agent session works on this root branch (7A Task 7): end it, or start from
    // another branch.
    "BRANCH_IN_USE",
    // Agent sessions (7A-2 Task 6b): a run is active (steer it, or wait); the session ended.
    "SESSION_BUSY",
    "SESSION_ENDED",
    // An approval answered after its `expiresAt` (7A-2 review): it was expired instead; ask
    // for it again.
    "APPROVAL_EXPIRED",
    // Sub-project 7 §6.5: the `auto` merge policy needs sub-project 10's QA gate and critic;
    // the session's workspace holds its 256 MiB (7A-2 Task 8b): remove files first.
    "AUTO_MERGE_UNAVAILABLE",
    "WORKSPACE_FULL",
];

/// Codes of a job that ended without succeeding (exit 4, 5C-1 Decision 41): its outcome, and
/// the result codes a failed job carries (`fin job` and `fin wait` report them).
const JOB_RESULT_CODES: &[&str] = &[
    "JOB_FAILED",
    "JOB_CANCELLED",
    "JOB_INTERRUPTED",
    "WORKER_CRASHED",
    "WORKER_HUNG",
    "JOB_STALLED",
    "JOB_TIMEOUT",
    "POISON_ITEM",
    "HELPER_FAILED",
];

/// Codes that mean "this request can never succeed as sent" (exit 2, like `USAGE`).
/// `TX_TOO_LARGE`: a transaction whose record would not fit a segment; split it.
/// `CONSENT_UI_ONLY` (plan D, Decision 14b): turning `training` on is a person's choice (the
/// `fix` names the ways in).
/// `FRAME_TOO_LARGE`: a request over the server's 16 MiB frame limit (5A Decision 13).
const USAGE_CODES: &[&str] = &[
    "USAGE",
    "TX_TOO_LARGE",
    "CONSENT_UI_ONLY",
    "FRAME_TOO_LARGE",
    // Plugins (8A): a manifest over 256 KiB; several registries list the plugin (or its
    // registry's root changed, `details.reason: rootChanged`): name one with `--registry`.
    "MANIFEST_TOO_LARGE",
    "REGISTRY_AMBIGUOUS",
    // Bad `fin agent` usage (sub-project 7 §15).
    "AGENT_ARGS",
];

/// Codes that mean "unavailable, in use, or try again later" (exit 5): no usable server, or
/// a resource another process holds (`DB_IN_USE`: a rebuild needs the project database to
/// itself). `SERVER_TIMEOUT` is fin's own: the server
/// accepted the connection but did not answer in time. `ENGINE_CLOSED`, `ENGINE_BUSY` (a
/// verify, rebuild, compaction or takeover has the project for now),
/// `SERVER_SHUTTING_DOWN` and `CANCELLED` (the server interrupted the request, e.g. while
/// stopping) are transient: retrying against a (re)started server can succeed.
const UNAVAILABLE_CODES: &[&str] = &[
    // Too many WebSocket tickets at once (5A, review of Task 6): wait and retry.
    "TICKET_LIMIT",
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
    // A job's mapped file failed `fin-shm`'s checks (a link, a hard link, a special file, a
    // size against its ring header or cap, a truncation while read; sub-project 6 spec §17):
    // the job failed, not the request.
    "SHM_REFUSED",
    // Jobs (5C-1 Decision 41): 50,000 unfinished jobs; no worker could be started
    // (`details.reason`); a worker built for another envelope (`details.expected`, `got`); a
    // wait that timed out; an `io:` pool two abandoned processes hold (`details.pool`); a job's
    // group over its memory limit (`details.limit`, `used`); a `jobs.watch` reader that fell
    // `details.buffer` events behind (5C-2: list again, then watch again); a principal with
    // `details.limit` `jobs.wait`/`jobs.watch` calls already open (5C-2 review M1).
    "JOB_QUEUE_FULL",
    "WORKER_UNAVAILABLE",
    "WORKER_PROTOCOL_MISMATCH",
    "WAIT_TIMEOUT",
    "RESOURCE_STUCK",
    "WORKER_OOM",
    "WATCH_LAGGING",
    "TOO_MANY_WATCHES",
    // A registry folder that cannot be read; the clock is more than a day before the last
    // refresh (`fix`: set the clock, then refresh) (8A Task 9a). `REGISTRY_STALE` is a warning.
    "REGISTRY_UNREACHABLE",
    "CLOCK_SUSPECT",
    // `REGISTRY_STALE` (a warning in `plugin ls`); a disabled plugin; a code plugin with no
    // runtime yet; too many open plans or suggestions for this principal (8A Tasks 9b, 12a):
    // wait, then retry.
    "REGISTRY_STALE",
    "PLUGIN_DISABLED",
    "PLUGIN_RUNTIME_UNAVAILABLE",
    "TOO_MANY_PLANS",
    "TOO_MANY_SUGGESTIONS",
    // Libraries (5D Decision 51): a library offline or paused, a root that cannot be read, a
    // catalog newer than this build or unusable, and the reasons and warnings a status shows
    // (`library.status`): wait, or the owner acts, then retry.
    "LIBRARY_OFFLINE",
    "LIBRARY_PAUSED",
    "LIBRARY_ROOT_UNREADABLE",
    "CATALOG_TOO_NEW",
    "CATALOG_UNAVAILABLE",
    "CATALOG_NOT_OPEN",
    "ROOT_UNMOUNTED",
    "ROOT_VOLUME_CHANGED",
    "ROOT_EMPTY_SUSPECT",
    "ROOT_TIMEOUT",
    "ROOT_UNREACHABLE",
    "DIR_EMPTY_SUSPECT",
    "WATCH_DEGRADED",
    "WATCH_OVERFLOW",
    "WATCH_ERROR",
    "ENUMERATION_FALLBACK",
    // Agent sessions (7A-2 Task 6b): the session is interrupted (resume it); its provider
    // exited or could not start (`details.code`, `signal`). Both are resumable.
    "SESSION_INTERRUPTED",
    "PROVIDER_EXITED",
    // Agent sessions (7A-2 review): a non-local model without a price cannot keep its money
    // budget (refresh the catalogue or pick another model); an `agent.watch` that fell
    // behind (watch again from `details.lastSeq`).
    "PRICING_UNKNOWN",
    "WATCH_LAGGING",
    // Providers (sub-project 7 §15; 7A-2 Task 12): not installed, or the local model server
    // does not answer; a CLI below the version floor or missing a capability; not signed in,
    // or the OpenRouter key missing or refused; the CLI reports another account; rate limited
    // (`details.retryAfterMs`) or overloaded; a message the adapter cannot map (drift); the
    // prompt would pass the model's window; a sandbox present but not applied; a tool or a
    // blocking hook that ran past its time; the secret store busy or unusable on this host.
    "PROVIDER_UNAVAILABLE",
    "PROVIDER_VERSION",
    "PROVIDER_AUTH",
    "PROVIDER_ACCOUNT_CHANGED",
    "PROVIDER_RATE_LIMITED",
    "PROVIDER_OVERLOADED",
    "PROVIDER_PROTOCOL",
    "MODEL_CONTEXT_EXCEEDED",
    "SANDBOX_UNAVAILABLE",
    "TOOL_TIMEOUT",
    "HOOK_TIMEOUT",
    "SECRET_STORE_BUSY",
    "SECRET_STORE_UNAVAILABLE",
];

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
#[path = "output_tests.rs"]
mod tests;
