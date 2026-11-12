//! Fake agent CLIs for tests. Dev only; never shipped.
//!
//! # `fake-claude`
//!
//! A stand-in for the Claude Code CLI (`claude -p --input-format stream-json
//! --output-format stream-json ...`). It parses the flags the real CLI would (an unknown
//! flag or a bad value exits 1 with a commander-style error on stderr), answers `--version`
//! and `auth status [--json]`, answers the `initialize` control request before any prompt,
//! emits `system/init` on the first user line, and then plays a script. It reads nothing but
//! its script and, for `--resume`, the transcripts under `$CLAUDE_CONFIG_DIR/projects/`.
//!
//! ## Environment (all optional)
//!
//! - `FAKE_CLAUDE_SCRIPT`: path of the script (JSON). Without it the fake answers
//!   `initialize`, emits `system/init`, and ends every user turn with a plain `result`.
//!   *Tests that sandbox the adapter must admit this name (and the two below) through
//!   `assertSessionEnv`'s `extra`.*
//! - `FAKE_CLAUDE_OBSERVED`: path of the file the fake writes (rewritten on every change);
//!   else the script's `observed` field; else nothing is written.
//! - `FAKE_CLAUDE_VERSION`: the version for `--version` and `initialize`
//!   (else the script's `version`, else `2.1.321`).
//! - `CLAUDE_CONFIG_DIR`: where transcripts live (`projects/<cwd>/<uuid>.jsonl`). Unset: no
//!   transcript is kept and `--resume` finds nothing.
//!
//! ## Observed file
//!
//! `{"argv": [...], "flags": {"--permission-mode": "manual", "--verbose": true, ...},
//! "envNames": [sorted names, never values], "resume": {"found", "transcriptLines"},
//! "answers": [{"tool", "answer"}], "spawned": [{"argv", "code"}], "refused": "<reason>"}`.
//!
//! ## Script
//!
//! `{"version", "model", "auth": {...}, "initialize": {...}, "init": {...}, "observed",
//! "steps": [...]}`: `auth` is printed by `auth status --json` (default
//! `{loggedIn, authMethod, email}`; exit 1 when `loggedIn` is false); `initialize` and `init`
//! are shallow-merged over the `initialize` response and the `system/init` line. A step runs
//! its fields in this order (all optional):
//!
//! 1. `exitOn: "interrupt"`: from now on an interrupt (control request or SIGINT) answers,
//!    emits its `result`, and exits 0.
//! 2. `expect: "user" | "control"` (with `subtype` for control): wait for a user line, or a
//!    control request of that subtype (answered with an empty success).
//! 3. `sleep` (ms), still serving control requests and queueing user lines (steers).
//! 4. `writeFile {path, contents?}`; `spawn {argv}` (waits for it; the exit code is recorded).
//! 5. `emit [lines]`: JSON values written as lines (`"$session_id"` is replaced; a string
//!    entry is written verbatim); `unknownLine` (a line of a type no CLI has).
//! 6. `ask {tool, input}`: send a `can_use_tool` control request and wait for the answer, then
//!    emit `onAllow` or `onDeny` (an `error` answer counts as a deny).
//! 7. `notLoggedIn`: the `result` of an unsigned-in first turn.
//!    `cumulativeUsage {inputTokens, outputTokens, cacheReadInputTokens,
//!    cacheCreationInputTokens, costUsd}` (+ `text`): add this turn's delta to the process's
//!    totals (restored from the transcript on `--resume`) and emit a `result` carrying the
//!    cumulative `usage`, `modelUsage` and `total_cost_usd`.
//! 8. `crash {signal: "KILL"|"TERM"|"SEGV"|"INT"|"HUP"}` or `{code}`.
//!
//! An interrupt mid-script emits the `control_response`, then a `result`
//! (`error_during_execution`, `is_error: true`) and skips the remaining steps up to the next
//! `expect: "user"`. When the steps run out the fake idles: each user line gets a plain
//! `result`; EOF on stdin exits 0.

pub mod claude;
