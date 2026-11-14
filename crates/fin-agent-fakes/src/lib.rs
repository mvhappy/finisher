//! Fake agent CLIs for tests. Dev only; never shipped.
//!
//! # `fake-claude`
//!
//! A stand-in for the Claude Code CLI (`claude -p --input-format stream-json
//! --output-format stream-json ...`), shaped after the lines 2.1.321 wrote in plan 7D's Task 0.
//! It parses the flags the real CLI would (an unknown flag or a bad value exits 1 with a
//! commander-style error on stderr), answers `--version` and `auth status [--json|--text]`,
//! answers the `initialize` control request before any prompt, emits `system/init` on the
//! first user line, and then plays a script. It reads nothing but its configuration, its
//! script, `--settings`, `--mcp-config` and, for `--resume`, the transcripts under
//! `$CLAUDE_CONFIG_DIR/projects/`.
//!
//! ## Configuration
//!
//! The sidecar file `<directory of the executable>/fake-claude.json`, `{"script", "observed",
//! "version"}` (a relative `script` is relative to that directory), so a test that runs the
//! fake inside a sandbox passes nothing through the environment: it copies the binary into
//! a directory of its own and writes the sidecar there. The variables `FAKE_CLAUDE_SCRIPT`,
//! `FAKE_CLAUDE_OBSERVED` and `FAKE_CLAUDE_VERSION` win over the sidecar (the crate's own
//! tests use them). Without a script the fake answers `initialize`, emits `system/init`, and
//! ends every user turn with a plain `result`. The version defaults to the script's
//! `version`, else `2.1.321`. `CLAUDE_CONFIG_DIR` is where transcripts live
//! (`projects/<cwd>/<uuid>.jsonl`); unset, none is kept and `--resume` finds nothing.
//!
//! ## Observed file
//!
//! `observed` (else the script's `observed`) is an absolute path, or `@config/<name>` for
//! `<$CLAUDE_CONFIG_DIR>/<name>` (writable inside the sandbox). The file is a JSON array with
//! one record per process, appended in start order and never removed (a resume test counts
//! them): `{"id", "pid", "argv", "flags": {"--permission-mode": "manual", "--verbose": true,
//! "--add-dir": [..], ...}, "envNames": [sorted names, never values], "settings":
//! "applied"|"ignored"|"absent", "resume": {"found", "transcriptLines"}, "controlRequests":
//! [{"requestId", "subtype", "request"}], "controlResponses": [{"requestId", "subtype",
//! "raw"}] (every answer the host sent, the line byte for byte), "answers": [{"tool",
//! "requestId", "via": "stdio"|"mcp", "answer", "raw"}], "cancelled": [requestId],
//! "spawned": [{"argv", "code"}], "mcpCalls": [{"tool", "requestId", "toolUseId", "result",
//! "error"}], "refused": "<reason>"}`.
//!
//! ## Script
//!
//! `{"version", "model", "auth", "initialize", "init", "observed", "onInterrupt",
//! "onSigint", "mcpTimeoutMs", "refuseFlagValues", "steps": [...]}`:
//!
//! - `auth` is merged over what `auth status` prints (signed in by default; with
//!   `"loggedIn": false`, or `CLAUDE_SECURESTORAGE_CONFIG_DIR` naming no folder, the CLI's
//!   signed-out fields, exit 1). Signed in, `email`/`orgId`/
//!   `orgName` come from `$CLAUDE_CONFIG_DIR/.claude.json`'s `oauthAccount` (`emailAddress`,
//!   `organizationUuid`, `organizationName`; the sign-in home's, Decision 19), else `null`.
//! - `initialize` and `init` are shallow-merged over the `initialize` response and the
//!   `system/init` line; a value of `"$remove"` deletes that key. Defaults copy the real
//!   CLI: five built-in `agents`, `capabilities`, the five models (`--model` resolves through
//!   them, `default` → `claude-opus-5-5`), `memory_paths` absent (present only when
//!   `--settings` turns `autoMemoryEnabled` on and `CLAUDE_CODE_DISABLE_AUTO_MEMORY` is not
//!   `1`), `output_style` from `--settings`' `outputStyle`, `tools` from `--tools` (the
//!   Linux default set; `--restricted` drops the command tools) less `--disallowedTools`
//!   (bare names and trailing `*`), plus each connected SDK server's `mcp__<server>__<tool>`.
//!   A `--settings` file in which a known key has the wrong type is ignored whole.
//! - `onInterrupt` / `onSigint`: `"result"` (default: the open turn ends with an
//!   `error_during_execution` result and the process stays), `"ignore"`, or `"exit"` (that
//!   result, then exit 0; 130 for SIGINT). An interrupt with no turn open is answered and
//!   writes no result.
//! - `mcpTimeoutMs` (default 1000): how long the first turn waits for the SDK servers.
//! - `initializeSilent`: `initialize` gets no answer (the SDK handshake still starts).
//! - `interruptStillQueued`: the `still_queued` list of the `interrupt` answer (default `[]`,
//!   as 2.1.321 answered).
//! - `refuseFlagValues`: `{"<flag>": [values]}` this version does not know (exit 1), e.g.
//!   `--permission-prompt-tool stdio` for a version that dropped it.
//!
//! **SDK MCP servers.** For each `--mcp-config` server of `type: "sdk"` the fake sends the
//! `mcp_message` `initialize` before it answers the host's `initialize` (or at the first turn
//! when none came), then, once that is answered, `notifications/initialized` and
//! `tools/list`. A server not through by the first turn's timeout is `failed` in
//! `init.mcp_servers` and its tools are absent.
//!
//! **Turns.** A turn starts at a user line (`system/init` before every one, then
//! `system/status {status: "requesting"}`) and ends with a `result`; one whose steps wrote
//! none gets a plain `success` result before the next `expect: "user"` or when the steps run
//! out. User lines received mid-turn are queued (steers) and all played as the one next turn
//! (as 2.1.321 does: the running turn's `result` has `queued_turn_count: 0`). Then the fake
//! idles: each user line (or lines queued together) gets a plain turn.
//! **Stdin EOF** mid-turn lets the turn play to its `result` (a pending `ask` counts as
//! denied), then the fake exits: 1 when the last result had `is_error`, else 0 (also 0 after
//! `initialize` only). The system lines `ui_invalidate` (at start) and `status` are written
//! as the CLI does. An unknown `--resume` writes `result {subtype: "error_during_execution",
//! is_error: true, num_turns: 0}` on stdout, the CLI's message on stderr, and exits 1.
//!
//! A step runs its fields in this order (all optional):
//!
//! 1. `expect: "user" | "control"` (with `subtype` for control): wait for a user line (a new
//!    turn), or a control request of that subtype (answered with an empty success).
//!    With `expect: "user"`, `init {…}` is merged into the script's `init` from the turn it starts
//!    on, e.g. a tool set that changes mid-instance.
//! 2. `sleep` (ms), still serving control requests and queueing user lines.
//! 3. `writeFile {path, contents?}`; `spawn {argv}` (waits for it; the exit code is recorded).
//! 4. `emit [lines]`: JSON values written as lines (`"$session_id"` is replaced; a string
//!    entry is written verbatim); `unknownLine` (a line of a type no CLI has).
//! 5. `ask {tool, input, cancelAfterMs?}`: a `can_use_tool` control request, or with
//!    `--permission-prompt-tool mcp__<server>__<tool>` an `mcp_message` `tools/call` of that
//!    tool (the answer's text content is the `{behavior, ...}` JSON). Then `onAllow` or
//!    `onDeny` (an `error` answer is a deny): a list of lines, or `{writeFile?, spawn?,
//!    emit}` (so a `spawn` runs only when allowed). Unanswered after `cancelAfterMs`, the
//!    request is withdrawn with `control_cancel_request` and neither branch runs.
//!    `toolUseId` names the call (default `toolu_fake_<n>`). `bash: true` plays the CLI's `Bash`
//!    tool as 2.1.321 does: an `assistant` `tool_use` before the prompt (whose fields are the
//!    CLI's Bash request's); once answered, an allowed `input.command` runs with `/bin/sh -c`
//!    and the call's `tool_result` is written (an error one when denied), recorded under
//!    `spawned`. `input.run_in_background: true` is refused before any prompt under
//!    `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`; without it the process is not waited for.
//!    `mcp: true` plays a tool of an SDK MCP server (`tool` = `mcp__<server>__<tool>`, e.g.
//!    Finisher's brokered `mcp__finisher__shell`, plan 7D Task 13): announced like `bash`, refused
//!    before any prompt when no connected server listed it; once allowed, an `mcp_message`
//!    `tools/call {name, arguments: input, _meta: {"claudecode/toolUseId"}}` to that server, and
//!    the call's `tool_result` is the answer's `content` (`is_error` from `isError` or a JSON-RPC
//!    error), recorded under `mcpCalls`. An interrupt while it waits ends the turn and sends
//!    `notifications/cancelled {requestId}`.
//! 6. `notLoggedIn`: the unsigned turn of the CLI (a synthetic `assistant` with
//!    `error: "authentication_failed"`, then `result {subtype: "success", is_error: true,
//!    terminal_reason: "api_error", modelUsage: {}}`).
//!    `cumulativeUsage {inputTokens, outputTokens, cacheReadInputTokens,
//!    cacheCreationInputTokens, costUsd}` (+ `text`): add this turn's delta to the process's
//!    totals (restored from the transcript on `--resume`) and emit a `result` carrying the
//!    cumulative `usage`, `modelUsage` and `total_cost_usd` (`error_max_budget_usd` past
//!    `--max-budget-usd`).
//! 7. `crash {signal: "KILL"|"TERM"|"SEGV"|"INT"|"HUP"}` or `{code}`.
//!
//! After an interrupt ends a turn, its remaining steps are skipped up to the next
//! `expect: "user"`.

pub mod claude;
