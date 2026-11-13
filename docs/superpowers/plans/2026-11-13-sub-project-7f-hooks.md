# Sub-project 7F: Hooks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the reply chain and its failure rules, trust, the sandboxed hook process, the hook credential, the chain's policy stage, the auto-merge point, the plugin layer), `sonnet` for plumbing (tracker, gates, console text, e2e, docs). Task 13 is controller-run (it needs the macOS and Windows hosts).

**Goal:** hooks run. After 7F:
- the events of arch §10, **except `PreOp`/`PostOp`** (cut from v1, spec §9.1), are fired at the points spec §9.1 names: `SessionStart`, `SessionEnd`, `UserPrompt`, `PreCommand`, `PostCommand`, `BranchReady`, `PreMerge`, `PostMerge`, `JobDone`, `Approval`, `Notification` (`PreRender`/`PostRender` are accepted in configuration and inert until a render job kind exists, Decision 17);
- a hook answers **`allow | block {reason} | context {text}`** (no `modify`, spec §9.1); the first `block` stops the chain, `context` replies concatenate, in the order managed, user, project, local, plugin, then file order;
- **command hooks** run through `fin-sandbox`'s **`hook` profile** (built in 7C Task 5: `crates/fin-sandbox/src/profile/hook.rs`, `HookInput` in `src/inputs.rs:62`) with the cleared environment, no network, the project read-only, a private temp, a timeout (10 s default, 60 s at most, `onTimeout: allow|block`, default `allow` except `PreMerge`) and, only when declared, a `plan`-scoped credential of their own for `fin`;
- **project hooks** (`finisher.hooks.json`) stay off until a person trusts them; trust records `project_trust.hooksHash` (reserved since agent migration 0004, unused until now), and a changed file is off again until re-trusted;
- a block is audited (`hook.block`, reserved in `packages/contracts/src/audit.ts:55`), answered `HOOK_BLOCKED {hook, reason}` (exit 7, already in `crates/fin/src/output_codes.rs:58`), and a timeout is a visible `HOOK_TIMEOUT` notice (`output_codes.rs:555`);
- **7B's hook points are wired:** the `auto` merge trigger's `hookBlocked` refusal (`agent/auto-merge.ts:128`, `PreMergeHook`) and `branchReady` (`ReviewLink.onBranchReady`, `agent/review-link.ts`) get `BranchReady` and `PreMerge` hooks; the 80 % budget notice becomes a `Notification`;
- **plugin hook handlers** (sp8b plan Task 17) run as `plugin.hook` v1 jobs in the `plugin` layer, gated by `hooks:observe`; `hooks` becomes consumable at the next plugin host API minor.

**What exists that this plan starts from** (integration branch `sp4c-edit-breadth`, read 2026-11-13; 7A, 7B, 7C (but seccomp and Task 16), 7D-1 and 8B (but Tasks 16, 17, 19) built):
- **The `hook` profile and launcher** (7C Task 5): `fin_sandbox::profile::hook(&HookInput)` (project read-only, the hook's own file, toolchains, `tmp` the only write root, `Net::None`, or with `fin` declared `Net::Loopback {ports: [listener]}` on Linux, `fin.sock` on macOS, the pipe on Windows; headless devices; `Limits.wall_ms` from `timeout_ms`); `fin-sandbox-exec --policy-fd 3` reads a `LaunchRequest {profile: agent|hook, input}`; a hook without network gets an empty network namespace (`linux/namespace/layout.rs:111`). Linux escape cases for the hook already exist (`crates/fin-sandbox/tests/escape_agent_linux.rs`: "the hook: project read-only, no IP unless `fin` is declared") and the common suite runs `hook` (`tests/escape_common.rs`; Linux table "hook 22/1"). Windows: `fin_sandbox::windows::grants::hook` and fin-node `sandboxHookGrants` (`crates/fin-node/src/sandbox_acl.rs:190`); `grantTable` (`apps/finisherd/src/sandbox/broker.ts:82`) already names it; **not wired** ("nothing in `finisherd` launches a hook yet", TASKS 7C Task 11 Windows; owner `call:<id>`). fin-node has `sandboxAgentCheck` but **no `sandboxHookCheck`** (`crates/fin-node/src/sandbox.rs`). `server.status` already shows the `hook` profile's warning (`server-admin.ts:117`). `fin_sandbox::status::READY` is empty, so the `hook` profile runs `direct` (unconfined, env allow-list, `SANDBOX_DISABLED {profileNotReady}`) on every OS until 7C Task 16.
- **The launcher start is private to the session sandbox:** `start(via, argv, cwd, env, platform, onExit)` and `killAll` in `apps/finisherd/src/agent/sandbox.ts:343` (471 lines), hard-coding `profile: "agent"` in its errors; `scripts/check-agent-spawn.mjs` allows process starts under `agent/` only in `sandbox.ts`, `builtin/fin-tool.ts`, `builtin/environment.ts`.
- **Trust** (7B): `agent/trust.ts` (`readRules` cache by `(path, ino, size, mtime, ctime)`, `RACY_MS`, `hashOf` = `sha256:<hex>`, `NO_FILE` = `none`, `RULES_FILE_MAX` 256 KiB, `projectRules`, `trustProject`), `agent/rule-store.ts` (`ProjectTrust {projectId, rulesHash, hooksHash?, promptsHash?, trustedAt, by}`; `trust`, `trustPrompts`; nothing writes `hooksHash`), `agent/rules-rpc.ts:139` `agent.project.trust {projectId, expectedHash, expectedPromptsHash?}` (prompts checked first so a refusal changes nothing; audited with `rulesHash`/`promptsHash`), the console `finisherd project trust` (`agent/console-rules.ts`), audit `RULE_FIELDS` (`rulesHash`, `promptsHash`).
- **The chain** (`chain/chain.ts`): stage 5 `PolicyRule`s (`chain/policy.ts`; 7B's `commandRules` appended in `handlers.ts:182`), stage 7 `ChainHooks.after` (`chain/hooks.ts`; wrapped by `withOutcomes` then `withReviewLink`, `handlers.ts:178-179`). `commandsOf`/`commandOf` (`agent/command-of.ts:193,205`) name an RPC as `fin` commands; `globMatch` (`agent/glob.ts:44`) matches rule patterns.
- **Runtime points:** `Runtime.start` (`agent/runtime.ts:216`), `promptIn` (`:324`, the one path of `prompt`, `start {prompt}` and saved prompts), `steer` (`:379`), `after` (`:427`, journal phase `ended` → `reviewLink.sessionEnded`), `approvalOpened` (`:539`); the 80 % budget notice `notice {budget, used, limit, at: 0.8}` (`agent/runtime-engine.ts:487`); `publishStatus` (`runtime-engine.ts:188`) feeds `WatchHub.publish` (`agent/watch-hub.ts`). `RunTrigger` and `SessionTrigger` already list `hook` (reserved).
- **Review and merge points:** `autoMergeTrigger` with `preMerge?: PreMergeHook = (ev, session) => Effect<boolean>` (none wired: allows; tests use it as a latch, TASKS 7B Task 12 S3); `main.ts:421` subscribes it; `history.merge` (`handlers.ts:413`) and `review.merge` (`reviews/rpc.ts:62`).
- **Jobs:** `JobEngine.onDone(listener: (job: JobView) => void)` (`jobs/api.ts:68`) is 5C's bus; no render job kind exists (`where.minutesOver` is inert for the same reason, TASKS 7B follow-ups).
- **Plugins** (8B): `HookContribution {event: String, handler: String}` typed, refused `PLUGIN_CONTRIBUTION_UNSUPPORTED` by presence (`crates/fin-plugin-api/src/contribution.rs:135`, `HOST_API = (1, 1, 0)` at `:10`; finisherd `PLUGIN_HOST_API = "1.1.0"`, `plugins/host/processes.ts:49`); permissions `hooks:observe`/`hooks:modify` parse and validate (`validate/permissions.rs`); no `protocol/hook.rs`; the SDKs announce only `plugin.command` v1 (`packages/plugin-sdk/src/runtime.ts:78`).
- **Codes and audit:** `HOOK_BLOCKED` (exit 7) and `HOOK_TIMEOUT` (exit 5) in `output_codes.rs` and `console/exit-codes.ts`; audit kinds `hook.block` and `hook.modify` reserved, with no details schema (`detailsSchema`, `audit.ts`).
- **Versions:** fin-core `SCHEMA_VERSION` is 4 (`crates/fin-core/src/model/mod.rs:38`); the `agent` control chain is at migration 0012. **7F changes neither** (Decision 22).

**Architecture:**
- **`crates/fin-plugin-api/src/protocol/hook.rs`** (new): `HookEvent`, `HookRequest`, `HookReply`, `HookRun` (the `plugin.hook` job's params) and `HookOut`, one source for command hooks and plugin handlers, generated into `@finisher/core-types`.
- **`packages/contracts/src/hooks.ts`** (new): the configuration schema (`HookSpec`), listing types for `agent.hooks.list`, the trust fields.
- **`apps/finisherd/src/hooks/`** (new; every file ≤ 600 lines, most ≤ 300): `config.ts` (layers, decode, trust status), `process.ts` (one command hook through the launcher), `engine.ts` (dispatch: order, chain, timeouts, failures, the observe queue, stats), `credential.ts` (the `fin` credential), `session.ts` (session events and context delivery), `chain.ts` (`PreCommand`, `PostCommand`, `PreMerge`/`PostMerge` on RPCs), `review.ts` (`BranchReady`, the auto-merge point), `jobs.ts` (`JobDone`), `rpc.ts` (`agent.hooks.list`), `wiring.ts` (`buildHooks`).
- **`apps/finisherd/src/sandbox/launch.ts`** (new): the launcher start and group kill moved out of `agent/sandbox.ts`, profile-parametrised.
- **Plugins:** `apps/finisherd/src/plugins/host/hooks.ts`, `apps/finisherd/src/jobs/kinds/plugin.hook.ts`; SDK hook handlers in `crates/fin-plugin-sdk` and `packages/plugin-sdk`.

**Tech Stack:** as 7A–7C and 8B: Node 24, Effect 4.0.6, vitest, fast-check; Rust 2024, `serde_json`, `schemars`. **No new dependency** (crate or npm), so no licence or `check:dep-age` step.

**Spec:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1.4 (7F row), §1.6, §6.5, §8.9, §8.10 (`hook` column), §9 (all), §15 (`HOOK_BLOCKED`, `HOOK_TIMEOUT`), §25 (7C as built), §26 (7B as built); arch §10; sub-project 8 spec §3.6 (the hook row), §4.3 (risk), §12, §24.4 ("For 7F"). Plans: 7B (`2026-11-08-sub-project-7b-subagents-rules-approvals.md`, Task 12), 7C (`2026-11-08-sub-project-7c-os-sandbox.md`, Tasks 5, 10, 11), 8B (`2026-11-08-sub-project-8b-plugin-runtime.md`, Decision 20, Task 17).

**Branch:** `sp7f-hooks` from the integration branch (`sp4c-edit-breadth`). Batch D needs nothing beyond 8B as merged.

**What this plan closes in `TASKS.md`:**

| Tracker line | Where |
|---|---|
| "Follow-ups from 7B": **7F: `PreMerge` and `BranchReady` hooks** (the trigger's `hookBlocked` point; `branchReady` the event to attach to) | Task 8 |
| same line: **`hooksHash` in `project_trust`** | Task 2 |
| "Follow-ups for 7B and 7C (from 7A)": **the 80 % `Notification` hook (7F)** | Task 6 |
| 7C Task 11 (Windows): **Hook grants: not wired** (`sandboxHookGrants`, `grantTable`, owner `call:<id>`) | Task 3 |
| 8B **Task 17: hook handlers (gated on 7F)**; 8B summary "*For 7F (Task 17):* `plugin.hook` v1, `hooks` consumable at host API 1.2" | Tasks 9, 10 |
| Sub-project 7 line "7F hooks" | this plan; ticked by Task 12 |

**Out of scope, with the reason:**
- `PreOp`/`PostOp` and **`modify` replies**, and therefore `hooks:modify` taking effect and the `hook.modify` audit kind (spec §9.1, §1.6: cut from v1). The 8B plan's Task 17 tests `modify_with_grant_rewrites_payload_and_audits_both` are replaced (Decision 19).
- **Firing `PreRender`/`PostRender`:** no render job kind exists until 6E; the events parse and are listed `inert` (Decision 17).
- **Hooks that start sessions or runs** (`SessionTrigger`/`RunTrigger` `hook` stay reserved; no event reply can start work).
- **A hook UI** (sub-project 9); **the critic gate** beyond `BranchReady`'s block reaching the `auto` merge trigger (sub-project 10 reads the recorded gate); **network for command hooks** (spec §8.10: none but `fin`'s listener; Owner question 3).
- **A diagnostics journal kind for hook runs** (no `fin-core` event kind; runs show as session `notice` nodes, `hook.block` audit rows and `server.status` counters, Decision 21).
- **Seccomp, `READY` and the macOS/Windows escape runs** (7C Tasks 4, 16): the hook profile is applied where 7C says it is ready; until then hooks run `direct` with the warning, like agents.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL dependency in shipped code (`pnpm gates`). This plan adds **no** crate or npm package.
- **No cloud-only imports.** **Primitive ops live only in `fin-core`: 7F adds none** and no `fin-core` model or event type, so `SCHEMA_VERSION` (4) does not change. **TypeScript model types are generated, never hand-written:** the hook protocol types are Rust (`fin_plugin_api::protocol::hook`), registered in `crates/fin-plugin-api/src/schema.rs` and generated into `@finisher/core-types` (`pnpm --filter @finisher/core-types generate`; drift gate `scripts/check-generated-types.mjs`). Control-plane schemas (configuration, listing) are Effect Schemas in `@finisher/contracts`, typed against the generated types (`satisfies`).
- **No backward-compatibility machinery** (no users): no upcaster, no migration of old rows, no legacy configuration keys.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500 (`scripts/check-file-size.mjs`). **Near the limit today and touched here:** `apps/finisherd/src/main.ts` **587** (Task 4 first extracts the review wiring, unconditionally), `apps/finisherd/src/jobs/engine.ts` 592 (not touched: `onDone` is enough), `apps/finisherd/src/agent/runtime-engine.ts` **576** (Task 6: +4), `apps/finisherd/src/agent/runtime.ts` **557** (Task 6: +20), `packages/contracts/src/scopes.ts` 562, `packages/contracts/src/rpc.ts` 554, `crates/fin/src/output_codes.rs` 563, `apps/finisherd/src/handlers.ts` 513, `agent/approvals.ts` 507. These take **call-site lines only**; each task that touches one states the budget and, where it would pass 595, first moves code out (named in the task).
- Errors are `{code, message, details, fix?}`; a new code goes into `crates/fin/src/output_codes.rs`, `apps/finisherd/src/errors.ts` (if typed there) and `console/exit-codes.ts` (the parity test in `console.test.ts`).
- **A process starts only through the launcher module:** `sandbox/launch.ts` is the one place `hooks/` reaches `child_process` (the spawn gate is extended, Task 3).

**Testing and process**
- Real engine, real `control.db`, real `finisherd` in RPC tests. Hook programs are one Node script, `apps/finisherd/test/fixtures/hooks/fake-hook.mjs`, driven by its arguments (reply, sleep, crash, garbage, oversize, fork a sleeper, read a path, connect, write a path). It is configured as `run: ["node", "fake-hook.mjs", <args…>]`, `file: "fake-hook.mjs"` (Decision 6: `node` is finisherd's own Node, the script the hook's own file), copied into the test's project folder; the same configuration runs on POSIX and Windows, with no shebang and no shell shim.
- Hooks run `direct` in most tests, and under the real launcher on Linux with `FIN_TEST_SANDBOX_READY=hook` (test-hooks fin-node), as 7C/7D test the `agent` profile.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates` and `rtk pnpm test:gates` (**the four gates**: `pnpm gates` runs the file-size, spawn, generated-types and licence gates but not their tests, and `pnpm test` does not include `scripts/**/*.test.mjs` (`vitest.config.ts:23`); `test:gates` is `node --test "scripts/**/*.test.mjs"`); tasks touching the runtime, RPCs, chain or console also run `rtk pnpm e2e`. Rust tasks also run `rtk cargo clippy --workspace --all-targets -- -D warnings` and `rtk cargo fmt --all --check`.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, user names or key names in tracked files or fixtures.
- **Tracker:** each task ticks its line in `TASKS.md` under a new "Sub-project 7F: hooks" section (Task 1 makes it), in the same commit.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Events and replies
1. **Events and who may block.** `HookEvent` = `SessionStart | SessionEnd | UserPrompt | PreCommand | PostCommand | BranchReady | PreMerge | PostMerge | PreRender | PostRender | JobDone | Approval | Notification`. Four classes (`HookEvent::class()`):
   - **Blocking before** (awaited; a `block` refuses the action): `UserPrompt`, `PreCommand`, `PreMerge`, `BranchReady` (its block gates the `auto` merge trigger, Decision 13) and `PreRender` (inert).
   - **Blocking after:** `PostCommand` (spec §9.1: "block with reason; add context"). The command has already landed, so a block **cannot undo it**: the first block stops the later `PostCommand` handlers, is audited (`hook.block {after: true}`) and its reason reaches the caller as a visible `notice {hook: {event: PostCommand, id, blocked: reason}}` in the session's active run, delivered to the agent like context (Decision 16). It runs off the call's path (stage 7 never fails or delays the answer).
   - **Awaited, non-blocking:** `SessionStart`: `Runtime.start` waits for its chain (bounded by the hooks' timeouts) to collect context for the first run; a `block` there is ignored with a notice.
   - **Observe-only** (detached; a `block` is ignored with a `notice` (`ignoredBlock`) and counted): `SessionEnd`, `PostMerge`, `PostRender`, `JobDone`, `Approval`, `Notification`.
   (Owner question 2.)
2. **Replies** are one JSON object: `{"reply": "allow"}`, `{"reply": "block", "reason": "…"}`, `{"reply": "context", "text": "…"}` (`HookReply`, serde `tag = "reply"`, `deny_unknown_fields`). A command hook's **exit 0 with empty stdout is `allow`**. Anything else that is not one valid reply line (a non-zero exit, a signal, a second JSON value, more than 64 KiB on stdout, invalid UTF-8, `modify`) is a **failure**. `reason` ≤ 1 KiB and `text` ≤ 16 KiB after control characters (but `\n`, `\t`) are stripped; longer is a failure, not truncated.
3. **Failure semantics:** a timeout **and every other failure** follow the hook's `onTimeout` (`allow` | `block`; default `allow`, `block` for `PreMerge`); a failure answered `block` gives `HOOK_BLOCKED {hook, reason: "timeout" | "failed"}`. Every timeout writes a `HOOK_TIMEOUT {hook, event, timeoutS}` notice and every other failure a `notice {hook: {id, failed: <kind>}}` (no new error code); neither is an error of the call when the result is `allow`. (Owner question 1.)
4. **Order and chain** (spec §9.1): managed, user, project, local, plugin; file order within a layer (a plugin layer is ordered by plugin id, then manifest order). Blocking events run **sequentially**; the first `block` (or failure resolved to `block`) stops the chain; `context` texts are joined in order with `\n\n`, at most 64 KiB in all (later ones are dropped with a notice). `PostCommand` and observe-only events run sequentially per event too, but off the caller's path; **their hooks are resolved when the event is taken off the queue, not when it is queued** (Decision 5's check runs then), so a file changed while an event waits applies to it.
5. **Freshness and cost.** **Every dispatch re-validates every layer file's metadata** before any hook runs, as `trust.ts` does at every decision (`readRules`, `trust.ts:105`: open, `fstat`, compare `(path, ino, size, mtime, ctime)`; a file changed within `RACY_MS` is re-read): no time-based cache, so a changed, deleted or replaced `finisher.hooks.json` never runs after the change (spec §9.4), and the trusted hash is compared on each re-read. Parsed layers and the per-project `EventIndex` (`Map<HookEvent, Hook[]>`) are kept by those keys and the enabled plugins' digest. **Cost contract** (refining spec §9.1's "one map lookup", recorded as a deviation in Task 12): a point with no hook for its event costs one `Map.get` **plus one `fstat`/`stat` per configuration location that applies** (managed, user and local always; the project file only while its trust record holds a `hooksHash` other than `none`, since an untrusted file applies to nothing), and never a read, a parse or a process; a refresh (a changed key) reads and parses that file once. Task 4 measures the unchanged and the changed path separately.

### Handlers and the sandbox
6. **Configuration** (`HookSpec`, `packages/contracts/src/hooks.ts`): `{id (slug ≤ 64), event, run: [program, ...args] (1–64 strings, each ≤ 4 KiB), file?: <path>, match?: <command pattern, RuleCommand grammar>, principals?: "agent" | "all" (default "agent"), timeoutS?: 1–60 (default 10), onTimeout?: "allow" | "block", fin?: true}`. **The program and the hook's own file are separate.** `run[0]` is the program, resolved once at load: an absolute path; a path with a separator, relative to the folder of the file that declares it; the bare name `node`, which is finisherd's own Node (`process.execPath`, as the `node` plugin runtime uses it); or another bare name looked up in the toolchain base (`sandboxToolchainBase()`) only, never the user's `PATH`. On Windows the program must be an `.exe` (`.cmd`/`.bat` and other shell-run files are refused: nothing runs through a shell, Decision 9). `file` is the hook's own file (`HookInput.hook_file`, read-only), relative to the declaring folder unless absolute; absent, it is `run[0]` when that is a declaring-folder path, else none (the program's own directory then reaches the policy through `toolchains`). The resolved program (and, for `node`, its install directory) is added to `HookInput.toolchains`. `match` filters by name: the command names of `commandsOf` for `PreCommand`/`PostCommand` (any name matches), the job kind for `JobDone`, `status.<status>` / `budget.<kind>` for `Notification`, the approval kind for `Approval`; it is refused on other events. Unknown keys are refused. At most 64 hooks per layer file.
7. **Where configuration lives** (spec §9.3): managed `<server-data>/managed-settings.json` key `hooks`; user `<server-data>/settings.json` key `hooks`; project `finisher.hooks.json` beside `finisher.project.json` (`{"hooks": [...]}`); local `<server-data>/projects/<id>/hooks.json` (`{"hooks": [...]}`, beside fin-store's `project.fdb`, `crates/fin-store/src/paths.rs:51`); plugin: enabled code plugins' `contributes.hooks`. Hooks are **not** a `settings.write` path (`SETTINGS_PATHS`, `contracts/devices.ts:200`, unchanged): a person edits the files.
8. **Unreadable layers fail closed for blocking events** (as 7B's rules, `trust.ts` `unreadable`): a managed, user or local layer that applies but cannot be read or decoded stands as a synthetic hook `<layer>.unreadable` that blocks every blocking event (`HOOK_BLOCKED {hook: "<layer>.unreadable", reason: "unreadable"}`) and shows in `agent.hooks.list` and `server.status`; no file, or no `hooks` key, is no hooks. An untrusted, changed, deleted or unreadable **project** file simply does not apply (as rules).
9. **The process** (spec §9.2): stdin is one JSON line `HookRequest {event, v: 1, session?, project, payload}` (`session`: `{id, role, mode, harness, rootBranch?}`; `project`: `{id, dir}`), then EOF; the environment is cleared to `PATH` (toolchain base), `HOME`, `TMPDIR`/`TEMP`/`TMP` (the private temp), `LANG=C.UTF-8`, `FIN_HOOK_EVENT`, and, with `fin`, `FIN_CREDENTIAL`/`FIN_SERVER` (Decision 11); cwd is the private temp `<server-data>/hooks/run/<ulid>/` (0700, removed after the call). A payload over 1 MiB is sent as `{payloadOmitted: true, bytes}`. **Argument boundaries are kept**: the argv array goes to `spawn` without a shell (`agent/sandbox.ts:352` already spawns that way), and on Windows the launcher's `CreateProcessW` command line (`crates/fin-sandbox/src/windows/apply.rs:318`) is built by its existing quoting; Task 3 proves an argument with spaces, quotes and backslashes arrives intact in `direct` and AppContainer modes. The launcher gets `LaunchRequest {profile: "hook", input: HookInput}` exactly as the session sandbox does for `agent`; modes follow the `hook` profile's status as `sandboxMode` does for `agent` (`launcher` when enforced or Landlock disabled, `direct` when not ready, refused when unavailable: the hook then **fails**, Decision 3).
10. **Timeouts are enforced by `finisherd`**, not only the launcher: `HookInput.timeout_ms` is set, but only the Windows launcher reads `Limits.wall_ms` (`crates/fin-sandbox/src/windows/launcher.rs:186`); on Linux and macOS `finisherd` hangs the launcher up (`SIGHUP`, then `SIGKILL` of its group after `REAP_GRACE_MS`, as `agent/sandbox.ts` does) at the deadline. **At most 8 hook processes run at once** per server; time waiting for a slot counts against the hook's timeout.
11. **The `fin` credential** (spec §9.2 "a `plan`-scoped agent credential of its own"): a hook with `fin: true` gets an `agent`-kind credential minted through `Credentials.mint` (`auth/credentials.ts:212`) with `MODE_SCOPES.plan` (`agent/credentials.ts:50`), `projects: [the event's project]`, `branches: ["main"]` (no write scope, so branch limits never apply), label `hook <layer>:<id>`, the minimum expiry (`CREDENTIAL_MIN_EXPIRY_S`, renewed on use); **one per (hook, project)**, minted on first use, its file `<server-data>/hooks/creds/<hash>.json` (0600), revoked through 5A's one revoke path when the hook's configuration changes or disappears, at server stop, and at start for any left over (label prefix `hook `). It is not a session: the chain treats it as a sessionless `agent` credential (5A's branch limits; `reach.covers` false). **Calls made with a hook credential never fire hooks** (recursion guard). (Owner question 6.)
12. **Windows:** a launcher-mode hook's table is `sandboxHookGrants(input)`, granted through `grantTable` under owner `call:<ulid>` (7C Task 10's naming) before the start and revoked at the exit and on a failed start; the project container's SID is allowed on the pipe only for `fin` hooks.

### Event sources
13. **`BranchReady` and `PreMerge` on the `auto` path:** the hook bridge subscribes its own `onBranchReady` listener that runs `BranchReady` hooks for every event (context and block become `notice {hook: …}` nodes in the session's active run). The trigger's `preMerge` point becomes `(ev, session, item) => Effect<boolean>` (one added parameter, `agent/auto-merge.ts`): it awaits the **same** `BranchReady` outcome (memoised per event object, so the two listeners share one run) and refuses on its block, then runs `PreMerge` hooks with the item; false is `hookBlocked`. A successful auto merge fires `PostMerge`. Tests that pass their own `preMerge` (`AgentServeOptions.autoMerge`) still override it.
14. **`PreMerge`/`PostMerge` on RPCs:** a stage-5 `PolicyRule` (`hooks/chain.ts`) for `history.merge` (`{from, into}`, `contracts/history.ts`) and `review.merge` (the item read through the reviews workflow) for **every principal** (people's merges are policy too); `PostMerge` from stage 7 on success. The rule runs while the call holds its project's policy gate shared (5E), so a slow `PreMerge` delays policy writes on that project by at most its timeout.
15. **`PreCommand`/`PostCommand`:** a `PolicyRule` appended **after** `commandRules` (so a rule's `deny`/`ask` comes first and a hook never sees a call the rules refuse) for `agent` principals, and for every principal when a hook says `principals: "all"`; `PostCommand` in stage 7 (`ChainHooks.after`, wrapped like `withReviewLink`) for calls that answered, queued off the call's path, with Decision 1's blocking-after rule. Payload `{rpc, command, commands, payload}` (the decoded RPC payload, Decision 9's cap). A block is `HOOK_BLOCKED` (exit 7), audited `hook.block`. A dry run (`isDryRun`) still runs `PreCommand`.
16. **Session events and prompt origin:** `promptIn` gains an explicit origin, `{origin: "start" | "prompt" | "steer" | "brief", context?: string}` (`brief`: a child's first run from its parent, `runtime.ts:269`'s path when `parent` is set). **`UserPrompt` fires exactly once per person's prompt:** for `agent.session.start {prompt}` it runs **before** `openSession` (no session yet, so a block creates nothing) and `promptIn` is then called with `origin: "start"` and the collected context, so it does not fire again; for `prompt`, `steer` and saved prompts on an existing session it fires inside `promptIn`/`steer`; `brief` never fires it. **`SessionStart`** runs in `Runtime.start` right after the open and is **awaited** (Decision 1: awaited, non-blocking) before the first `promptIn`, whose context is the `SessionStart` context then the `UserPrompt` context, in that order; with no prompt it is held. `SessionEnd` from `Runtime.after` on the `ended` phase. **Context delivery:** the person's text stays the `userMessage` node as typed; the hooks' context is written as a `notice {hookContext: {event, hooks: [ids], text}}` node and appended to the provider's input of that run (`\n\n<hook-context>…</hook-context>`), so the trace store records what the model saw. Context from events without a prompt (`PostCommand` context and `PostCommand` block reasons, `PreCommand` allow-with-context, `BranchReady`, `Notification`) is delivered as a steer with `trigger: "hook"` when the session is in a run and its provider steers, else held for its next run; it never starts a run. `SessionStart` context waiting for a first prompt lives in memory only (Owner question 7).
17. **`PreRender`/`PostRender`** parse and are listed with `inert: "noRenderKind"`; nothing fires them until 6E registers a render kind (as 7B left `where.minutesOver` inert).
18. **`JobDone`** from `JobEngine.onDone` (5C's bus): payload `{jobId, kind, kindVersion, state, project?, owner, via?, error?: {code}}`; never for `plugin.hook` jobs (recursion). **`Approval`**, and **`Notification`** for session status changes and the 80 % budget notice, come from an **unconditional tap in the runtime engine, not from `WatchHub`**: `WatchHub.publish` drops items for unwatched sessions (`watch-hub.ts:56`) and `RuntimeEngine.flush` returns early when nobody watches (`runtime-engine.ts:172`), so a budget `notice` node written by `crossings` (`:487`) never reaches the hub unwatched. `RuntimeEngine` gets an optional `sessionEvents?: SessionEventTap` (from `Deps`) called (a) in `publish()` (`:184`) for every `statusChanged` and `approvalOpened` item, which is called whether or not anyone watches, and (b) in `crossings()` once per budget notice, beside its `append`. Each event is handed over once, at its source, so there is no cursor: attaching or detaching an `agent.watch` subscriber changes nothing. The observe queue holds at most 256 pending events per server; overflow drops the newest with a counted `dropped` (in `server.status`).

### Plugins (8B Task 17)
19. **No `modify` (spec §9.1 wins over the 8B plan's Task 17 and sp8 §3.6's table row).** `HookReply` has no `modify` variant, so a plugin answering it fails to decode: a failure under Decision 3. `hooks:modify` stays a valid manifest permission (8A validates it; high risk at install, sp8 §4.3) and **grants nothing in v1**; `hook.modify` stays reserved. (Owner question 5.)
20. **Plugin handlers:** a `plugin.hook` v1 job (family `plugin`, warm, interactive, `maxAttempts: 1`, never submittable by RPC, as `plugin.command`) on the subscriber's process key, rights = the event's principal (or `systemInvoker` for system events such as `JobDone` of a system job) ∩ the plugin's grants (`plugins/host/rights.ts`); a plugin receives an event only with `hooks:observe` naming it; the event must be in a project the invoker reaches. Timeout: the manifest's `timeoutS` (new optional field, 1–60, default 10); `onTimeout` is the event's default (a plugin cannot loosen `PreMerge`). `hooks` becomes consumable at the **next plugin host API minor** (`1.2.0` if 7F lands before 8C bumps it for `job.submit`; whoever lands second takes the following minor, and says so in its commit).

### Versions
21. **No diagnostics kind for hook runs** (no `fin-core` events change). Evidence lives in session `notice` nodes, `hook.block` audit rows and `server.status.hooks {runs, blocks, timeouts, failures, dropped}` per event since start.
22. **No control-database migration and no model bump:** `project_trust.hooksHash` already exists (agent migration 0004); configuration is files; credentials use 5A's `credentials` table. `SCHEMA_VERSION` stays 4. If review adds a stored table after all, it takes the next `agent` migration number at the time it lands (0013 today; whoever lands first takes it).

## Frozen interfaces

```rust
// crates/fin-plugin-api/src/protocol/hook.rs
pub enum HookEvent { SessionStart, SessionEnd, UserPrompt, PreCommand, PostCommand, BranchReady, PreMerge, PostMerge, PreRender, PostRender, JobDone, Approval, Notification }
pub enum HookClass { BlockingBefore, BlockingAfter /* PostCommand */, Awaited /* SessionStart */, Observe }
impl HookEvent { pub fn class(self) -> HookClass; pub fn default_on_timeout_blocks(self) -> bool /* PreMerge */; pub const ALL: [HookEvent; 13]; }
#[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
pub enum HookReply { Allow, Block { reason: String }, Context { text: String } }
pub struct HookRequest { pub event: HookEvent, pub v: u32 /* 1 */, pub session: Option<HookSession>, pub project: HookProject, pub payload: serde_json::Value }
pub struct HookRun { pub handler: String, pub request: HookRequest }   // plugin.hook v1 params
pub type HookOut = HookReply;
```

```ts
// packages/contracts/src/hooks.ts
export const HookSpec: Schema<{ id; event: HookEvent; run: string[]; file?: string; match?; principals?: "agent" | "all"; timeoutS?; onTimeout?: "allow" | "block"; fin?: true }>;
export type HookLayer = "managed" | "user" | "project" | "local" | "plugin";
export interface ListedHook { layer: HookLayer; source: string; id: string; event: HookEvent; applies: boolean; inert?: "noRenderKind"; plugin?: string }
// apps/finisherd/src/hooks/engine.ts
export interface HookOutcome { blocked?: { hook: string; reason: string }; context?: string; ran: number }
export interface HookEngine {
  fire(event: HookEvent, at: HookAt, payload: unknown): Effect<HookOutcome>;   // blocking events: awaited by the caller
  observe(event: HookEvent, at: HookAt, payload: unknown): void;                // PostCommand and observe-only: queued; hooks resolved when it runs
  stats(): HookStats;
}
export interface HookAt { projectId: string; session?: Session; principal: Principal }
// apps/finisherd/src/hooks/process.ts
export function runCommandHook(h: ResolvedHook, req: HookRequest, deps: HookProcessDeps): Effect<HookReply | HookFailure>;
// apps/finisherd/src/sandbox/launch.ts
export function startProcess(via: { launcher: string; request: { profile: "agent" | "hook"; input: unknown } } | undefined, argv, cwd, env, platform, onExit): Promise<SandboxedChild>;
// apps/finisherd/src/agent/auto-merge.ts (changed)
export type PreMergeHook = (ev: BranchReady, session: Session, item: ReviewItem) => Effect.Effect<boolean>;
```

## Review Focus

1. **Nothing untrusted runs:** a project hook runs only while its file's hash is the trusted one; a change, deletion or unreadable file stops it at the next event, not at a restart. → Task 2 (`changed_project_hooks_do_not_run`).
2. **Every hook process starts through the launcher module with the `hook` input,** the environment cleared, no credential unless `fin`, no network but `fin`'s endpoint. → Task 3 (`hook_env_is_cleared`, `hook_cannot_reach_network`, spawn gate).
3. **Blocking is exact:** first block wins, a block on an observe-only event never refuses anything, a `PostCommand` block stops later handlers but never undoes the call, a failure follows `onTimeout`, `PreMerge` fails closed by default. → Task 4.
4. **No recursion and no starvation:** hook credentials fire no hooks, `plugin.hook` jobs fire no `JobDone`, the observe queue is bounded, at most 8 processes. → Tasks 4, 5, 8, 10.
5. **The auto-merge point refuses on a hook's block and on a `PreMerge` timeout,** and the trigger's existing refusals are unchanged. → Task 8.
6. **No `modify`:** no path rewrites a payload. → Tasks 1, 9, 10.

---

## Batches

| Batch | Tasks | Delivers | Needs | Can run in parallel with |
|---|---|---|---|---|
| **A** Types, configuration, trust | 1, 2 | protocol types, configuration schema, layers, trust with `hooksHash`, `agent.hooks.list`, console | nothing beyond the integration branch | — |
| **B** Running hooks | 3, 4, 5 | the launcher module, one command hook in the `hook` profile (Windows grants), the engine (order, chain, failures, queue, stats, audit), the `fin` credential | A (3 needs only Task 1) | D's Task 9 |
| **C** Event sources | 6, 7, 8 | session events and context delivery, `Approval`/`Notification`; chain events and merges; `BranchReady`, the auto-merge point, `JobDone` | B (Tasks 3, 4; Task 5 for the `fin` cases); Task 7 also Task 6 | 6 ∥ 8; **7 after 6** (its `PostCommand` context tests use Task 6's delivery) |
| **D** Plugin handlers (8B Task 17) | 9, 10 | `hooks` consumable, protocol, SDKs, `plugin.hook` v1, the plugin layer | 9: Task 1; 10: Tasks 4, 8, 9 | 9 ∥ B, C |
| **E** e2e, docs, validation | 11, 12, 13 | e2e `hooks` scenario, spec as built, tracker, cross-platform run | everything | — |

There is no separate wiring task: each source adds itself to `hooks/wiring.ts` (`buildHooks`) in its own task.

Each batch is reviewed as one unit by a fresh reviewer before the next starts; its review fixes land as `fix(hooks): batch <X> review`.

---

## Batch A: types, configuration, trust

### Task 1: Tracker, protocol types and the configuration schema — tier: opus

**Depends on:** nothing.

**Files:**
- Modify: `TASKS.md` (new section "Sub-project 7F: hooks" with Tasks 1–13 and the batch table; point the 7B follow-up, the 80 % `Notification` line, 7C's "Hook grants: not wired" and 8B Task 17 at this plan).
- Create: `crates/fin-plugin-api/src/protocol/hook.rs` (Frozen interfaces), `crates/fin-plugin-api/tests/hook_protocol.rs`.
- Modify: `crates/fin-plugin-api/src/protocol/mod.rs` (51 lines; `pub mod hook`), `crates/fin-plugin-api/src/schema.rs` (register `HookEvent`, `HookRequest`, `HookReply`, `HookRun`, `HookSession`, `HookProject`), `packages/core-types/scripts/generate.mjs` (`ROOTS` += `HookEvent`, `HookReply`, `HookRequest`); regenerate `packages/core-types/src/generated/index.ts`, `packages/core-types/schema/fin-core.schema.json` and `packages/contracts/src/generated/schemas.ts`.
- Create: `packages/contracts/src/hooks.ts` (`HookSpec`, `HookLayer`, `ListedHook`, `HOOK_LIMITS {perLayer: 64, runArgs: 64, argBytes: 4096, reasonBytes: 1024, textBytes: 16384, contextBytes: 65536, stdoutBytes: 65536, payloadBytes: 1048576, timeoutS: {default: 10, max: 60}, processes: 8, queue: 256}`), `packages/contracts/src/hooks.test.ts`.
- Modify: `packages/contracts/src/index.ts` (export), `packages/contracts/src/audit.ts` (365 lines; `HOOK_FIELDS {event, hookId, layer, session?, projectId?, after?}` (`after: true` for a `PostCommand` block) for `hook.block`, a `HookDetails` schema in `detailsSchema`; never the reason text: the session's notice holds it), `packages/contracts/src/audit.test.ts`.
- Modify: `crates/fin/src/output_codes.rs` (563 → ~566: `PROJECT_HOOKS_UNTRUSTED`, a warning like `SANDBOX_DISABLED`, never an exit), `apps/finisherd/src/console/exit-codes.ts`.

**Step 1: Write failing tests:**
- Rust (`hook_protocol.rs`): `reply_shapes_round_trip` (allow, block, context); `modify_reply_does_not_decode`; `unknown_reply_field_refused`; `event_names_are_pascal_case_and_complete` (`HookEvent::ALL` serialises to exactly the 13 names of spec §9.1 minus `PreOp`/`PostOp`); `classes_are_decision_1` (blocking before, `PostCommand` blocking after, `SessionStart` awaited, the rest observe-only); `pre_merge_defaults_to_block`; `request_v_is_1`.
- TS (`hooks.test.ts`): `hook_spec_decodes_minimal_and_full`; `hook_spec_refuses_unknown_keys`; `match_refused_on_events_without_names` (`SessionStart`, `PreMerge`, …); `timeout_bounds` (0, 61 refused); `run_limits` (65 args, a 4,097-byte arg refused); `event_literals_match_json_schema` (the generated TypeScript `HookEvent` is an erased string union (`generated/index.ts`, e.g. `Harness` at `:75`), so there is no runtime enum to compare: the contracts' `HookEvent` schema is **generated** (Task 1 adds `HookEvent`, `HookReply`, `HookRequest` to `ROOTS` in `packages/core-types/scripts/generate.mjs`, which writes `packages/contracts/src/generated/schemas.ts` with a `Same<…>` check as for `Probe`), and the test compares its literals with `definitions.HookEvent.enum` in `packages/core-types/schema/fin-core.schema.json`; completeness by an exhaustive `const all: Record<HookEvent, true> = {…}` and `satisfies`, so a 14th event fails to compile).
- `audit.test.ts`: `hook_block_details_are_closed` (an extra `reason` key refused).
- `console.test.ts` parity passes with the new warning code.

**Step 2: Implement.** **Step 3:** four gates, clippy, fmt → PASS. **Commit** `feat(hooks): hook protocol types, configuration schema and audit fields`.

### Task 2: Configuration layers, project trust with `hooksHash`, `agent.hooks.list` and the console — tier: opus

**Depends on:** Task 1.

**Files:**
- Create: `apps/finisherd/src/agent/file-cache.ts` (the `readRules` cache of `trust.ts` made generic: `readCached<T>(path, slot, parse: (json) => T): Read<T>`, with `RACY_MS`, `CACHE_MAX`, `O_NONBLOCK`, regular-file and size checks unchanged); `trust.ts` (209 lines) imports it (no behaviour change: `agent-trust.test.ts` stays green untouched).
- Create: `apps/finisherd/src/hooks/config.ts` (the five layers of Decision 7; `projectHooks(dir, trust)` → `{status: none|trusted|untrusted|changed, hooks, ignored, hooksHash?, reason?}` as `projectRules`; Decision 8's `unreadable`; `trustHooks(store, projectId, dir, expectedHooksHash)`), `apps/finisherd/src/hooks/rpc.ts` (`agent.hooks.list {projectId?}` → `{hooks: ListedHook[], ignored?, projectHooks?, hooksHash?}`).
- Modify: `apps/finisherd/src/agent/rule-store.ts` (236 lines; `trustHooks(projectId, hooksHash)`, an `UPDATE project_trust SET hooksHash = ?` like `trustPrompts`), `apps/finisherd/src/agent/rules-rpc.ts` (152 lines; `agent.project.trust` takes `expectedHooksHash?`, checked **before** the rules and prompts so a refusal changes nothing; audited with `hooksHash`; answered with it), `packages/contracts/src/rules.ts` (144 lines; payload and success gain `expectedHooksHash?`/`hooksHash?`), `packages/contracts/src/audit.ts` (`RULE_FIELDS.hooksHash`), `packages/contracts/src/rpc.ts` (554 → 555: `agent.hooks.list`), `packages/contracts/src/scopes.ts` (562 → 563: `agent.hooks.list` `{scopes: ["agent.admin"], resolve: PROJECT, kinds: PEOPLE}`, as `agent.project.trust`), `apps/finisherd/src/agent/command-of.ts` (`RPC_ONLY` gains `agent.hooks.list`), `apps/finisherd/src/agent/console-rules.ts` (130 lines; `finisherd project trust` lists the hooks file's hooks with their ids, events and `run` before confirming, and sends `expectedHooksHash`; the success line names it), `apps/finisherd/src/agent/warnings.ts` (`PROJECT_HOOKS_UNTRUSTED {projectId, status}` when the project has a hooks file that does not apply; added to `agent.session.start`'s warnings), `apps/finisherd/src/handlers.ts` (513 → ≤ 516: register the RPC; call-site lines), `TASKS.md`.
- Tests: `apps/finisherd/test/hooks-config.test.ts`; extend `agent-trust.test.ts`, `agent-saved-prompts.test.ts` (trust with all three hashes), `packages/contracts/src/agent.test.ts` and `index.test.ts` (the RPC list and scope table), `crates/fin` `registry_rpcs*` (`every_rpc_is_named_or_rpc_only` passes with the new RPC_ONLY entry), `agent-console.test.ts`.

**Step 1: Write failing tests:** `layers_in_order` (managed, user, project, local, then plugin rows from a stub); `no_file_is_no_hooks`; `unreadable_user_layer_stands_as_blocking_hook` (and listed); `project_hooks_untrusted_do_not_apply`; `trust_records_hooks_hash`; **`changed_project_hooks_do_not_run`** (edit the file after trusting: `changed`, nothing applies, at the next read without a restart); **`change_within_a_second_is_seen`** (prime the parsed cache with one dispatch, rewrite the file with the same size inside the same second, dispatch again at once: the old hook does not run); `deleted_trusted_file_is_changed`; `trust_binds_what_was_shown` (`expectedHooksHash` stale → `INVALID_REQUEST {field: expectedHooksHash, reason: changed}`, nothing recorded, rules and prompts untouched); `trust_without_hooks_field_keeps_hooks_untrusted`; `invalid_hooks_file_cannot_be_trusted`; `hooks_list_is_people_only` (an agent's call `SCOPE_DENIED`); `run_relative_to_declaring_file`; `start_warns_untrusted_hooks`; `console_trust_shows_hooks_and_sends_hash`.

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(hooks): configuration layers and project trust of hooks`.

---

## Batch B: running hooks

### Task 3: The launcher module and one command hook in the `hook` profile — tier: opus

**Depends on:** Task 1.

**Files:**
- Create: `apps/finisherd/src/sandbox/launch.ts` (move `start` and `killAll` out of `agent/sandbox.ts`; `startProcess` takes the profile in the request and puts it in its `SANDBOX_UNAVAILABLE` details; `agent/sandbox.ts` 471 → ~400 imports it, behaviour unchanged).
- Create: `apps/finisherd/src/hooks/process.ts` (`runCommandHook`: Decision 9's input, environment, cwd, stdin, the stdout cap and decoder, Decision 10's deadline and slot semaphore, the mode from `currentSandboxStatus().hook`, Windows grants per Decision 12; answers a `HookReply` or a `HookFailure {kind: timeout | exit | signal | invalid | tooLarge | unavailable, detail}`).
- Modify: `crates/fin-node/src/sandbox.rs` (54 lines; `sandbox_hook_check(input) -> Option<String>`, like `sandbox_agent_check`), fin-node's `index.d.ts` through its build.
- Modify: `scripts/check-agent-spawn.mjs` and `scripts/check-agent-spawn.test.mjs`: also check `apps/finisherd/src/hooks/` (no allowed file there); `sandbox/launch.ts` is outside both roots.
- Modify: `apps/finisherd/src/hooks/config.ts` (Decision 6's program and `file` resolution at load).
- Tests: `apps/finisherd/test/hooks-process.test.ts`, `apps/finisherd/test/fixtures/hooks/fake-hook.mjs`; `agent-sandbox.test.ts` and `agent-sandbox-seams.test.ts` unchanged and green.

**Step 1: Write failing tests:** `allow_on_empty_stdout`; `block_and_context_replies`; `nonzero_exit_is_failure`; `two_json_values_is_failure`; `stdout_over_cap_is_failure_and_killed`; `reason_over_1kib_is_failure`; **`hook_env_is_cleared`** (a parent variable `SECRET_X` and `FIN_CREDENTIAL` absent; only Decision 9's names); `stdin_is_one_request_line_then_eof`; `cwd_is_private_temp_removed_after`; `timeout_kills_tree` (a hook that forks a sleeper: both gone at the deadline, `HookFailure.timeout`; on Linux under the launcher via `SIGHUP`); `slot_wait_counts_against_timeout` (9 hooks of 1 s at a 1.5 s timeout: the ninth times out); `unavailable_profile_is_failure`; `direct_mode_runs_with_warning`; Linux launcher mode (`FIN_TEST_SANDBOX_READY=hook`): **`hook_cannot_reach_network`** (TCP connect refused, no IP), `hook_reads_project_but_cannot_write_it`, `hook_cannot_read_server_data` (`auth/local.key`), `hook_writes_only_its_temp`; Windows (skipped elsewhere): `windows_hook_grants_owner_call_revoked_at_exit` (recording broker, as `acl-grants.test.ts`); `sandbox_hook_check_reports_bad_input`; **`argument_boundaries_survive`** (an argument holding spaces, `"`, `\` and a trailing `\` is echoed back byte-exact; on every OS in `direct` mode, on Linux under the launcher, and on Windows in AppContainer mode with `FIN_TEST_SANDBOX_READY=hook`); `program_resolution` (`node` → `process.execPath`; a bare name only from the toolchain base, a user `PATH` entry ignored; a declaring-folder path; Windows `.cmd` refused at load); `hook_file_is_granted_read_only` (the script is readable, a sibling file outside the project is not; Windows: the `sandboxHookGrants` table names `file` with a read mask); spawn gate: `hooks_dir_may_not_import_child_process` (in `scripts/check-agent-spawn.test.mjs`, run by `rtk pnpm test:gates`).

**Step 2: Implement.** **Step 3:** four gates → PASS. **Commit** `feat(hooks): run command hooks in the hook sandbox profile`.

### Task 4: The engine — order, chain, failures, the observe queue, audit and stats — tier: opus

**Depends on:** Tasks 2, 3.

**Files:**
- Create: `apps/finisherd/src/hooks/engine.ts` (`HookEngine`: Decisions 1, 3, 4, 5, 18's queue; `fire` resolves layers through `config.ts` and runs `process.ts` per hook, a plugin layer through a port Task 10 fills (none until then); audits `hook.block` with the caller's principal; writes notices through a `SessionNotices` port (Task 6 fills it; a no-op until then)), `apps/finisherd/src/hooks/wiring.ts` (`buildHooks(deps)` → `{engine, policies: PolicyRule[], wrapAfter(hooks: ChainHooks): ChainHooks, close()}`; the sources of Tasks 6–8 add to it).
- Modify: `apps/finisherd/src/main.ts` (**587**): **first, unconditionally,** move the reviews/auto-merge block at `main.ts:415-421` into `startAgentReviews(...)` in `agent/wiring.ts` (378 lines; a separate `refactor(finisherd): …` commit, no behaviour change), which frees ~5 lines; then build the engine after the agent core and before `handlers` and close it on shutdown (budget +5); Tasks 8 and 10 add their lines to `startAgentReviews`/`hooks/wiring.ts`, not `main.ts`. `main.ts` ends ≤ 590, `apps/finisherd/src/serve-options.ts` (51 lines; `ServeOptions.hooks?` for tests: a recording engine or limits), `packages/contracts/src/auth.ts` (`ServerStatusSuccess.hooks?: {events: Record<HookEvent, {runs, blocks, timeouts, failures}>, dropped, unreadable: HookLayer[]}`), `apps/finisherd/src/server-admin.ts` (185 lines; fill it), `TASKS.md`.
- Tests: `apps/finisherd/test/hooks-engine.test.ts`.

**Step 1: Write failing tests:** `layer_order_then_file_order`; `first_block_stops_chain` (the third hook never starts); `context_concatenates_in_order` and `context_over_64kib_drops_later_with_notice`; `block_on_observe_only_event_is_ignored_and_counted`; `failure_follows_on_timeout` (allow → passes with a notice; block → `HOOK_BLOCKED {reason: failed}`); **`pre_merge_timeout_blocks_by_default`**; `timeout_notice_is_hook_timeout`; `match_filters_commands` (any of `commandsOf`'s names); `principals_agent_by_default`; `observe_runs_after_the_call_answered`; `observe_queue_bounded` (257th dropped, `dropped` 1); `block_is_audited_without_reason`; `no_hooks_costs_a_lookup_and_stats` (10,000 `fire`s with no hook for the event: no read, no parse, no process, exactly one `fstat` per applying location per dispatch (counted through an injected fs), the project file not checked while untrusted; the unchanged path and the changed path (a refresh) timed separately, each p99 bound only with `FIN_STRICT_TIMING=1`, the repo's timing pattern); **`queued_event_resolves_hooks_when_it_runs`** (queue a `PostCommand` behind a slow one, change the trusted project file before it runs: the queued event runs no project hook); `post_command_block_stops_later_handlers` (the third `PostCommand` hook never starts; one `hook.block {after: true}` row; the call's answer unchanged); `config_change_applies_without_restart`; `server_status_reports_counters`.

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(hooks): the hook engine`.

### Task 5: The hook's `fin` credential — tier: opus

**Depends on:** Tasks 3, 4.

**Files:**
- Create: `apps/finisherd/src/hooks/credential.ts` (Decision 11: mint, file, renew, revoke on change/stop, start sweep; the recursion guard's set of live hook credential ids).
- Modify: `apps/finisherd/src/hooks/process.ts` (`fin: true` → `HookInput.fin {credential, endpoint}` from the agent listener as `launcherServer` answers it (`agent/sandbox-input.ts:191`), `FIN_CREDENTIAL`/`FIN_SERVER` in the environment; Windows: the project container allowed on the pipe), `apps/finisherd/src/hooks/engine.ts` (skip every event whose principal is a live hook credential), `TASKS.md`.
- Tests: `apps/finisherd/test/hooks-credential.test.ts`.

**Step 1: Write failing tests:** `fin_hook_reads_with_plan_scopes` (the hook runs `fin view` against the project: ok); `fin_hook_cannot_write` (`fin item trim` → `SCOPE_DENIED`); `fin_hook_reaches_only_event_project` (`PROJECT_NOT_FOUND` for another project); `hook_without_fin_has_no_credential`; `one_credential_per_hook_and_project` (two runs: one `credential.mint` audit row); `credential_revoked_when_hook_changes` and `…at_stop`, `leftover_hook_credentials_revoked_at_start`; **`hook_fin_calls_fire_no_hooks`** (a `PreCommand` hook with `principals: all` and `fin` calling `fin view` does not run itself again); Linux launcher mode: `fin_hook_reaches_only_listener_port` (the listener answers; another loopback port and any IP refused).

**Step 2: Implement.** **Step 3:** four gates → PASS. **Commit** `feat(hooks): a plan-scoped fin credential for hooks that declare it`.

---

## Batch C: event sources

### Task 6: Session events, context delivery, `Approval` and `Notification` — tier: opus

**Depends on:** Task 4.

**Files:**
- Create: `apps/finisherd/src/hooks/session.ts` (the bridge: `SessionStart`, `UserPrompt`, `SessionEnd`; Decision 16's context delivery over a narrow `ContextPort {notice, steerHook, hold}`; the `SessionEventTap` for `Notification` (status changes, budget notices) and `Approval` (cards); fills the engine's `SessionNotices` port), `apps/finisherd/src/agent/hook-context.ts` (the held-context queue per session and the steer-or-hold logic over runtime internals passed in, so the runtime takes call-site lines only).
- Modify: `apps/finisherd/src/agent/runtime.ts` (**557**; budget **+20**: `UserPrompt` before `openSession` in `start` and inside `promptIn`/`steer`; `SessionStart` awaited after the open; `promptIn`'s `origin` and `context` (Decision 16); context appended to the provider input and the held queue drained in `promptIn`; `SessionEnd` in `after` beside `reviewLink.sessionEnded`. If it would pass 595, first move `rollback` (`:297-315`) into `agent/rollback.ts`'s caller side), `apps/finisherd/src/agent/runtime-deps.ts` (271 lines; `Deps.hooks?: SessionHookPort`, `Deps.sessionEvents?: SessionEventTap`), `apps/finisherd/src/agent/runtime-engine.ts` (**576**; budget **+4**, ≤ 580: the tap's field and its two calls in `publish()` and `crossings()`, Decision 18), `apps/finisherd/src/agent/watch-hub.ts` (**no change**), `apps/finisherd/src/agent/wiring.ts` (378; pass the port; call-site lines), `apps/finisherd/src/hooks/wiring.ts`, `TASKS.md`.
- Tests: `apps/finisherd/test/hooks-session.test.ts` (real runtime, `fake` harness, as `agent-runtime.test.ts`).

**Step 1: Write failing tests:** `user_prompt_block_refuses_prompt` (`HOOK_BLOCKED`, no run, no node); **`user_prompt_block_on_start_creates_nothing`** (no session, no branch, no credential); `user_prompt_on_steer_and_saved_prompt`; `child_brief_fires_no_user_prompt`; `user_prompt_context_reaches_provider_and_notice` (the fake provider's input ends with the `<hook-context>` block; the `userMessage` node holds the typed text only; a `notice {hookContext}` precedes it); `session_start_context_joins_first_run`; **`delayed_session_start_then_one_user_prompt`** (a `SessionStart` hook that sleeps 1 s and a counting `UserPrompt` hook; `agent.session.start {prompt}`: the first run's input holds the start context then the prompt context, and the `UserPrompt` hook ran exactly once); `session_start_block_is_ignored`; `session_end_observed_once` (also after `delete`); `context_port_steers_running_session` and `context_port_holds_when_idle` (driven through the engine's port with a synthetic context, so no `PostCommand` source is needed: a `userMessage {trigger: hook}` in the run; no run started when idle, delivered with the next prompt); **`budget_80_percent_fires_notification_unwatched`** (no `agent.watch` subscriber: the hook runs once per kind, `match: "budget.*"`); **`attaching_a_watcher_changes_no_hook_runs`** (then attach a subscriber and cross another kind and change status: each event runs its hook exactly once, nothing missed or repeated); `status_change_fires_notification_unwatched`; `approval_card_fires_approval`; `runtime_files_within_size`.

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(hooks): session events, prompt hooks and context delivery`.

### Task 7: Chain events — `PreCommand`, `PostCommand`, `PreMerge`/`PostMerge` on RPCs — tier: opus

**Depends on:** Tasks 4 and 6 (context delivery for `PostCommand`).

**Files:**
- Create: `apps/finisherd/src/hooks/chain.ts` (Decisions 14 and 15: `preCommand: PolicyRule`, `preMerge: PolicyRule`, `withHooksAfter(hooks: ChainHooks): ChainHooks` for `PostCommand`/`PostMerge`; the payload cap; the reviews workflow read for `review.merge`).
- Modify: `apps/finisherd/src/handlers.ts` (513; budget **+4**: append `...hooks.policies` after `commandRules` at `:182`, wrap `hooks` with `hooks.wrapAfter` after `withReviewLink` at `:179`), `apps/finisherd/src/hooks/wiring.ts`, `TASKS.md`.
- Tests: `apps/finisherd/test/hooks-chain.test.ts`; `chain.test.ts` and `policy-gate.test.ts` unchanged and green.

**Step 1: Write failing tests:** **`pre_command_blocks_agent_call`** (`fin item trim` from a session → exit 7 `HOOK_BLOCKED {hook: "user:no-trim", reason}`, nothing written, one `hook.block` audit row); `rules_decide_before_hooks` (a rule `deny` answers `RULE_DENIED` and the hook never runs; an `ask` opens its card before any hook); `pre_command_people_only_with_all`; `pre_command_payload_names_commands`; `dry_run_still_fires_pre_command`; `post_command_after_success_only`; `post_command_never_fails_the_call` (a hook that crashes: the call's answer stands); **`post_command_context_steers_running_session`** and `post_command_context_held_when_idle` (moved here from Task 6); **`post_command_block_reports_to_the_agent`** (the write stays; the next hooks do not run; a `notice {hook: {event: PostCommand, blocked}}` in the run and the reason delivered to the agent as context; one `hook.block {after: true}` row); **`pre_merge_blocks_history_merge_by_a_person`**; `pre_merge_on_review_merge_has_item_fields`; `post_merge_fires_once_per_merge`; `pre_merge_timeout_blocks_merge`; `blocked_write_releases_one_time_grant` (a 7B one-time grant consumed for the call is given back, `Outcome.undo`).

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(hooks): command and merge hooks in the chain`.

### Task 8: `BranchReady`, the auto-merge point, `JobDone` and the inert render events — tier: opus

**Depends on:** Task 4.

**Files:**
- Create: `apps/finisherd/src/hooks/review.ts` (Decision 13: the `onBranchReady` listener, the memoised outcome per event, `preMerge(ev, session, item)`, `PostMerge` after a merged outcome), `apps/finisherd/src/hooks/jobs.ts` (Decision 18's `JobDone` from `JobEngine.onDone`, never for `plugin.hook`; `PreRender`/`PostRender` listed inert).
- Modify: `apps/finisherd/src/agent/auto-merge.ts` (186 lines; `PreMergeHook` gains `item`; the call at `:128` passes it; comment updated: "7F's `PreMerge` and `BranchReady` hooks"), `apps/finisherd/src/agent/wiring.ts` (`startAgentReviews` from Task 4: `preMerge: hooks.preMerge` before `...agentOptions?.autoMerge`, and the trigger wrapped for `PostMerge`; `main.ts` unchanged), `apps/finisherd/src/hooks/wiring.ts`, `TASKS.md` (tick the 7B follow-up line).
- Tests: `apps/finisherd/test/hooks-review.test.ts`, `apps/finisherd/test/hooks-jobs.test.ts`; extend `agent-auto-merge.test.ts` (with `available: true` and the test verdicts it already uses).

**Step 1: Write failing tests:** `branch_ready_runs_for_every_event` (a `draft` session's review request and an end with unmerged commits; context and block as notices in the active run); **`branch_ready_block_refuses_auto_merge`** (`hookBlocked`, item open, `tryAutoMerge` never called); **`pre_merge_block_refuses_auto_merge`** and `pre_merge_timeout_refuses_auto_merge` (default block); `pre_merge_sees_item`; `branch_ready_runs_once_for_both_listeners`; `auto_merge_success_fires_post_merge`; `test_pre_merge_option_still_overrides`; the existing 18 auto-merge cases unchanged; `job_done_fires_with_kind_match`; `job_done_project_hooks_see_own_project_only`; **`plugin_hook_jobs_fire_no_job_done`**; `render_events_listed_inert_and_never_fire`.

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(hooks): BranchReady and PreMerge on the auto merge path, JobDone`.

---

## Batch D: plugin handlers (8B Task 17)

### Task 9: `hooks` consumable, the `plugin.hook` protocol and the SDKs — tier: opus

**Depends on:** Task 1. Replaces the 8B plan's Task 17 Rust half.

**Files:**
- Modify: `crates/fin-plugin-api/src/contribution.rs` (150 lines; `HOST_API` to the next minor (Decision 20) and `consumable` lists `Hooks` from it), `crates/fin-plugin-api/src/manifest/contributes.rs` (183 lines; `HookContribution {event: HookEvent, handler, timeoutS?: 1–60}`), `crates/fin-plugin-api/src/validate/content.rs` (171 lines; a hook needs `hooks:observe` naming its event, a handler name `[a-z][a-z0-9-]{0,63}`, no duplicate `(event, handler)`), `crates/fin-plugin-api/src/validate/permissions.rs` (`hooks:observe`/`modify` hook names must be `HookEvent` names), `crates/fin-plugin-sdk/src/{lib,runtime,macros}.rs` (`Plugin::hook(&self, handler: &str, req: HookRequest, host)` dispatching by the manifest's handler name; `hello` announces `plugin.hook` v1 when the plugin has hooks), `packages/plugin-sdk/src/{index,runtime,envelope}.ts` (`definePlugin({hooks: {<handler>: (req, host) => reply}})`, **keyed by the manifest's `handler` name**, as `commands` are keyed by name (`packages/plugin-sdk/src/runtime.ts:40`); `HookRun.handler` selects it; runtime.ts 272 lines), `crates/fin-plugin-test` (the `command` profile ignores hooks; a `hook` conformance case per declared handler), `apps/finisherd/src/plugins/host/processes.ts` (`PLUGIN_HOST_API`), regenerate core-types, `TASKS.md`.
- Tests: `crates/fin-plugin-api/tests/{validate,protocol}.rs`, `crates/fin-plugin-sdk/tests/sdk.rs`, `packages/plugin-sdk/test/sdk.test.ts`.

**Step 1: Write failing tests:** `hooks_consumable_from_new_minor` (and still refused below it); `hook_needs_observe_for_its_event`; `unknown_event_refused`; `duplicate_handler_refused`; `hook_timeout_bounds`; `modify_permission_still_parses_and_validates`; Rust SDK `hook_handler_round_trip`, `two_handlers_on_one_event_dispatch_by_name` and `hello_announces_hook_kind_only_with_hooks`; TS SDK the same three plus `unknown_handler_is_plugin_error`, and `modify_reply_cannot_be_returned` (a type error in a `@ts-expect-error` line, and a runtime refusal of an untyped one).

**Step 2: Implement.** **Step 3:** four gates, clippy, fmt → PASS. **Commit** `feat(plugins): hook handlers in the plugin protocol and SDKs`.

### Task 10: The plugin layer — `plugin.hook` v1 jobs in the engine — tier: opus

**Depends on:** Tasks 4, 8, 9. Replaces the 8B plan's Task 17 finisherd half.

**Files:**
- Create: `apps/finisherd/src/jobs/kinds/plugin.hook.ts` (as `plugin.command.ts`: family `plugin`, warm, interactive, `maxAttempts: 1`, `timeoutS: 60` with the engine cancelling at the hook's own), `apps/finisherd/src/plugins/host/hooks.ts` (the subscriber index from enabled, active code plugins' manifests (read like `commands.ts`, digest-keyed); rights per Decision 20; submit, wait, decode `HookOut`; fills the engine's plugin layer port).
- Modify: `apps/finisherd/src/hooks/engine.ts` (the plugin layer), `apps/finisherd/src/hooks/wiring.ts` (register the kind with the plugin family through the host's existing registration hook; `main.ts` unchanged), `TASKS.md` (tick 8B Task 17, pointing here).
- Tests: `apps/finisherd/test/plugins-hooks.test.ts` (the 8B test probe `test/fixtures/plugins/probe` gains hook ops).

**Step 1: Write failing tests:** `observe_without_grant_receives_nothing` (8B's name kept); `observe_grant_is_per_event` (replaces 8B's `handler_sees_only_subscribers_events`); `plugin_block_refuses_pre_command`; `plugin_layer_runs_last` (a user-layer block stops the chain before any plugin job); **`modify_reply_is_a_failure`** and `hooks_modify_grants_nothing` (replace 8B's `modify_without_grant_is_allow_and_audited` and `modify_with_grant_rewrites_payload_and_audits_both`: no rewrite, no `hook.modify` row); `hook_timeout_ordering` (8B's name: the manifest's `timeoutS`, then the event default `onTimeout`; `PreMerge` blocks); `plugin_rights_are_invoker_and_grants` (a session's event: the job's rights are the session ∩ grants); `disabled_plugin_hooks_stop_at_once`; `plugin_hook_job_is_not_submittable_by_rpc`.

**Step 2: Implement.** **Step 3:** four gates and `rtk pnpm e2e` → PASS. **Commit** `feat(plugins): plugin hook handlers in the hook engine`.

---

## Batch E: e2e, docs, validation

### Task 11: e2e `hooks` scenario — tier: sonnet

**Depends on:** Tasks 1–10.

**Files:** Create `scripts/e2e/hooks.mjs` (+ `hooks-lib.mjs`, fixtures under `scripts/e2e/fixtures/hooks/`), register in `scripts/e2e.mjs`; `TASKS.md`.

The scenario (release `finisherd` and `fin`, test-hooks fin-node, `fake` harness from a scripts file as `sandbox.mjs` does): a project with `finisher.hooks.json` (a `PreCommand` hook blocking `item trim`, a `UserPrompt` hook adding context) and a user-layer `PreMerge` hook. (1) Untrusted: a session's `fin item trim` lands, the prompt carries no context, `agent.session.start` warns `PROJECT_HOOKS_UNTRUSTED`. (2) `finisherd project trust` shows the hooks and trusts them. (3) The trim is exit 7 `HOOK_BLOCKED`, `auth.audit.list` holds one `hook.block`. (4) The prompt's context shows in `agent.watch` as a `notice {hookContext}`. (5) A person's `fin merge` is refused by `PreMerge`. (6) Editing the file turns the block off until trusted again. (7) `server.status.hooks` counts what ran. On Linux with `FIN_TEST_SANDBOX_READY=hook`, the blocking hook runs under the launcher (the journal's sandbox status, and a hook that tries to write the project folder fails).

- [ ] **Step 1:** scenario and its unit test (`scripts/e2e/hooks.test.mjs`: the step list). **Step 2:** `rtk pnpm test:gates` (runs `scripts/e2e/hooks.test.mjs`), `rtk pnpm e2e hooks` and the full `rtk pnpm e2e` → PASS. **Commit** `test(e2e): hooks scenario`.

### Task 12: Spec as built, cross-references and the tracker — tier: sonnet

**Depends on:** Task 11.

**Files:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (status line revision; §9 rewritten as built with every Decision of this plan mapped to *As built* or *Deviation*; §1.4's 7F row; §15 `PROJECT_HOOKS_UNTRUSTED`; a new §27 "7F as built"); `docs/specs/2026-11-07-sub-project-8-plugins-design.md` (§3.6 row: no `modify` in v1, host API minor as built; §24.4 "For 7F" closed); arch §10 and §27 pointer (`modify` cut, configuration files); `TASKS.md` (7F section ticked; the 7B and 7C follow-up lines and 8B Task 17 closed with pointers; residuals: Owner-question defaults as built, `SessionStart` context in memory only, `PreRender`/`PostRender` inert until 6E).

- [ ] **Step 1:** write; a fresh reviewer checks each claim against the code. **Step 2:** four gates → PASS. **Commit** `docs: 7F hooks as built`.

### Task 13: Cross-platform validation (controller-run; needs the hosts) — tier: opus (controller)

- [ ] 1. `FIN_REQUIRE_SERVER=1 cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm test:gates`, `pnpm e2e` (all scenarios) on macOS (Apple silicon), Windows 11 (non-elevated) and the Linux VM.
- [ ] 2. Under the launcher with `FIN_TEST_SANDBOX_READY=hook`: the `hooks-process` launcher cases on each OS (macOS Seatbelt `hook` profile; Windows in `direct` and AppContainer modes with `sandboxHookGrants` and `call:<id>` revoke, including `argument_boundaries_survive` and `hook_file_is_granted_read_only`), and the `hook` rows of `pnpm sandbox:suite`.
- [ ] 3. Record the `no_hooks_costs_a_lookup` and per-hook start latency (p50/p95, `direct` and launcher) per OS in the spec's §27. Results and any fixes as `fix(hooks): <os> validation`.

---

## Spec and code disagreements (resolved here; the code wins unless noted)

1. **`modify`:** the 8B plan's Task 17 (tests `modify_with_grant_rewrites_payload_and_audits_both`, `modify_without_grant_is_allow_and_audited`) and sp8 §3.6's table row keep `modify`; sp7 §9.1 and §1.6 cut it from v1. **Spec 7 wins** (it owns the hook engine and is the later decision); Decision 19, Task 10's replacement tests; Task 12 fixes sp8 §3.6.
2. **`hooksHash`:** spec §9.4 says `finisherd project trust` records it; the code reserves the column (agent migration 0004) but `RuleStore.trust` and `agent.project.trust` never write it. Task 2 adds it as a third optional hash beside rules and prompts.
3. **The hook timeout:** `HookInput.timeout_ms`'s doc says "the launcher ends the whole tree when it passes", but only the Windows launcher reads `Limits.wall_ms`; Linux and macOS launchers do not. Decision 10: `finisherd` enforces the deadline everywhere.
4. **8B's Task 17 file list** names `crates/fin-plugin-api/src/protocol/hook.rs`, which does not exist; `HookContribution.event` is a free `String` today. Tasks 1 and 9 create the file and type the event.
5. **`PreRender`/`PostRender`:** spec §9.1 wires them to the jobs bus, but no render job kind exists (6E). Decision 17: inert until then.
6. **`SessionStart`/`UserPrompt` points:** spec §9.1 says "fired by the runtime"; the code has no hook point, only `Runtime.after`'s journal phases and `promptIn`. Decision 16 places them; `UserPrompt` on `start {prompt}` runs before the session exists (spec's `session?` is optional, so this is within it).
7. **The `PreMergeHook` signature** (`agent/auto-merge.ts:68`) carries no item; `PreMerge` hooks need one. Decision 13 adds the parameter (call-site change; existing tests pass two-argument functions, which still type-check).
8. **Hook grants on Windows** were built in 7C (`sandboxHookGrants`) but never called; Task 3 wires them.
9. **The no-hook cost:** spec §9.1's "one map lookup" cannot hold together with spec §9.4's "a change disables them again" when configuration is files with no watcher: Decision 5 adds one `fstat` per applying location per dispatch (as `trust.ts` does for rules) and Task 12 records it.
10. **`Notification` source:** `WatchHub` only sees watched sessions (`watch-hub.ts:56`, `runtime-engine.ts:172`), so the tap is in `RuntimeEngine` (Decision 18).
11. **Network for hooks:** arch §10 lists "route to phone, Slack" for `Approval`/`Notification`, but spec §8.10 gives command hooks no network beyond `fin`'s listener. Spec §8.10 wins (Owner question 3); routing goes through plugin handlers with brokered `net`.

## Owner questions (the plan uses each recommended default)

1. **What does a failed hook do (crash, bad reply, timeout)?**
   - **(a, recommended)** One rule: every failure follows the hook's `onTimeout`; default `allow` (fail open, with a visible notice) for every event but `PreMerge`, which fails closed (Decisions 3, 8; unreadable non-project layers fail closed like rules).
   - (b) Timeouts follow `onTimeout`; crashes and bad replies always block blocking events.
   - (c) Fail closed everywhere for blocking events.
2. **Which events may block?**
   - **(a, recommended)** As spec §9.1 reads: `UserPrompt`, `PreCommand`, `PreMerge`, `BranchReady` (gating the `auto` merge only) and `PreRender` (inert) refuse the action; `PostCommand`'s block stops the later handlers and reaches the agent as a visible notice and context, but cannot undo the landed command; `SessionStart` is awaited for context but cannot block; the rest observe-only and detached (Decision 1).
   - (b) Also `SessionStart` (refuse a session's start) and `Approval` (auto-deny a card).
   - (c) `PostCommand` observe-only (its block ignored like the other after-the-fact events).
3. **Network for command hooks?** Spec §8.10 gives them none (only `fin`'s listener), so a command hook cannot post to a phone or chat service.
   - **(a, recommended)** Keep it: routing goes through plugin handlers with `hooks:observe` and brokered `net` (high risk at install, sp8 §4.3).
   - (b) Allow an opt-in `net: true` (open IP like the `agent` profile) for managed, user and local hooks only, never project hooks (a `fin-sandbox` policy change: `Net::Open` for `hook`).
4. **Who may a `principals: "all"` hook observe?** Spec §9.1 allows any hook to ask for every principal.
   - **(a, recommended)** As the spec says, in every layer; a project's hooks only after trust, and they see people's commands only in that project.
   - (b) `all` only in managed, user and local layers: a trusted project folder (which travels) never sees the owner's own commands.
5. **`hooks:modify` in manifests, now that `modify` is cut:**
   - **(a, recommended)** Keep accepting it (8A validates it; shown high risk); it grants nothing in v1 (Decision 19).
   - (b) Refuse it at install and enable (`PLUGIN_CONTRIBUTION_UNSUPPORTED {kind: hooksModify}`) until a release implements it.
6. **Lifetime of a hook's `fin` credential:**
   - **(a, recommended)** One per (hook, project), minted on first use, revoked when the hook changes, at stop and swept at start (Decision 11): two audit rows per hook and project, not per run.
   - (b) One per run (mint and revoke each time: exact but two audit rows per hook run, which a `PreCommand` hook with `fin` makes per agent command).
7. **`SessionStart` context before a first prompt:**
   - **(a, recommended)** Held in memory; lost if the server restarts before the session's first prompt (recorded residual).
   - (b) Stored on the session (`session_state`, one more `agent` migration).
8. **Trust re-prompt when `finisher.hooks.json` changes:**
   - **(a, recommended)** The project's hooks stop at once; `agent.session.start` warns `PROJECT_HOOKS_UNTRUSTED {status: changed}`, `agent.hooks.list` and `finisherd project trust` show it; a person re-trusts (Task 2).
   - (b) Also stop the project's running sessions' `PreCommand` calls (refuse with `HOOK_BLOCKED {reason: untrusted}`) until re-trusted.
