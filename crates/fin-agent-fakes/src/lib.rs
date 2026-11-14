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
//!
//! # `fake-codex`
//!
//! A stand-in for `codex app-server --listen stdio://` (codex-cli 0.167.0): newline-delimited
//! JSON-RPC on stdin/stdout, shaped after plan 7E's Task 0 and the 7E Task 4 review's side by
//! side runs of the pinned binary (unsigned), and checked line by line against the committed
//! schema (`vendor/codex/0.167.0/schema/`) by `tests/fake_codex*.rs`. Every behaviour the real
//! binary was not seen doing is marked `UNOBSERVED(7E Task 0 Step 3)` in the source.
//!
//! **Argv** (Decision 4): `app-server --listen stdio:// --strict-config -c k=v ... --enable F
//! ... --disable F ...`, plus `--version` (`codex-cli <version>`). An unknown flag or a missing
//! subcommand is a usage error (stderr, exit 2). An unknown feature (the rows of the committed
//! `features.txt`) exits 1 `Unknown feature flag: F`. A `-c` value of the wrong type for a key
//! the fake types (`codex::config_check`) exits 1 with serde's message and ``in `key` ``, with or
//! without `--strict-config`; under `--strict-config` an unknown `-c` key exits 1 ``unknown
//! configuration field `prefix` in -c/--config override`` (the key up to its first unknown
//! segment), and an unknown key of `$CODEX_HOME/config.toml` exits 1 ``<file>:<line>:<col>:
//! unknown configuration field `k` ``.
//!
//! **Configuration**: the sidecar `<directory of the executable>/fake-codex.json`, `{"script",
//! "observed", "version"}` (a relative `script` is relative to that directory), or the variables
//! `FAKE_CODEX_SCRIPT`, `FAKE_CODEX_OBSERVED`, `FAKE_CODEX_VERSION`, which win. TS tests run
//! `target/debug/fake-codex[.exe]` (built by `cargo build -p fin-agent-fakes`), as for
//! `fake-claude`. `CODEX_HOME` (else `$HOME/.codex`) is where it keeps threads
//! (`sessions/rollout-<id>.json`, so a resume in a new process finds them), reads `config.toml`
//! (the `user` layer) and looks for `auth.json`, `skills/`, `hooks.json`, `AGENTS.md`. Unless
//! `skills.bundled.enabled = false`, it writes the five bundled skills under
//! `$CODEX_HOME/skills/.system/` at start, as 0.167.0 does. Example scripts:
//! `scripts/codex/*.json`.
//!
//! **Methods** (a positive set; anything else is `-32600 "Invalid request: unknown variant
//! `m`, expected one of ..."`, not `-32601`; a line that is not a JSON-RPC message, or longer
//! than 16 MiB, is logged on stderr and never answered): `initialize` (before it `-32600 "Not
//! initialized"`, twice `"Already initialized"`; then an unprompted
//! `remoteControl/status/changed`), `config/read` (layers, highest first, with
//! `includeLayers`: `sessionFlags` from `-c`/`--enable`/`--disable`, `user` from
//! `$CODEX_HOME/config.toml`, `system`; merged table by table, so `-c mcp_servers={}` keeps a
//! user's server; origins per leaf, a table feature's as `features.F.enabled`),
//! `configRequirements/read`, `skills/list` (planted `SKILL.md`s under the cwd's and each
//! parent's `.agents/skills` up to the project root (`project_root_markers`, `.git`),
//! `$HOME/.agents/skills`, `$CODEX_HOME/skills`, then the bundled `system` ones), `hooks/list`
//! (`$CODEX_HOME/hooks.json`, unless the `hooks` feature is off), `plugin/installed`,
//! `mcpServerStatus/list` (the effective `mcp_servers`; the fake runs none, so an enabled one
//! failed to start), `app/installed` (empty), `experimentalFeature/list`, `account/read`
//! (signed out without `$CODEX_HOME/auth.json`; signed in from `fake-account.json` beside it,
//! else an `account` object in `auth.json`, else the script's `account`, else a ChatGPT Plus
//! person), `model/list` (paged by `limit` and `cursor`; `invalid cursor: X`, `cursor N exceeds
//! total models M`), `thread/start` (`instructionSources` lists `$CODEX_HOME/AGENTS.md` or its
//! override, and the cwd's while `project_doc_max_bytes > 0`; unset settings come from the
//! effective config; `dynamicTools` and `multiAgentMode` need the `experimentalApi` capability;
//! then `thread/started` and each enabled MCP server's `starting`/`failed`), `thread/resume`
//! (`no rollout found for thread id X` when unknown or ephemeral), `thread/read` (a loaded
//! thread; a stored one `notLoaded`; else `thread not loaded: X`), `thread/revert` (`thread not
//! found: X` unless loaded in this process, `turn not found: X`; drops `beforeTurnId` and every
//! later turn; a running turn is interrupted first and the answer follows its `turn/completed`;
//! the answer's `turns` is empty, then `thread/reverted`), `turn/start` (during a turn of the
//! same thread: the input joins that turn, which is the answer), `turn/steer`,
//! `turn/interrupt` (`no active turn to interrupt`, `expected active turn id X but found Y`).
//! Params are checked as 0.167.0's serde types check them (`codex::params`: `Invalid request:
//! unknown variant ...`, `missing field ...`, `invalid type ...`; ids are UUIDs, `invalid
//! thread id: ...`); `thread not found: X` for a thread not loaded. EOF on stdin exits 0 at once.
//!
//! ## Script
//!
//! `{"version", "observed", "account", "models", "requirements", "plugins", "apps",
//! "mcpServers" (planted `plugin/installed` marketplaces, `app/installed` apps and
//! `mcpServerStatus/list` data), "refuseDynamicTools" (a pin without them: `thread/start` with
//! `dynamicTools` is `-32600 unknown field`), "signedInElsewhere" (`account/read` signed in with
//! no `auth.json`), "steer": "join"|"refuse",
//! "onInterrupt": "completed"|"ignore"|"exit", "resumeRestoresDynamicTools" (default true),
//! "revertFails", "turns": [{"steer"?, "onInterrupt"?, "steps": [...]}]}`. **The nth turn
//! started on a thread plays `turns[n]`**: the count is kept with the thread (and saved when the
//! turn starts), so after a crash in turn 0 a resume in a new process plays turn 1; past the end
//! (or with no script) a turn is a plain `"ok"` agent message. `observed` may be `@codex/<name>`
//! for `$CODEX_HOME/<name>`. A turn is: response (`turn` `inProgress`, no items, `startedAt`
//! null), `thread/status/changed` active, `turn/started`, the user message item (once in the
//! turn's items), the steps, then `thread/status/changed` idle and `turn/completed` (no items,
//! `itemsView: "notLoaded"`; `thread/read` has them) or, failed, `thread/status/changed`
//! `systemError`, `error {willRetry: false}` and `turn/completed`. The turn is saved with the
//! thread as it starts; one a crash cut short reads back `interrupted`. A step runs these
//! fields in order (all optional):
//!
//! 1. `expect: "steer"|"interrupt"`: wait for a `turn/steer` (or a joining `turn/start`;
//!    consumed one by one) or a `turn/interrupt` (the wait ends the turn `interrupted` unless
//!    `onInterrupt` is `ignore`).
//! 2. `sleep` (ms), still serving requests.
//! 3. `emit [{method, params}]` (`"$threadId"`/`"$turnId"` replaced), `say "text"`
//!    (`item/started`, `item/agentMessage/delta`, `item/completed`).
//! 4. `usage {last, total?, modelContextWindow?}`: `thread/tokenUsage/updated`; counts default
//!    to 0, `totalTokens` to input + output, `total` to the thread's running total plus `last`
//!    (kept in the thread, so a resume carries on).
//! 5. `unknownNotification` (`true` or a method name), `accountChange {type, email,
//!    planType}` (writes `fake-account.json`, sends `account/updated`).
//! 6. `dynamicTool {tool, arguments, callId?, onSuccess?, onFailure?}`: `item/started`
//!    `dynamicToolCall`, the server request `item/tool/call`, the wait for its answer
//!    (`{contentItems, success}`), `serverRequest/resolved`, `item/completed` (`failed` unless
//!    `success`). A tool the thread did not declare in `dynamicTools` (or lost on a resume with
//!    `resumeRestoresDynamicTools: false`) is skipped, as the model could not call it, and
//!    recorded under `skippedTools`.
//! 7. `patch {changes, itemId?, onAccept?, onDecline?}`: `item/started` `fileChange`, the
//!    server request `item/fileChange/requestApproval {itemId, ...}` (the changes are in the
//!    item, as on the real server), the wait, `serverRequest/resolved`, `item/completed`.
//!    `accept`/`acceptForSession` write the changes to disk (`codex::patch_fs`: `add` writes
//!    `diff` as the text, `delete` removes, `update` applies the unified diff, then `move_path`;
//!    relative to the thread's cwd; all or nothing) and complete the item, `failed` when a hunk
//!    does not apply (recorded under `patchErrors`); `decline` and `cancel` write nothing and
//!    decline it; `cancel` also ends the turn `interrupted`.
//! 8. `notLoggedIn`: the capture's unsigned turn: `error {willRetry}` "Reconnecting... 2/5" to
//!    "5/5" over the WebSocket (`responseStreamDisconnected` 401), the warning "Falling back
//!    from WebSockets to HTTPS transport. ...", "1/5" to "5/5" over HTTPS, then the turn fails
//!    `httpConnectionFailed` 401 (without the 14 s). `fail {codexErrorInfo, message?,
//!    additionalDetails?, retries?}`: `retries` retry `error`s, then a failed turn.
//!    `crash {signal: KILL|TERM|SEGV|INT|HUP}` or `{code}`.
//!
//! **A server request open when the turn is interrupted** (`turn/interrupt`, a revert) is
//! withdrawn: `serverRequest/resolved`, its item completed (`failed` tool call with `success:
//! false`, `declined` patch), the turn `interrupted`; an answer that comes later is recorded
//! with `late: true`. An answer that does not match the schema's response type (`contentItems`
//! and `success`; a known `decision`) is recorded under `badAnswers` and counts as an error.
//!
//! `turn/steer`: `join` writes the text as a user message item of the active turn and answers
//! `{turnId}`; `refuse` answers `-32600` with `data.codexErrorInfo.activeTurnNotSteerable`; a
//! stale `expectedTurnId` and no active turn are `-32600` errors.
//!
//! ## Observed file
//!
//! As `fake-claude`: a JSON array with one record per process, appended in start order and
//! never removed (updates take an OS lock on `<file>.lock`; a failed update is said on stderr
//! and carried by the next): `{"id", "pid", "argv", "flags": {"strictConfig", "listen",
//! "config": {key: "raw TOML text"}, "enable": [...], "disable": [...], "version"}, "envNames":
//! [sorted names, never values], "clientInfo", "capabilities", "received": [{"method", "id"?,
//! "params"}] (every message the client sent, notifications included, before any check),
//! "answers": [{"id", "method", "result"|"error", "late"?}] (the client's answers to server
//! requests), "badAnswers": [{"id", "method", "reason", "raw"}], "patchErrors": [{"itemId",
//! "error"}], "steers": [{"turnId", "input", "joined", "via"?}], "skippedTools": [...], "exit":
//! {"code"|"signal", "reason": "eof"|"crash"|"interrupted"}}`.

pub mod claude;
pub mod codex;
