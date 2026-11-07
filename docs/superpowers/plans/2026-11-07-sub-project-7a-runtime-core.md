# Sub-project 7A: Runtime core and the built-in agent — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the event kinds and their classes, the session state machine and crash recovery, credentials and branch reach, secrets, the built-in loop and its tools, attribution, verdicts), `sonnet` for plumbing (contracts, the catalogue, API adapters against the fake server, RPC and console wiring, e2e, docs).

**Goal:** an agent can edit a Finisher project on its own branch, driven by Finisher's own loop on the user's API keys, with every turn, tool call, result and verdict recorded under D34 and joined to its commits. After this plan:
- sessions, runs and nodes are persisted in `control.db`, become `interrupted` on a crash and resume without re-running a tool;
- one adapter contract (`ProviderAdapter`) with two implementations: the scripted `fake` harness and the `builtin` loop over Anthropic Messages, OpenAI Responses and OpenAI-compatible Chat Completions;
- the built-in agent's tools are a native `fin` tool (argv, no shell) and three workspace file tools confined to `workspace/<sessionId>/`;
- each session gets its own branch and a minted `agent` credential whose scopes come from its mode (`plan`, `draft`, `auto`) and whose writes are confined to its branch subtree;
- a bundled model catalogue prices every turn; token, money, wall-clock and call budgets stop a run at a safe point;
- API keys live in an encrypted `SecretStore` and reach a model call only through `ModelGateway`;
- the agent kinds are version 1 (`agent.session`, `agent.turn`, `agent.toolCall`, `agent.toolResult`, `agent.editVerdict`, the new `agent.approval`), with `FIN_CALL` set exactly for every built-in tool call, large content as blobs, and verdicts from merges, undos, reverts and abandoned branches;
- `finisherd agent …` and `finisherd provider key …` console commands drive all of it for people.

**Architecture:**
- **`fin-core::events`:** the v1 payloads, `agent.approval`, the tables `AGENT_ROLES` and `AGENT_TOOLS`, `AGENT_MODELS` grown to the catalogue, a `Harness` enum.
- **`apps/finisherd/src/agent/`** (new): `store.ts` (migration and queries), `session.ts` (state machine), `adapter.ts` (contract and events), `runtime.ts` (runs, events → nodes, crash recovery), `fake.ts`, `builtin/{loop.ts, tools.ts, anthropic.ts, openai.ts, compat.ts, request.ts}`, `catalogue/{models.json, catalogue.ts}`, `budget.ts`, `credentials.ts`, `reach.ts`, `journal.ts`, `verdicts.ts`, `rpc.ts`, `console.ts`.
- **`apps/finisherd/src/secrets/`** (new): `store.ts`, `gateway.ts`; the encryption is in `fin-node` (`secrets.rs`).
- **`@finisher/contracts`:** `agent.ts` (RPC group, `Session`, `Run`, `Node`, `ProviderEvent` schemas) and the new scopes.
- **`fin`:** the default idempotency key from `FIN_CALL` (Decision 14).
- **`fin-node`:** `secrets.rs` (XChaCha20-Poly1305), and `attachBlob` on the napi `Journal` if D4 did not expose it.

**Tech Stack:** Node 26.12, Effect 4.0.6, `node:sqlite` (through 5A's `ControlStore`), `@anthropic-ai/sdk` 1.6.0 (Apache-2.0), `openai` 7.53.0 (Apache-2.0), vitest; Rust 2024, `chacha20poly1305` 0.11 (Apache-2.0 OR MIT), `schemars` 1. No other new dependencies; `pnpm gates` runs the licence gates over these.

**Spec:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1–5, §6.1, §10, §12, §13, §15, §16 (7A parts), §19–20. Parent: arch §8, §9, §16, §21.1. Related: the diagnostics plan Decisions 3, 14b, 15–20, 23a, 24, 28, 34; the sub-project 5 design §3.2–3.5, §4.1–4.5 (principals, the agent credential, scopes, human-only, exit 7) and its 5A plan.

**Branch:** `sp7a-runtime`, stacked on the branch that holds **5A** (Tasks 1–12) and therefore D1–D7. 7A calls 5A's `ControlStore`, `Audit`, the agent-credential minting service and the authorisation wrapper; it must not start before they exist. The `control.db` migration takes the next free number at that point.

**Out of scope:** session rollback and fork, subagents, roles beyond `editor` and `critic` as data stubs, saved prompts, command-pattern rules, approvals answered by people (the `agent.approval` kind and table ship; only `budget` approvals are created, answered from the console), review requests and review verdicts, context compaction (7B); the OS sandbox and the agent listener (7C); Claude Code (7D) and Codex (7E); hooks (7F); workspace artifacts and the rest of the full `fin` (7G); the eval harness (7H); the keychain backend (with 5B); any UI (9).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code; `pnpm gates` runs cargo-deny and the JS licence gate.
- **No cloud-only imports** (the import-graph guard).
- **Model and event types are generated, never hand-written:** every payload change derives `JsonSchema` and goes through `crates/fin-store/examples/export_schema.rs` and `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). Runtime types that are TS-owned control-plane state (`Session`, `Run`, `Node`, `ProviderEvent`) are Effect Schemas in `@finisher/contracts`, like sub-project 5's.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. `apps/finisherd/src/projects.ts` (520) and `main.ts` take call-site lines only; every agent file is new.
- **No secret in the environment, argv, a log, the journal or a project folder.** API keys enter by stdin to the console, live encrypted in server data, and are decrypted only inside `ModelGateway` for one request.
- **Off means off** (D34): every emitter checks the journal gate before it builds a payload; nothing reaches disk for a project that is `never` or unknown.
- Errors are `{ code, message, details, fix? }`; new codes go into `crates/fin/src/output.rs` (exit classes) and `apps/finisherd/src/errors.ts`.

**Testing and process**
- **Tests never use the network.** Model APIs run against the loopback fake model server (Task 9).
- Clocks and id makers are injected (`Clock`, `Ids`) in every runtime module, so timeouts, budgets and retention are tested without sleeping.
- Crash tests spawn `finisherd` as a child and kill it (`SIGKILL`; `TerminateProcess` on Windows), as the 4A lease tests do.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch `fin` or `finisherd` wiring also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures and scripted transcripts included.
- **Tracker:** each task ticks its line in `TASKS.md` under a new heading "Sub-project 7A: runtime core", in the same commit.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Store and state

1. **Tables** (one migration, the next free `m<n>` after 5A/5C/5E's):
   - `sessions {id, projectId, harness, provider, model, role, mode, rootBranch?, base {branch, seq}, parent?, status, statusReason?, credentialId?, providerRef?, budget JSON, createdBy JSON, createdAt, endedAt?, title?}`;
   - `runs {id, sessionId, seq, trigger, status, startedAt, endedAt?, headAtStart?, stopReason?, usage JSON}`;
   - `nodes {id, runId, sessionId, seq, kind, providerId?, payload JSON | blobRef, createdAt}` with an index on `(sessionId, seq)`;
   - `session_txs {txId, sessionId, runId, callId, at}` (the verdict tracker's join);
   - `approvals {id, sessionId, kind, subject JSON, requestedAt, expiresAt, decision?, by?, answeredAt?}`;
   - `budgets {sessionId, kind, limit, used, reserved}`.
   Node payloads ≤ 64 KiB are inline; larger ones are written to `<server-data>/agents/<sessionId>/blobs/<blake3-hex>` (temp-then-rename) first, then referenced.
2. **The state machine** (`session.ts`) is a pure function `next(state, event) → {state, effects}` over the statuses of spec §3.2, tested by properties. Persistence applies its effects in one `ControlStore.tx`.
3. **Crash recovery** at start: every `starting|running|waitingApproval` session → `interrupted` (`serverRestart`); every open run → `interrupted`; an open `toolCall` node without a result gets a synthetic `toolResult {ok: false, code: SESSION_INTERRUPTED}`.
4. **Retention:** product state of `ended` sessions older than `agents.retentionDays` (default 90) is swept hourly with their blob directories. `agent.session.delete` (owner) removes one at once.

### Providers and the built-in loop

5. **`ProviderEvent`** is the closed union of spec §4.1. Adapters emit only events; `runtime.ts` alone writes nodes, budgets and journal entries, in event order per session.
6. **One active run per session.** `prompt` while running is `SESSION_BUSY`; `steer` on the built-in loop queues the text as a user message appended before the next model call (the loop has no mid-stream injection).
7. **Requests are built by Finisher** (`builtin/request.ts`): one `ModelRequest {system, tools, messages, model, maxTokens, temperature?, reasoning?}` in Finisher's own neutral form, translated per API. The translated body is what the `capture` event records, byte for byte as sent.
8. **API specifics:**
   - Anthropic: `messages.stream`; `cache_control: {type: "ephemeral"}` on the last system block, the tool list and the last stable message; extended thinking per the catalogue's capability, its blocks kept and sent back unchanged.
   - OpenAI Responses: `store: false`, full input every call, `include: ["reasoning.encrypted_content"]` so reasoning carries across calls without server state; function tools.
   - Compatible: Chat Completions with `stream_options.include_usage`; a model without tool support is refused at start (`MODEL_UNKNOWN` with `details.reason: noTools`).
9. **Retries** (built-in only): 408, 429, 500, 502, 503, 504, 529 and connection resets; `retry-after` (seconds or HTTP date) or 1 s × 2ⁿ, ≤ 60 s, 5 attempts; each retry is a `notice` node. A retry never re-runs a tool: it re-sends the same model request.
10. **Caps per run:** 60 model calls, 200 tool calls, 16 KiB of each tool result shown to the model (head 12 KiB + tail 4 KiB + a note naming the node), context stop at 85 % of the catalogue's window (`MODEL_CONTEXT_EXCEEDED`, run ends, session `idle`).

### The built-in tools

11. **`fin {argv: string[], stdin?: string}`:**
    - runs the `fin` binary (`agents.finPath`, default: next to `finisherd`'s bundled binaries; in development `target/<profile>/fin`) with `execFile` semantics: no shell, argv as given, `cwd` = `workspace/<sessionId>/`;
    - environment: `childEnv({FIN_SERVER, FIN_SESSION})` plus `FIN_CREDENTIAL` and `FIN_CALL` (Decision 14); `FIN_SERVER` is the normal local socket until 7C's agent listener;
    - timeout 120 s (`agents.toolTimeoutS`), then kill the process tree; stdout is the result (JSON when `fin` printed JSON), with the exit code and the last 1 KiB of stderr;
    - `argv[0]` is a command word, never a path; `fin` itself refuses anything an agent principal may not do (scopes, `HUMAN_ONLY`, branch reach), so the tool needs no command filter of its own.
12. **`workspace_read {path, offset?, limit?}`, `workspace_write {path, content, mode?: create|overwrite|append}`, `workspace_list {path?}`:** paths are relative, normalised, never absolute, never `..`; the real path (symlinks resolved, case-folded on Windows and on case-insensitive macOS volumes) must stay under `workspace/<sessionId>/` (`PATH_NOT_ALLOWED`, exit 7); reads ≤ 256 KiB per call, writes ≤ 8 MiB per file and 256 MiB per session; writes are temp-then-rename (arch §6.7).
13. **Tool definitions** are static JSON in `builtin/tools.ts`; their hash is `toolsHash`. The system prompt is the role's prompt + the `fin` map (from `fin` with no arguments, captured once per `fin` version) + `fin context`-like facts (project, branch, mode, budget); its hash is `systemHash`.
14. **`FIN_CALL` and idempotency:** when `FIN_CALL` is set and a write command has no `--idem-key`, `fin` uses `FIN_CALL` as the key (`batch` lines use `<FIN_CALL>.<line>`). A built-in write's txId therefore joins its tool call directly, and a retried call after a crash is a dedupe (spec §3.4). A user's `fin` without `FIN_CALL` is unchanged.

### Sessions, branches and credentials

15. **Start** (`agent.session.start`, scope `agent.run`, human principals only in 7A):
    1. resolve role (`editor` default), mode (`draft` default, ≤ the role's ceiling), harness (`builtin`), model (role default for the harness, or given; must be in the catalogue or an `openaiCompatible` entry the owner configured);
    2. unless `plan`: create the root branch from `base` (default `main` head) named `agent/<role>-<n>` through `branch.create` as the starter's principal;
    3. mint the agent credential through 5A's service: scopes from the mode table (spec §6.1, minus `jobs.submit`, which needs 5C, and `review.write`, which needs 5E: both added when those land), `projects: [id]`, `branches: [root]`, `expiresAt: null` until `end`;
    4. write `<server-data>/agents/<sessionId>/credential.json` (0600; temp-then-rename) and create `tmp/`;
    5. record the session `idle`, then start the first run when `prompt` is given.
16. **Branch reach** (`reach.ts`), checked in 5A's authorisation wrapper for `agent` principals with a `branches` list: the target branch of `tx.commit`, `history.undo|redo|checkpoint|cherryPick|revert`, the `into` of `history.merge`, the branch of `branch.set`, and the parent of `branch.create` must be the root or a descendant of it (lineage from the engine's branch index, cached per project and refreshed on `branch` records). Otherwise `BRANCH_NOT_ALLOWED` (exit 7), audited as `auth.denied`. Reads are not restricted.
17. **`Tx.author`** for the built-in tool's commits is `agent:<role>:<sessionId>` (set by `fin` from `FIN_AUTHOR`, which the tool sets).
18. **End** revokes the credential and sets `endedAt`; the branch stays. **Delete** (owner) ends first, then removes product state and the session directory.

### Budgets and the catalogue

19. **Catalogue** (`catalogue/models.json`): `{asOf, models: [{id, provider, api: anthropic|openaiResponses|chat, window, maxOutput, capabilities: {tools, thinking, images, caching}, price: {input, output, cacheRead, cacheWrite} per MTok in micros}]}`. Task 4 fills the Anthropic entries (`claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`) and the OpenAI models the owner names, with prices read from the providers' pricing pages on the day and `asOf` set; tests use a separate test catalogue. Owner-configured compatible endpoints add `{id, baseUrl, window, price?}` entries in settings (`agents.compatible`), priced 0 when no price is given (`costBasis: unknown`).
20. **Default budgets per session** (settings `agents.defaultBudget`): 2,000,000 tokens, 10 USD, 60 minutes of wall clock with a run active, the per-run caps of Decision 10. Per server per day: 50 USD across API-key sessions (`agents.dailyMoneyMicros`). The role may lower them; the starter may set them.
21. **Enforcement points:** before each model call (projected: `used + maxTokens × output price` must fit) and after each turn and tool call. At ≥ 80 % a `notice` node (once per kind); at ≥ 100 % the run stops after the running tool call, the session becomes `interrupted` with `BUDGET_EXCEEDED`, and a `budget` approval is created (`finisherd agent approve <id> allow --add <amount>` raises the limit and resumes).

### Secrets

22. **`SecretStore` file backend:** `<server-data>/secrets.json` (0600), entries `{id, provider, label, baseUrl?, nonce, ciphertext, createdAt}` encrypted with XChaCha20-Poly1305 under `<server-data>/auth/secrets.key` (32 random bytes, 0600, created on first use, never rotated automatically: rotation re-encrypts). The encryption runs in `fin-node` (`secrets.rs`); plaintext crosses napi only inside `ModelGateway.withKey`.
23. **`ModelGateway.withKey(provider, sessionId, f)`** decrypts, calls `f(key)` for one request, and drops the buffer (overwritten). It refuses a session that is not `running` (`PROVIDER_AUTH`). The first grant per session and every refusal are audited (`grant`).
24. **Console only:** `finisherd provider key set <provider> [--label] [--base-url]` reads the key from stdin (never argv), `provider key ls` shows ids, labels and providers (never values), `provider key rm <id>`. RPCs for the UI come with sub-project 9; none returns a value.

### Journal

25. **Emission** (`journal.ts`) through `fin-node`'s napi `Journal` with `source: agent`, `session` = the `finisherd` process run, and `ctx {project, branch: root, actor: {kind: agent, session: <sessionId>, call?}}`:
    - `agent.session` at `started`, `resumed`, `interrupted`, `ended` (totals at `ended`);
    - `agent.turn` per model call (built-in: `capture: exact`; the request body and the response as content);
    - `agent.toolCall` at announcement, `agent.toolResult` at completion with `txIds` from `session_txs` for that call and `attribution: exact` (built-in tools always set `FIN_CALL`);
    - `agent.approval` for budget approvals;
    - `agent.editVerdict` from the tracker (Decision 27).
26. **Blobs:** any content field over 16 KiB becomes a `Body::Blob` through `attachBlob` (≤ 4 MiB; above it, truncated with `truncated: true`); `system` and `tools` are always blobs, so one copy is stored per distinct prompt (content addressing) and dropped with its last entry (D-plan Decision 20).
27. **The verdict tracker** (`verdicts.ts`) subscribes to the history RPC results inside `finisherd` and to branch status changes:
    - `history.merge` copying records whose origin txIds are in `session_txs`, into a branch outside that session's subtree → `merged`, `by` from the merging principal's actor;
    - `history.undo`/`history.revert` naming such a txId → `undone`/`reverted`, `by: agent` when the actor is that session, `user` when a human principal, `agent` (another session) otherwise;
    - `branch.set {archived: true}` of a session's root with unmerged session txs, or `agent.session.delete` → `abandoned` for those txs;
    - one verdict per `(txId, via)`; `afterS` from `session_txs.at`.
    If a history result does not name the txIds it undid or copied, Task 11 adds them to the `fin-node` result (an additive field) rather than re-deriving them in TS.

## Questions for the owner

The spec's Owner questions 1 (which outputs may be training targets) and 2 (exactness of CLI traffic) do not block 7A: 7A records everything the built-in agent sends and receives under the training opt-in, with provenance on every turn, which every answer to question 1 needs. Questions 3–5 concern 7C, 7D/7E and later.

## Review Focus

1. **No secret leaks:** keys never in env, argv, logs, nodes, the journal or an RPC result; `ModelGateway` holds plaintext for one request only; the console reads keys from stdin.
2. **An agent cannot become the user:** the built-in `fin` tool runs with the agent credential only; `local.key` is not readable through `workspace_read` (path confinement); a session cannot write outside its branch subtree through any RPC (`reach_matrix`).
3. **Resume never re-runs an edit:** the `FIN_CALL` idempotency default, the synthetic interrupted result, and the crash tests at every point of a tool call.
4. **The journal join:** every built-in write's `log.commit.ctx.actor.call` equals its `agent.toolCall.callId`, and `agent.toolResult.txIds` lists exactly those commits.
5. **Consent:** with only diagnostics on, no `agent.*` content and no blob reaches disk; with `collect: never`, nothing for that project.
6. **Budgets stop at a safe point** and never mid-commit.
7. **Event order:** nodes, budgets and journal entries follow the provider event order per session, and a late event from a replaced provider instance is dropped.

---

### Task 1: Agent kinds v1 in `fin-core` — tier: opus

**Files:**
- Modify: `crates/fin-core/src/events/payloads/agent.rs` (v1 payloads), `crates/fin-core/src/events/kinds.rs` (versions 1; `agent.approval`), `crates/fin-core/src/events/names.rs` (`AGENT_ROLES`, `AGENT_TOOLS`, `AGENT_MODELS` growth), `crates/fin-store/examples/export_schema.rs`, `packages/core-types/schema/fin-core.schema.json`, `packages/core-types/src/generated.ts` (generated), `TASKS.md`.
- Test: `crates/fin-core/tests/events_agent.rs`.

**Interfaces:**
```rust
pub enum Harness { Builtin, ClaudeCode, Codex, Fake, Plugin }
pub enum AgentMode { Plan, Draft, Auto }
pub enum SessionPhase { Started, Resumed, Interrupted, Ended }
pub enum SessionTrigger { User, SavedPrompt, Subagent, Hook, Eval }
pub struct AgentSession { session_id: IdRef, phase: SessionPhase, harness: Harness, provider: Name, model: Name,
    model_hash: Option<Hashed>, role: Name /* AGENT_ROLES */, mode: AgentMode, parent: Option<IdRef>,
    trigger: SessionTrigger, reason: Option<Code>, totals: Option<SessionTotals> }
pub struct SessionTotals { turns: u32, tool_calls: u32, txs: u32, input_tokens: u64, output_tokens: u64,
    cost_micros: Option<u64>, wall_s: u64 }
pub enum CostBasis { Reported, Priced, Subscription, Unknown }
pub enum Capture { Exact, Stream }
pub struct AgentTurn { turn_id, run_id: IdRef, seq: u32, parent_turn_id: Option<IdRef>, provider, model,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens: u64, latency_ms,
    ttft_ms: Option<u64>, stop_reason, cost_micros: Option<u64>, cost_basis: CostBasis, capture: Capture,
    system_hash: Hashed, tools_hash: Hashed,
    messages, output, system, tools: Option<Content<Body>> /* modelInput, modelOutput, modelInput, modelInput */ }
pub struct AgentToolCall { call_id: IdRef, turn_id: IdRef, tool: Name /* AGENT_TOOLS */,
    command: Option<Name> /* CLI_COMMANDS */, args_hash: Hashed, args: Option<Content<Body>> }
pub enum Attribution { Exact, Inferred, Ambiguous, None }
pub struct AgentToolResult { call_id, ok: bool, code: Option<Code>, exit: Option<i32>, micros: u64,
    tx_ids: Vec<IdRef>, attribution: Attribution, truncated: bool, output: Option<Content<Body>> }
pub enum ApprovalKind { Rule, Budget, Provider, Plugin }
pub enum ApprovalDecision { Allow, Deny, Expired }
pub struct AgentApproval { approval_id: IdRef, session_id: IdRef, kind: ApprovalKind,
    decision: ApprovalDecision, by: DecidedBy /* user|policy */, latency_ms: u64, subject: Option<Content<Body>> /* userText */ }
pub enum Verdict { Accepted, Rejected, Undone, Reverted, Merged, Abandoned }
pub enum VerdictVia { Review, Undo, Revert, Merge, Archive, Critic }
pub struct AgentEditVerdict { tx_ids: Vec<IdRef> /* ≤ 64 */, session_id: IdRef, verdict: Verdict, by: VerdictBy,
    via: VerdictVia, after_s: u64, reason: Option<Content<Body>> /* userText */ }
```
Spec §12.2; D-plan Decisions 3, 4, 20, 28.

- [ ] **Step 1: Write failing tests:**
  - `agent_kinds_are_version_1` (all five; `agent.approval` registered at 1; `training_only` stays on turn, toolCall, toolResult);
  - `agent_kinds_round_trip` (a full payload each; `Body::Inline` and `Body::Blob`);
  - `payload_strings_are_classified` still passes with the new fields (the existing schema walk);
  - `redact_drops_agent_content` (under diagnostics, `messages`, `output`, `system`, `tools`, `args`, `subject`, `reason` are gone and the hashes and counts remain);
  - `blob_refs_listed` (`AnyPayload::blobs()` finds every `Body::Blob` in the new fields);
  - `agent_tables_match_patterns` and `roles_and_tools_tables_have_other`;
  - `tx_ids_capped_at_64` (`validate_payload` refuses 65 with `EVENT_INVALID`).
- [ ] **Step 2: Implement.** Keep `approx_struct!` lists complete (the D1 `approx_covers_every_field` test). `AGENT_MODELS` gets the Anthropic ids now; Task 4's drift test adds the rest.
- [ ] **Step 3: Regenerate** the schema and TS types; run the gates → PASS. **Commit** `feat(core): agent event kinds v1 with harness, modes, tool kinds, attribution and approvals`.

### Task 2: Contracts: agent RPCs, runtime schemas and scopes — tier: sonnet

**Files:**
- Create: `packages/contracts/src/agent.ts`.
- Modify: `packages/contracts/src/rpc.ts` (the group), `packages/contracts/src/scopes.ts` (5A's file: `agent.read`, `agent.run`, `agent.approve` (human), `agent.admin` (owner); each new RPC's scope), `packages/contracts/src/index.ts`, `crates/fin-core/src/events/names.rs` (`RPC_METHODS` additions; the D4 drift test), `TASKS.md`.
- Test: `packages/contracts/src/agent.test.ts`.

**Interfaces:**
```ts
export const Harness = Schema.Literals(["builtin", "claudeCode", "codex", "fake"]);
export const Mode = Schema.Literals(["plan", "draft", "auto"]);
export const SessionStatus = Schema.Literals(["starting", "idle", "running", "waitingApproval", "interrupted", "ended"]);
export const NodeKind = Schema.Literals(["userMessage", "assistantMessage", "reasoning", "plan", "toolCall",
  "toolResult", "approval", "subagent", "usage", "notice", "error"]);
export const Session: Schema.Struct<…>;  export const Run: Schema.Struct<…>;  export const Node: Schema.Struct<…>;
export const BudgetSpec: Schema.Struct<{ tokens?, moneyMicros?, wallS?, modelCallsPerRun?, toolCallsPerRun? }>;
export const ProviderEvent: Schema.Union<…>;   // spec §4.1, used by adapters and the fake's scripts
export const WatchItem: Schema.Union<[NodeAdded, Delta, StatusChanged, UsageUpdated, ApprovalOpened]>;
// RPCs: agent.session.start|prompt|steer|interrupt|resume|end|list|get|delete, agent.watch (stream),
//       agent.approval.list|answer, agent.providers
```
Spec §3.5; Decisions 6, 15.

- [ ] **Step 1: Write failing tests:** every new RPC has exactly one scope (5A's `rpc_scopes_cover_every_rpc`); `agent.approval.answer` and `agent.session.delete` are refused for `local` and `agent` principals (5A's `human_only_matrix` extended); schema round trips for every `ProviderEvent` and `WatchItem` variant; `agent.watch` is listed among the stream RPCs excluded from `rpc.stats` (`rpc_stats_excludes_streams`).
- [ ] **Step 2: Implement.** `agent.session.start` from an `agent` principal is `SCOPE_DENIED` in 7A (a parent's own children arrive in 7B).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(contracts): agent runtime RPCs, schemas and scopes`.

### Task 3: The session store and the state machine — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/{store.ts, session.ts, blobs.ts}`, `apps/finisherd/src/control/migrations/m<n>_agent.ts`.
- Modify: `apps/finisherd/src/control/migrations.ts` (register), `TASKS.md`.
- Test: `apps/finisherd/test/agent-store.test.ts`, `apps/finisherd/test/agent-session.test.ts`.

**Interfaces:**
```ts
// session.ts (pure)
export type SessionEvent = { kind: "runStarted"; runId: string } | { kind: "runEnded"; stop: string }
  | { kind: "approvalOpened"; id: string } | { kind: "approvalAnswered"; id: string }
  | { kind: "interrupted"; reason: string } | { kind: "resumed" } | { kind: "ended"; reason: string };
export function next(s: SessionState, e: SessionEvent): { state: SessionState; effects: Effect[] } | { error: FinError };
// store.ts
export class AgentStore {
  constructor(control: ControlStore, serverData: string, clock: Clock, ids: Ids);
  createSession(s: NewSession): Session;
  apply(sessionId: string, e: SessionEvent): Session;            // one tx; SESSION_BUSY / SESSION_ENDED
  appendNode(n: NewNode): Node;                                  // inline ≤ 64 KiB, else blob first
  nodes(sessionId: string, fromSeq?: number, limit?: number): Node[];
  recordTx(txId: string, sessionId: string, runId: string, callId: string): void;
  sessionOfTx(txIds: string[]): Map<string, SessionTx>;
  recoverAfterCrash(): { interrupted: string[] };                // Decision 3
  sweep(now: number): number;                                    // Decision 4
  delete(sessionId: string): void;
}
```
Decisions 1–4.

- [ ] **Step 1: Write failing tests:**
  - `state_machine_properties` (fast-check: random event sequences never reach an undefined state; `ended` absorbs; `prompt` while `running` → `SESSION_BUSY`; `resume` only from `interrupted`);
  - `node_blob_threshold` (64 KiB inline, 64 KiB + 1 in a blob, written before the row; a crash between the two leaves an orphan blob, swept, never a dangling row);
  - `recover_marks_interrupted_and_closes_open_calls` (a session `running` with an unanswered `toolCall` node gets a synthetic result);
  - `session_txs_join` (`sessionOfTx` for known and unknown txIds);
  - `retention_sweeps_ended_sessions_and_blobs`; `delete_removes_rows_and_dir`;
  - `statement_budget` (5A's per-statement budget, with 10k sessions and 1M nodes in the measurement script; ≤ 1 ms p99 on the Linux VM, asserted ≤ 5 ms in the test).
- [ ] **Step 2: Implement.** All writes through `ControlStore.tx`; no bulk work on the main thread (blob writes are async file I/O before the transaction).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): agent session store, state machine and crash recovery`.

### Task 4: The model catalogue and the budget ledger — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/agent/catalogue/{models.json, catalogue.ts}`, `apps/finisherd/src/agent/budget.ts`, `apps/finisherd/test/fixtures/catalogue.test.json`.
- Modify: `crates/fin-core/src/events/names.rs` (`AGENT_MODELS` = the catalogue's ids + `other`), `TASKS.md`.
- Test: `apps/finisherd/test/agent-budget.test.ts`, `crates/fin-core/tests/events_agent.rs` (`catalogue_ids_in_agent_models`, reading `models.json` at test time).

**Interfaces:**
```ts
export interface ModelEntry { id: string; provider: "anthropic" | "openai" | "other"; api: "anthropic" | "openaiResponses" | "chat";
  window: number; maxOutput: number; capabilities: { tools: boolean; thinking: boolean; images: boolean; caching: boolean };
  price?: { input: number; output: number; cacheRead: number; cacheWrite: number } /* micros per MTok */; baseUrl?: string }
export class Catalogue { static load(bundled: string, settings: CompatibleEntry[]): Catalogue; get(id: string): ModelEntry | undefined;
  price(id: string, u: Usage): { micros: number; basis: "priced" | "unknown" } }
export class Budget {
  constructor(store: AgentStore, sessionId: string, limits: BudgetLimits, daily: DailyLedger);
  canCall(projected: Usage): Allowance;            // ok | stop {kind, used, limit}
  charge(u: Usage, micros: number): Crossings;     // notices at 80 %, stop at 100 %
  raise(kind: BudgetKind, by: number): void;
}
```
Decisions 10, 19–21.

- [ ] **Step 1: Write failing tests:** pricing with cache read and write; `unknown` for unpriced compatible models; projected stop before a call that cannot fit; 80 % notice once per kind; 100 % stop; daily money cap across sessions and its UTC-day rollover (injected clock); `raise` after a stop; `catalogue_ids_in_agent_models`.
- [ ] **Step 2: Fill the catalogue** from the providers' pricing and model pages on the day (record the URLs in the commit body, not in the file); set `asOf`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): model catalogue and session budgets`.

### Task 5: `SecretStore` and `ModelGateway` — tier: opus

**Files:**
- Create: `crates/fin-node/src/secrets.rs`, `apps/finisherd/src/secrets/{store.ts, gateway.ts}`.
- Modify: `crates/fin-node/Cargo.toml` (`chacha20poly1305`), `crates/fin-node/src/lib.rs` (exports), `deny.toml` if needed, `TASKS.md`.
- Test: `crates/fin-node/tests/secrets.rs`, `apps/finisherd/test/secrets.test.ts`.

**Interfaces:**
```rust
#[napi] pub fn secrets_seal(key_path: String, plaintext: Buffer) -> napi::Result<SealedSecret>; // {nonce, ciphertext} base64url
#[napi] pub fn secrets_open(key_path: String, sealed: SealedSecret) -> napi::Result<Buffer>;   // SECRET_CORRUPT on a bad tag
```
```ts
export interface SecretStore { put(e: { provider; label; baseUrl?; value: Buffer }): string; list(): SecretMeta[]; remove(id: string): void; }
export class ModelGateway {
  withKey<A>(provider: string, sessionId: string, f: (key: string, baseUrl?: string) => Promise<A>): Promise<A>;
}
```
Decisions 22–24.

- [ ] **Step 1: Write failing tests:**
  - Rust: seal/open round trip; a flipped byte → `SECRET_CORRUPT`; a missing key file is created 0600 (unix) inside an already-private directory;
  - TS: `secrets.json` is 0600 and holds no plaintext (grep for the test key); `list()` has no value field; `withKey` refuses a non-running session (`PROVIDER_AUTH`) and audits the refusal; the first grant per session is audited once; the key never appears in thrown errors, logs captured during the test, or `JSON.stringify` of anything the gateway returns.
- [ ] **Step 2: Implement.** Plaintext buffers are zero-filled after `f` settles. Concurrent `put`s serialise through one in-process queue; the file is written temp-then-rename.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): encrypted secret store and model gateway`.

### Task 6: The adapter contract, the runtime engine and the fake provider — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/{adapter.ts, runtime.ts, fake.ts}`.
- Modify: `TASKS.md`.
- Test: `apps/finisherd/test/agent-runtime.test.ts`, `apps/finisherd/test/fixtures/fake-scripts/*.json`.

**Interfaces:**
```ts
export interface ProviderAdapter { readonly harness: Harness; detect(): Effect<Detection, FinError>;
  open(spec: SessionSpec): Effect<ProviderSession, FinError, Scope.Scope>; }
export interface ProviderSession { readonly events: Stream.Stream<ProviderEvent>; startTurn(i: TurnInput): Effect<void, FinError>;
  steer(i: UserInput): Effect<void, FinError>; interrupt(): Effect<void>; respond(id: string, a: RuntimeAnswer): Effect<void, FinError>;
  rollback(toTurn: number): Effect<void, FinError>; fork(atTurn: number): Effect<ProviderRef, FinError>; ref(): ProviderRef | null; }
export class Runtime {   // Effect service
  start(req: StartRequest, by: Principal): Effect<Session, FinError>;
  prompt(sessionId, text, by): Effect<Run, FinError>; steer(...); interrupt(...); resume(...); end(...);
  watch(sessionId, fromSeq?): Stream.Stream<WatchItem, FinError>;
}
// fake.ts: a script is a list of steps per turn: emit events, wait for a tool result, crash, emit late events after replace
export const FakeAdapter: (scripts: FakeScript[]) => ProviderAdapter;
```
Decisions 2, 3, 5, 6.

- [ ] **Step 1: Write failing tests** (all with `FakeAdapter`):
  - `events_become_nodes_in_order` (deltas streamed, completed nodes stored once);
  - `tool_call_gets_a_ulid_call_id` (announced before execution; `providerId` kept);
  - `late_events_from_replaced_instance_dropped` (arch §27.5);
  - `spawn_failure_fails_turn_provider_exited`;
  - `interrupt_keeps_branch_and_status_interrupted`;
  - `resume_after_crash_never_reruns_tool` (crash points: after announce, after the tool process started, after the commit, after the result; the commit count is 1 in every case);
  - `steer_is_queued_before_next_call`;
  - `watch_snapshot_then_live` (a watcher joining mid-run gets every node once);
  - `budget_stop_at_safe_point` (stop requested during a tool call takes effect after its result).
- [ ] **Step 2: Implement.** One fiber per live session consumes its `events` stream and applies them through `AgentStore`; RPC calls enqueue commands to that fiber, so per-session order is total.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): provider adapter contract, agent runtime and fake provider`.

### Task 7: Credentials, session branches and branch reach — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/{credentials.ts, reach.ts}`.
- Modify: 5A's `apps/finisherd/src/authorize.ts` (one call into `reach.ts` for agent principals with branches), `apps/finisherd/src/errors.ts` and `crates/fin/src/output.rs` (`BRANCH_NOT_ALLOWED` exit 7, `BRANCH_IN_USE` exit 4), `TASKS.md`.
- Test: `apps/finisherd/test/agent-credentials.test.ts`, `apps/finisherd/test/agent-reach.test.ts`.

**Interfaces:**
```ts
export const MODE_SCOPES: Record<Mode, readonly Scope[]>;     // spec §6.1, 7A subset (Decision 15.3)
export function mintForSession(s: Session, root: string | null): Effect<{ credentialId: string; path: string }, FinError>;
export function revokeForSession(s: Session): Effect<void>;
export class BranchReach { allowed(principal: Principal, projectId: string, rpc: string, payload: unknown): Effect<void, FinError> }
```
Decisions 15–18.

- [ ] **Step 1: Write failing tests:**
  - `start_creates_root_branch_and_credential` (branch named `agent/editor-<n>` from `main` head; credential file 0600 with the 5A format; principal scopes equal `MODE_SCOPES.draft`);
  - `plan_mode_has_no_branch_and_no_write_scope`;
  - `reach_matrix`: for an agent principal rooted at `br3`, every write RPC targeting `br3`, a branch it created under `br3`, `main` and a sibling session's branch → allowed, allowed, `BRANCH_NOT_ALLOWED`, `BRANCH_NOT_ALLOWED`; reads of any branch allowed; `history.merge {from: main, into: br3}` allowed, `{from: br3, into: main}` refused;
  - `branch_in_use` (a second live session on the same root);
  - `end_revokes_credential` (`CREDENTIAL_REVOKED` for the old file); `resume_keeps_credential` across a restart;
  - `no_human_scope_in_any_mode` (every `MODE_SCOPES` entry against 5A's human-only list).
- [ ] **Step 2: Implement.** Lineage comes from the engine's branch index through `branch.list` (cached per project; invalidated by `branch` events on `project.events`).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): session branches, agent credentials by mode and branch reach`.

### Task 8: The built-in loop and its tools; `FIN_CALL` idempotency in `fin` — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/builtin/{loop.ts, tools.ts, request.ts, fin-tool.ts, workspace-tools.ts}`, `crates/fin/src/agent_env.rs`.
- Modify: `apps/finisherd/src/env.ts` (`ChildEnvExtra` gains `FIN_CREDENTIAL`, `FIN_CALL`, `FIN_AUTHOR`), `crates/fin/src/main.rs` (call-site lines), `crates/fin/src/commands/builder.rs` (idem default), `TASKS.md`.
- Test: `apps/finisherd/test/agent-builtin-loop.test.ts`, `apps/finisherd/test/agent-tools.test.ts`, `crates/fin/tests/agent_env.rs`.

**Interfaces:**
```ts
export interface ModelClient {   // implemented per API in Task 9
  stream(req: ModelRequest, key: string, baseUrl?: string, signal?: AbortSignal): AsyncIterable<ModelStreamEvent>;
  readonly wireBody: (req: ModelRequest) => unknown;   // what is sent, for capture
}
export const BuiltinAdapter: (deps: { clients: Record<Api, ModelClient>; gateway: ModelGateway; catalogue: Catalogue;
  finPath: string; serverData: string }) => ProviderAdapter;
export function runFinTool(call: { callId: string; argv: string[]; stdin?: string }, env: SessionEnv): Promise<ToolOutcome>;
export function workspaceTool(name: "workspace_read" | "workspace_write" | "workspace_list", args: unknown, root: string): Promise<ToolOutcome>;
```
```rust
// agent_env.rs: FIN_CALL → default idem key for single writes; `<call>.<line>` for batch lines.
pub fn default_idem_key(cmd: &Command, explicit: Option<&str>, line: Option<usize>) -> Option<String>;
```
Decisions 6, 7, 10–14, 17.

- [ ] **Step 1: Write failing tests:**
  - loop (with a scripted `ModelClient`): `end_turn_stops`, `tool_round_trip`, `parallel_reads_serial_writes`, `per_run_caps` (60 calls, 200 tools), `result_truncation_head_tail_note`, `context_stop_at_85_percent`, `retry_on_429_with_retry_after` (injected clock; same request body re-sent; no tool re-run);
  - `fin` tool: `argv_without_shell` (an argument `; rm -rf x` reaches `fin` verbatim and fails as usage), `env_is_allow_list_plus_session_vars` (no parent variable leaks; `FIN_TRAINING` absent), `timeout_kills_tree`, `json_stdout_parsed`, `exit_and_stderr_tail`;
  - workspace tools: `rejects_absolute_and_dotdot`, `rejects_symlink_escape` (a link to `<server-data>/auth/local.key`), `case_folding_on_windows`, `size_caps`, `write_is_temp_then_rename`;
  - `fin`: `fin_call_becomes_idem_key_for_writes`, `batch_lines_get_suffixed_keys`, `explicit_idem_key_wins`, `no_fin_call_no_change`, `reads_ignore_fin_call`.
- [ ] **Step 2: Implement.** The loop is an adapter like any other: it emits `ProviderEvent`s, including `capture {request, response}` per model call, and calls tools through the runtime's tool executor so the runtime sees `toolStarted`/`toolCompleted` with the call id it minted.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): built-in agent loop with a native fin tool and confined workspace tools`.

### Task 9: Model API adapters and the fake model server — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/agent/builtin/{anthropic.ts, openai.ts, compat.ts}`, `apps/finisherd/test/support/fake-model-server.ts`, `apps/finisherd/test/fixtures/model-streams/{anthropic,responses,chat}/*.sse`.
- Modify: `apps/finisherd/package.json` (`@anthropic-ai/sdk` 1.6.0, `openai` 7.53.0, exact pins), `pnpm-lock.yaml`, `TASKS.md`.
- Test: `apps/finisherd/test/agent-model-clients.test.ts`.

**Interfaces:** `anthropicClient`, `openaiResponsesClient`, `chatCompatClient`: each a `ModelClient` (Task 8). `startFakeModelServer(scripts): Promise<{ baseUrl; requests(): RecordedRequest[]; close() }>` on `127.0.0.1:0`. Decisions 7–9.

- [ ] **Step 1: Write failing tests** per API, against the fake server: text streaming; a tool call split across deltas; two parallel tool calls; usage incl. cache read/write and reasoning tokens; Anthropic thinking blocks sent back unchanged on the next call; Responses `store: false` and encrypted reasoning carried; Chat `include_usage`; 429 with `retry-after`, 529, a mid-stream disconnect (one retry, a `notice`); a malformed event → `PROVIDER_PROTOCOL`; 401 → `PROVIDER_AUTH`; context-length error → `MODEL_CONTEXT_EXCEEDED`; **`wire_body_equals_recorded_request`** (the capture is the exact bytes the server received, modulo JSON key order, which the client fixes).
- [ ] **Step 2: Implement.** SDK clients are created per request with the key from `ModelGateway.withKey`, `maxRetries: 0` (the loop owns retries) and the catalogue's `baseUrl` for compatible endpoints.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): Anthropic, OpenAI Responses and OpenAI-compatible model clients`.

### Task 10: Journal emission and attribution — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/journal.ts`.
- Modify: `crates/fin-node/src/journal.rs` (napi `attachBlob(bytes, mediaType)` if D4 did not add it), `apps/finisherd/src/agent/runtime.ts` (call sites), 5A's `authorize.ts` (an agent principal's `call` header is accepted and passed to the engine as the actor's call; spec §12.3), `TASKS.md`.
- Test: `apps/finisherd/test/agent-journal.test.ts`, `crates/fin-node/tests/journal_blob.rs`.

**Interfaces:**
```ts
export class AgentJournal {
  constructor(journal: NapiJournal, ids: Ids);
  session(s: Session, phase: SessionPhase, totals?: SessionTotals): void;
  turn(t: TurnRecord): void;            // builds content only when the gate has training
  toolCall(c: ToolCallRecord): void; toolResult(r: ToolResultRecord): void;
  approval(a: ApprovalRecord): void; verdict(v: VerdictRecord): void;
}
```
Decisions 25, 26.

- [ ] **Step 1: Write failing tests** (a real `fin-node` journal in a temp server data):
  - `off_means_off` (both purposes off: no `events/` directory after a whole scripted session);
  - `diagnostics_only_has_no_content_or_blobs`;
  - `training_records_exact_request_and_response` (`capture: exact`; the request blob equals the fake server's recorded body);
  - `system_and_tools_blobs_deduplicate` (two sessions with one role → one blob each, refcount 2; purging one session's entries keeps the blob);
  - `large_tool_output_is_a_blob`, `over_4_mib_truncated`;
  - `collect_never_drops_everything_for_that_project`;
  - **`call_join`** (every `log.commit` of a built-in write has `ctx.actor.call` = the `agent.toolCall.callId`, and the `agent.toolResult.txIds` are exactly those commits, `attribution: exact`);
  - `agent_header_cannot_claim_another_session` (5A rule: the session comes from the principal; only `call` from the header).
- [ ] **Step 2: Implement.** Payloads are built after the gate check; content is moved, not cloned, into the payload where possible.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): record sessions, turns, tool calls and results in the journal, joined by FIN_CALL`.

### Task 11: The verdict tracker — tier: opus

**Files:**
- Create: `apps/finisherd/src/agent/verdicts.ts`.
- Modify: `apps/finisherd/src/history.ts` (publish typed results to the tracker; call-site lines), `crates/fin-node/src/history.rs` and the contracts' history result schemas only if a result lacks the txIds it undid or copied (additive fields, Decision 27), `TASKS.md`.
- Test: `apps/finisherd/test/agent-verdicts.test.ts`.

**Interfaces:** `class VerdictTracker { observeHistory(rpc, payload, result, principal): void; observeBranch(event): void; observeSessionDeleted(s): void }`. Decision 27.

- [ ] **Step 1: Write failing tests** against a real engine: an agent session commits three txs; a person merges its branch into `main` → three `merged` by `user`; the agent undoes one of its own → `undone` by `agent`; a person reverts one on `main` → `reverted` by `user`; a second session archived unmerged → `abandoned`; a cherry-pick of one tx into `main` → `merged` (a copy into a branch outside the subtree); merging a child branch into its own session's root → no verdict (inside the subtree); idempotent under repeated events; `afterS` from the injected clock.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): edit verdicts from merges, undos, reverts and abandoned branches`.

### Task 12: RPC handlers, console commands and start-up wiring — tier: sonnet

**Files:**
- Create: `apps/finisherd/src/agent/{rpc.ts, console.ts}`, `apps/finisherd/src/secrets/console.ts`.
- Modify: `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/main.ts` (layers; `recoverAfterCrash` before the transports listen; the hourly sweep), 5A's console dispatcher, `TASKS.md`.
- Test: `apps/finisherd/test/agent-rpc.test.ts`, `apps/finisherd/test/agent-console.test.ts`.

**Interfaces:** console: `finisherd agent start --project <dir|id> [--model] [--role] [--mode] [--base] [--title] [--prompt <text> | --prompt-file <f>] [--budget-usd] [--budget-tokens]`, `agent ls [--project] [--status]`, `agent show <id> [--nodes] [--json]`, `agent prompt|steer <id> <text>`, `agent interrupt|resume|end <id>`, `agent watch <id>`, `agent approve <id> allow|deny [--add <amount>]`, `agent delete <id>`; `provider key set|ls|rm` (Decision 24). Text output for people, `--json` for scripts; errors as sp5 §4.6.

- [ ] **Step 1: Write failing tests:** each RPC's happy path and its codes (`SESSION_NOT_FOUND`, `SESSION_BUSY`, `SESSION_ENDED`, `MODE_INVALID`, `MODEL_UNKNOWN`, `HARNESS_UNKNOWN`); `agent.providers` lists `builtin` with the providers that have a key and `fake` only in test builds; console `agent watch` renders nodes as lines and exits when the session ends; `provider key set` reads stdin and refuses an argv value; start-up recovery runs before the first connection is accepted.
- [ ] **Step 2: Implement.** The `fake` harness is registered only when `FIN_TEST_HOOKS=1` and the build has test hooks (5A's `check-test-hooks` gate covers it).
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): agent RPCs and console commands`.

### Task 13: e2e `agent` scenario — tier: sonnet

**Files:**
- Create: `scripts/e2e/agent.mjs`, `scripts/e2e/fixtures/agent-streams/*.sse`.
- Modify: `scripts/e2e.mjs` (register), `TASKS.md`.

- [ ] **Step 1: Write the scenario.** `finisherd` with `FIN_TRAINING=on` (its own env) and the fake model server; a project from the reel fixtures:
  1. `finisherd provider key set anthropic` (a dummy key on stdin) and `agent start --mode draft --prompt "tighten the intro"`;
  2. the scripted model calls `fin view --budget 2000`, then `fin item trim …`, then `fin caption import …`, then ends;
  3. check: the session branch has two commits by `agent:editor:<id>`; `main` is untouched; `fin` with the agent credential cannot commit to `main` (`BRANCH_NOT_ALLOWED`, exit 7);
  4. `finisherd diagnostics export --content --kind agent.toolCall --kind log.commit` (content export is owner-only, sp5 §4.5): the call join of Task 10 holds;
  5. kill `finisherd` during the third tool call; restart; the session is `interrupted`; `agent resume`; the commit count is unchanged and the run completes;
  6. `fin merge --from <branch>` as `local`; `agent.editVerdict merged` for both txs;
  7. a second session with a 1-cent budget stops with `BUDGET_EXCEEDED` and a budget approval; `agent approve <id> allow --add 1` resumes it.
- [ ] **Step 2: Run** `rtk pnpm e2e` on Linux → PASS. **Commit** `test(e2e): agent session from prompt to merged verdict with crash and budget`.

### Task 14: Spec, architecture pointer and tracker — tier: sonnet

**Files:** Modify: `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (§3–5, §10, §12, §13 rewritten "as built" for 7A; status line), `docs/specs/2026-11-01-finisher-architecture-design.md` (a §27 item pointing to sub-project 7 §1–13 for 7A, and replacing §21.1's "Reserved (v0) for sub-project 7" list by a pointer), `README.md` (running an agent from the console), `TASKS.md` ("Follow-ups for 7B/7C" from this plan's leftovers).

- [ ] **Step 1: Write** the as-built sections from the code (names, defaults, codes), and the follow-ups.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 7A as built`.

### Task 15: Cross-platform validation with real model APIs (controller-run; needs hosts and keys) — tier: opus (controller)

- [ ] **Step 1:** On macOS, Windows 11 (as a non-elevated user) and the Linux VM: `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e`.
- [ ] **Step 2:** With the owner's keys, one real session per API (Anthropic, OpenAI Responses, one compatible local endpoint) on the reel project: the edit lands on the session branch, the turn's `costMicros` matches the provider's usage page within rounding, caching shows cache reads on the second call, and the journal's call join holds. Record measured latencies and costs in `archive/` and only the conclusions in the spec.
- [ ] **Step 3:** Record results in TASKS.md; fix waves as needed; final whole-branch review. **Commit** `docs: 7A validation results`.
