# Sub-project 7A: Runtime core and the built-in agent — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the event kinds and their classes, the session state machine and crash recovery, credentials and branch reach, secrets, the built-in loop and its tools, attribution, outcomes), `sonnet` for plumbing (contracts, the catalogue, API adapters against the fake server, RPC and console wiring, e2e, docs).

**Revision 2 (2026-11-07).** Answers the independent review of the spec (`docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md`, §23). 7A is **split in two**: **7A-1** (pure pieces; runs before 5A) and **7A-2** (everything that needs 5A). Every task below is tagged `[7A-1]` or `[7A-2]`; Tasks 3, 6 and 8 have an `a` part in 7A-1 and a `b` part in 7A-2.

**Revision 3 (2026-11-07).** Applies the owner's decisions (spec §20): the built-in agent talks to **OpenRouter**, through one OpenAI-compatible Chat Completions adapter (the direct Anthropic Messages and OpenAI Responses clients are removed; `@anthropic-ai/sdk` is dropped; revision 4 adds a local preset of the same adapter); the only secret Finisher stores is the **OpenRouter key the user pastes**, optionally per project; every session and turn record carries **model provenance** (harness, provider, model, weights class, upstream) because closed-model outputs may become training targets behind a counsel gate, which 7A defines (default `pendingCounsel`); spike S9's B5, B7 and H2 checks follow the kernel 6.2 floor and the user's own CLI sign-in, never copied. The toolchain is **Node 24**.

**Revision 4 (2026-11-07, later).** Applies two follow-up decisions (spec §20, F1 and F2). **F1:** the built-in agent's one adapter is an **OpenAI-compatible Chat Completions client with an OpenRouter preset and a local preset** (Ollama, LM Studio or a user-supplied base URL): a local URL needs no API key and the `SecretStore` is untouched by it; loopback is allowed for the built-in agent only, because it runs in `finisherd` and no sandbox egress rule changes; diagnostics record `provider: local` with the model as a table family name or a keyed hash; weights are `open` or `unknown`, never `closed`. The OpenRouter client of Task 9 generalises accordingly (Decisions 8, 15, 19, 20, 22–24; Tasks 1, 4, 5, 9, 10, 12, 13, 15). **F2:** where Landlock is disabled on a supported kernel every profile runs, with a persistent `SANDBOX_DISABLED` warning and a diagnostics event; the "refused unless owner override" rule and `agents.unsandboxed` are gone; Task 0's B5 checks the warn-and-run path. 7A itself never refuses a session for lack of a sandbox (the built-in agent has none).

**Goal:** an agent can edit a Finisher project on its own branch, driven by Finisher's own loop on the user's OpenRouter key or a local model server, with every turn, tool call and result recorded under D34 (content in a dedicated agent-trace store) and joined to its commits. After 7A-1 + 7A-2:
- sessions, runs and nodes are persisted in `control.db`, become `interrupted` on a crash and resume without re-running a tool;
- one adapter contract (`ProviderAdapter`) with two implementations: the scripted `fake` harness and the `builtin` loop over **OpenRouter or a local server**, through one OpenAI-compatible Chat Completions client with two presets;
- the built-in agent's tools are a native `fin` tool (argv, no shell) and three workspace file tools confined to `workspace/<sessionId>/`;
- each session gets its own branch and a minted `agent` credential **whose id is the session id**, whose scopes come from its mode (`plan`, `draft`, `auto`; never `project.create` or `project.open`) and whose writes are confined to its branch subtree; **`fin` under that credential writes the session's branch by default, keeps its `try` state per session, never falls back to the local key, and reads only files under the workspace and the project folder**;
- every agent commit is recorded in `session_txs`, and `Tx.author` is set by the server;
- a bundled model catalogue (OpenRouter slugs with maker and weights class, and the CLIs' model ids) prices every turn, beside a small `local-models.json` table that names well-known open-weight families for local sessions (which cost nothing and are not priced); token, money, wall-clock and call budgets stop a run at a safe point;
- the OpenRouter key (per server, optionally per project) lives in a 0600 `SecretStore` (with a `seal`/`open` seam for the keychain) and reaches a model call only through `ModelGateway`; Finisher stores no other model credential, and a local endpoint is a setting with no key;
- the agent kinds are version 1 (`agent.session`, `agent.turn`, `agent.toolCall`, `agent.toolResult`, the new `agent.approval`; `agent.editVerdict` is retired), `FIN_CALL` is set exactly for every built-in tool call, every session and turn carries its provenance (harness, provider, model, weights class, upstream), content goes to the agent-trace store as deltas plus a full-body hash, and outcomes (merged, undone, reverted, abandoned) are tracked and emitted as 5E's `review.verdict`;
- `finisherd agent …` and `finisherd provider key …` console commands drive all of it for people.

**Architecture:**
- **`fin-core::events`:** the v1 payloads (with provenance on session and turn), `agent.approval`, the tables `AGENT_ROLES` and `AGENT_TOOLS`, `AGENT_MODELS` grown to the catalogue, `openrouter` and `local` in `AGENT_PROVIDERS`, `Harness` and `Weights` enums. `agent.editVerdict` removed.
- **`apps/finisherd/src/agent/`** (new): `store.ts` (migration and queries), `session.ts` (pure state machine), `adapter.ts` (contract and events), `runtime.ts` (runs, events → nodes, crash recovery), `fake.ts`, `builtin/{loop.ts, tools.ts, compat.ts (the OpenAI-compatible Chat Completions client: OpenRouter and local presets), request.ts, fin-tool.ts, workspace-tools.ts}`, `catalogue/{models.json, local-models.json, catalogue.ts}`, `budget.ts`, `credentials.ts`, `reach.ts`, `journal.ts`, `outcomes.ts`, `rpc.ts`, `console.ts`.
- **`apps/finisherd/src/secrets/`** (new): `store.ts` (with `SecretBox`), `gateway.ts`. **No Rust**: the encrypted file is cut.
- **`@finisher/contracts`:** `agent-events.ts` (`ProviderEvent` and the runtime enums; 7A-1) and `agent.ts` (RPC group, `Session`, `Run`, `Node`; 7A-2), and the new scopes.
- **`fin`:** the default idempotency key from `FIN_CALL` (7A-1); under an agent credential, the session-root default branch, per-session try state, no local-key fallback, confined path arguments and a server-set author (7A-2).
- **`fin-node`:** a second `Journal` instance for the agent-trace store (options for directory, cap and retention) and `attachBlob` on the napi `Journal` if D4 did not expose it.

**Tech Stack:** Node 24 (the repo moved from Node 26 to 24.22.2 on 2026-11-08 to match Electron's embedded Node; `node:sqlite` verified there (TASKS.md)), Effect 4.0.6, `node:sqlite` (through 5A's `ControlStore`, 7A-2), `openai` 7.53.0 (7.47.0 is pinned until 7.53.0 is ≥7 days old, see TASKS.md; Apache-2.0; Chat Completions transport with OpenRouter's or a local server's base URL), fast-check, vitest; Rust 2024, `schemars` 1. No other new dependencies (`chacha20poly1305` and `@anthropic-ai/sdk` are not used); `pnpm gates` runs the licence gates over these.

**Spec:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1–5, §6.1, §10, §12, §13, §15, §16 (7A parts), §19–20, §23. Parent: arch §8, §9, §16, §21.1. Related: the diagnostics plan Decisions 3, 14b, 15–20, 23a, 24, 28, 34; the sub-project 5 design (revision 3) §3.2–3.5, §4.1–4.6 (principals, the agent credential, scopes, human-only, exit 7) and its 5A plan.

**Branches:**
- **7A-1: `sp7a1-pure`**, stacked on the branch that holds **D1–D7** (the 4C branch) and **independent of 5A**. It must not import from 5A, `control.db` or the authorisation wrapper; persistence is behind interfaces with in-memory implementations (`LedgerStore`, `AuditSink`).
- **7A-2: `sp7a2-runtime`**, stacked on 7A-1 **and** on the branch that holds 5A (Tasks 1–12): it calls 5A's `ControlStore`, `Audit`, the agent-credential mint service (with a caller-supplied id) and the authorisation wrapper. The `control.db` migration is a new chain `agent` (sp5 §3.1), so it never collides with 5A, 5C or 5E.
- **7C** (the `fin-sandbox` policy, launcher, profiles and suites) **can start now** on its own branch and does not wait for either; only its wiring into sessions waits for 7A-2. **Task 0 below** (spike S9) can run first.

**Out of scope:** session rollback and (cut) session fork, subagents, roles beyond `editor` and `critic` as data stubs, saved prompts, command-pattern rules, approvals answered by people (the `agent.approval` kind and table ship; only `budget` approvals are created, answered from the console), review requests and review verdicts from 5E itself (7B), context compaction (7B); the OS sandbox and the agent listener (7C); Claude Code (7D) and Codex (7E); hooks (7F); workspace artifacts and the rest of the full `fin` (7G); the eval harness (7H); the keychain backend (with 5B); any UI (9).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code; `pnpm gates` runs cargo-deny and the JS licence gate.
- **No cloud-only imports** (the import-graph guard).
- **Model and event types are generated, never hand-written:** every payload change derives `JsonSchema` and goes through `crates/fin-store/examples/export_schema.rs` and `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). Runtime types that are TS-owned control-plane state (`Session`, `Run`, `Node`, `ProviderEvent`) are Effect Schemas in `@finisher/contracts`, like sub-project 5's.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. `apps/finisherd/src/projects.ts` (520) and `main.ts` take call-site lines only; every agent file is new.
- **No secret in the environment, argv, a log, the journal or a project folder.** The OpenRouter key enters by stdin to the console, live in the 0600 secrets file in server data, and are read only inside `ModelGateway` for one request.
- **Off means off** (D34): every emitter checks the journal gate before it builds a payload; nothing reaches disk for a project that is `never` or unknown.
- Errors are `{ code, message, details, fix? }`; new codes go into `crates/fin/src/output.rs` (exit classes, including the new class 8 for `APPROVAL_PENDING`) and `apps/finisherd/src/errors.ts`.

**Testing and process**
- **Tests never use the network.** The client, in both presets, runs against the loopback fake model server (Task 9).
- Clocks and id makers are injected (`Clock`, `Ids`) in every runtime module, so timeouts, budgets and retention are tested without sleeping.
- Crash tests spawn `finisherd` as a child and kill it (`SIGKILL`; `TerminateProcess` on Windows), as the 4A lease tests do.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch `fin` or `finisherd` wiring also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures and scripted transcripts included.
- **Spikes and scratch work live in `archive/`** (git-ignored; AGENTS.md); only durable conclusions go into the spec.
- **Tracker:** each task ticks its line in `TASKS.md` under a new heading "Sub-project 7A: runtime core" (7A-1 and 7A-2 subheadings), in the same commit.

## Decisions taken here (the spec fixes the shape; these fix the details)

### Store and state

1. **Tables** (one migration in the new `agent` chain, 7A-2):
   - `sessions {id, projectId, harness, provider, model, role, mode, rootBranch?, base {branch, seq}, parent?, status, statusReason?, credentialId (= id), providerRef?, budget JSON, createdBy JSON, createdAt, endedAt?, title?}`;
   - `runs {id, sessionId, seq, trigger, status, startedAt, endedAt?, headAtStart?, stopReason?, usage JSON}`;
   - `nodes {id, runId, sessionId, seq, kind, providerId?, payload JSON | blobRef, createdAt}` with an index on `(sessionId, seq)`;
   - **`session_txs {txId, sessionId, callId?, attribution, at, outcome?}`**: **one row for every commit, undo, redo, revert, cherry-pick and merge an agent principal makes**, keyed by the principal's id (= session id), written by the authorisation wrapper's after stage whatever the call attribution; `callId` is null when unknown (spec §5.2, §12.3);
   - `session_state {sessionId, defaultBranch, tries JSON}`: the per-session default branch and `fin try` list (spec §5.1);
   - `approvals {id, sessionId, kind, subject JSON, argsHash, requestedAt, expiresAt, decision?, by?, answeredAt?, consumedAt?}`;
   - `budgets {sessionId, kind, limit, used, reserved}`.
   **Node payloads up to 4 KiB are inline**; larger ones are written to `<server-data>/agents/<sessionId>/blobs/<blake3-hex>` (temp-then-rename) first, then referenced.
2. **The state machine** (`session.ts`, 7A-1) is a pure function `next(state, event) → {state, effects}` over the statuses of spec §3.2, tested by properties. Persistence (7A-2) applies its effects in one `ControlStore.tx`.
3. **Crash recovery** at start: every `starting|running|waitingApproval` session → `interrupted` (`serverRestart`); every open run → `interrupted`; an open `toolCall` node without a result gets a synthetic `toolResult {ok: false, code: SESSION_INTERRUPTED}`.
4. **Retention:** product state of `ended` sessions older than `agents.retentionDays` (default 90) is swept hourly with their blob directories. `agent.session.delete` (owner) removes one at once. Diagnostics purge and export never touch these tables.

### Providers and the built-in loop

5. **`ProviderEvent`** is the closed union of spec §4.1 (no `fork`). Adapters emit only events; `runtime.ts` alone writes nodes, budgets and journal entries, in event order per session.
6. **One active run per session.** `prompt` while running is `SESSION_BUSY`; `steer` on the built-in loop queues the text as a user message appended before the next model call (the loop has no mid-stream injection).
7. **Requests are built by Finisher** (`builtin/request.ts`, 7A-1): one `ModelRequest {system, tools, messages, model, maxTokens, temperature?, reasoning?}` in Finisher's own neutral form, translated per API. The translated body is what the `capture` event records, byte for byte as sent, with `bodyHash` = BLAKE3 of those bytes.
8. **One adapter, two presets, both Chat Completions** (spec §4.4; Owner decision 7 and F1). **OpenRouter preset:** base URL from the catalogue (`openrouterBaseUrl`), streamed; `stream_options.include_usage` and OpenRouter's usage accounting on every request, so the response carries tokens, cache tokens, reasoning tokens and the **cost OpenRouter charged** (`costBasis: reported`) and the **upstream provider** that served it (`upstream`); function tools; reasoning through OpenRouter's `reasoning` parameter, its returned reasoning details kept and sent back unchanged; `cache_control: {type: "ephemeral"}` breakpoints on the last system block, the tool list and the last stable message for models the catalogue marks `caching: explicit`, none otherwise. A model without tool support is refused at start (`MODEL_UNKNOWN` with `details.reason: noTools`). Task 9 re-reads OpenRouter's API documentation on the day and records any difference in the commit body. There are no Anthropic Messages or OpenAI Responses clients in v1. **Local preset (F1):** the base URL is the owner's setting `agents.local.baseUrl` (the `ollama` and `lmstudio` presets default to those servers' usual loopback ports), validated when set (`http` or `https`, a host, no credentials in the URL) and settable only by the owner (`agent.admin`, human-only), never by an agent, a project file or a plugin. **No API key:** the client sends a fixed placeholder as the bearer value, there is no `SecretStore` entry and `ModelGateway` returns no key. `stream_options.include_usage` where the server supports it, else tokens are estimated (characters ÷ 4, `usageEstimated`); `costBasis: unknown` and no money charged; no `cache_control`, no reasoning parameter, no `upstream`; the window comes from `local-models.json` or the owner's entry (default 8,192); a server that rejects tools gives `MODEL_UNKNOWN` with `details.reason: noTools` on the first call; an unreachable server is `PROVIDER_UNAVAILABLE {provider: local}`; redirects to another origin are not followed. **Loopback is allowed for the built-in agent only:** the loop runs in `finisherd`, so no sandbox egress rule changes (CLI agents, hooks and plugins gain no loopback reach).
9. **Retries** (built-in only): 408, 429, 500, 502, 503, 504 and connection resets; `retry-after` (seconds or HTTP date) or 1 s × 2ⁿ, ≤ 60 s, 5 attempts; each retry is a `notice` node. A retry never re-runs a tool: it re-sends the same model request.
10. **Caps per run:** 60 model calls, 200 tool calls, 16 KiB of each tool result shown to the model (head 12 KiB + tail 4 KiB + a note naming the node), context stop at 85 % of the catalogue's window (`MODEL_CONTEXT_EXCEEDED`, run ends, session `idle`).

### The built-in tools and `fin`

11. **`fin {argv: string[], stdin?: string}`:**
    - runs the `fin` binary (`agents.finPath`, default: next to `finisherd`'s bundled binaries; in development `target/<profile>/fin`) with `execFile` semantics: no shell, argv as given, `cwd` = `workspace/<sessionId>/`;
    - environment: `childEnv({FIN_SERVER, FIN_SESSION})` plus `FIN_CREDENTIAL` and `FIN_CALL` (Decision 14); **`FIN_SERVER_DATA` and `FIN_AUTHOR` are deleted from the built-in tool's environment** (spec §8.5); `FIN_SERVER` is the normal local socket until 7C's agent listener;
    - timeout 120 s (`agents.toolTimeoutS`), then kill the process tree; stdout is the result (JSON when `fin` printed JSON), with the exit code and the last 1 KiB of stderr;
    - `argv[0]` is a command word, never a path; `fin` itself refuses anything an agent principal may not do (scopes, `HUMAN_ONLY`, branch reach, path reach), so the tool needs no command filter of its own beyond the client-side path check of Decision 14c.
12. **`workspace_read {path, offset?, limit?}`, `workspace_write {path, content, mode?: create|overwrite|append}`, `workspace_list {path?}`:** paths are relative, normalised, never absolute, never `..`; the real path (symlinks resolved, case-folded on Windows and on case-insensitive macOS volumes) must stay under `workspace/<sessionId>/` (`PATH_NOT_ALLOWED`, exit 7); reads ≤ 256 KiB per call, writes ≤ 8 MiB per file and 256 MiB per session; writes are temp-then-rename (arch §6.7).
13. **Tool definitions** are static JSON in `builtin/tools.ts`; their hash is `toolsHash`. **The system prompt is the role's prompt + the `fin` map (from `fin` with no arguments, captured once per `fin` version) only; its hash is `systemHash`, and it is identical across sessions of a role.** **Budget facts, the project, branch and mode (`fin context`-like facts) go in a user-role message at the head of each run**, never in the system prompt (spec §4.4).
14. **`FIN_CALL` and idempotency (7A-1):** when `FIN_CALL` is set and a write command has no `--idem-key`, `fin` uses `FIN_CALL` as the key (`batch` lines use `<FIN_CALL>.<line>`). A built-in write's txId therefore joins its tool call directly, and a retried call after a crash is a dedupe (spec §3.4). A user's `fin` without `FIN_CALL` is unchanged.
    - **14a (7A-2). Under an agent credential, `fin`'s default branch is the session's root, taken from the principal** (`auth.whoami` carries `branches` and `defaultBranch`); **try state is per session on the server** (`session_state`, through `agent.self` and `agent.self.try`) and **`fin` neither reads nor writes `fin-config.json`** (`commands/mod.rs:78-80`, `config.rs:24-30`, `try_cmd.rs:73-74`). `fin try` records its branch (must be in the subtree) and sets the session default; aborting restores the previous one.
    - **14b (7A-2). With `FIN_CREDENTIAL` set, `fin` never falls back to the local key** and ignores `FIN_SERVER_DATA` (endpoint from `FIN_SERVER` alone); a missing, expired or revoked credential file is `UNAUTHENTICATED` (exit 7). The built-in tool also deletes `FIN_SERVER_DATA` from its environment.
    - **14c (7A-2). Client-side file arguments are confined:** with an agent credential `fin` makes every path it reads itself absolute, resolves symlinks (case-folded on Windows and case-insensitive macOS) and requires it to lie under `workspace/<sessionId>/` or the project folder, else `PATH_NOT_ALLOWED` before the file is opened (`caption import --words`, `asset add`, `apply` files…). The registry marks such arguments (`argKinds: path`) and a drift test lists them.
    - **14d (7A-2). `Tx.author` is set by the server for agent principals**; `FIN_AUTHOR` and `fin-author` headers are ignored for them.

### Sessions, branches and credentials

15. **Start** (`agent.session.start`, scope `agent.run`, human principals only in 7A):
    1. resolve role (`editor` default), mode (`draft` default, ≤ the role's ceiling), harness (`builtin`), preset (`openrouter` by default, or `local`: the `provider` parameter of `agent.session.start`, `--provider` on the console), model (role default for the harness, or given; for `openrouter` it must be in the catalogue or an OpenRouter slug the owner added in `agents.models`; for `local` a name the server lists, a `local-models.json` family or one the owner added); for the OpenRouter preset the project's key, else the server's default, must exist (`PROVIDER_AUTH` otherwise); for the local preset the endpoint must be set (`PROVIDER_UNAVAILABLE` otherwise) and no key is looked up;
    2. unless `plan`: create the root branch from `base` (default `main` head) named `agent/<role>-<n>` through `branch.create` as the starter's principal;
    3. mint the agent credential through 5A's service **with `id` = the session id** (so the principal id is the session id): scopes from the mode table (spec §6.1, minus `jobs.submit`, which needs 5C, and `review.write`, which needs 5E: both added when those land; **never `project.create`, `project.open`**, any human-only scope, `branch.protected` or `review.decide`), `projects: [id]`, `branches: [root]`, expiry beyond the session's life;
    4. write `<server-data>/agents/<sessionId>/credential.json` (0600; temp-then-rename) and create `tmp/`, `git/` and `provider/`;
    5. record the session `idle` and its `session_state` (default branch = root), then start the first run when `prompt` is given.
16. **Branch reach and path reach** (`reach.ts`), checked in 5A's authorisation wrapper for `agent` principals: the target branch of `tx.commit`, `history.undo|redo|checkpoint|cherryPick|revert`, the `into` of `history.merge`, the branch of `branch.set`, and the parent of `branch.create` must be the root or a descendant of it (lineage from the engine's branch index, cached per project and refreshed on `branch` records). Otherwise `BRANCH_NOT_ALLOWED` (exit 7), audited as `auth.denied`. Reads are not restricted by branch. **Paths** resolve against the principal's roots: the project folder, `workspace/<sessionId>/` and the roots of libraries the project references; anything else is `PATH_NOT_ALLOWED`; a `project.open` or `project.create` is refused.
17. **`Tx.author`** for an agent's commits is `agent:<role>:<sessionId>`, **set by the server** (Decision 14d), never by `FIN_AUTHOR`.
18. **End** revokes the credential and sets `endedAt`; the branch stays. **Delete** (owner) ends first, then removes product state and the session directory. **Resume** refreshes the credential's expiry.

### Budgets and the catalogue

19. **Catalogue** (`catalogue/models.json`): `{asOf, openrouterBaseUrl, models: [{id, harness: builtin|claudeCode|codex, provider: openrouter|anthropic|openai, maker, weights: open|closed, variant?, window, maxOutput, capabilities: {tools, thinking, images, caching: explicit|automatic|none}, price: {input, output, cacheRead, cacheWrite} per MTok in micros}]}`. Built-in ids are OpenRouter slugs (`<maker>/<model>`; a `:free`-style suffix goes in `variant`, since `:` is not a name character). Task 4 fills the OpenRouter models the owner names, **at least one closed and one open-weight**, with prices from OpenRouter's models page on the day, and the model ids Claude Code and Codex report (`claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5`, `claude-haiku-5-5`, and the Codex models at the pinned version) for pricing CLI usage; `asOf` set; tests use a separate test catalogue. The owner may add OpenRouter slugs in settings (`agents.models: [{id, weights?, window}]`); such a model is recorded as `other` plus a keyed hash, with `weights: unknown` unless given, and priced from OpenRouter's reported cost. **Local models** are not in `models.json`: `local-models.json` (`{asOf, models: [{name, prefix, maker, weights: "open", window, tools}]}`) names well-known open-weight families by tag prefix; `Catalogue.local(name, extra)` takes the longest matching prefix and records the family `name` as the model (`weights: open`), else records `other` plus a keyed hash of the typed name with `weights: unknown` unless the owner sets `open` in `agents.models: [{id, provider: "local", weights?, window?}]`. A local model is never `closed`.
20. **Default budgets per session** (settings `agents.defaultBudget`): 2,000,000 tokens, 10 USD, 60 minutes of wall clock with a run active, the per-run caps of Decision 10. Per server per day: 50 USD across built-in (OpenRouter) sessions (`agents.dailyMoneyMicros`; local sessions cost nothing and are not counted). The role may lower them; the starter may set them.
21. **Enforcement points:** before each model call (projected: `used + maxTokens × output price` must fit) and after each turn and tool call. At ≥ 80 % a `notice` node (once per kind); at ≥ 100 % the run stops after the running tool call, the session becomes `interrupted` with `BUDGET_EXCEEDED`, and a `budget` approval is created (`finisherd agent approve <id> allow --add <amount>` raises the limit and resumes). The ledger is behind a `LedgerStore` interface (in-memory in 7A-1; `control.db` in 7A-2).

### Secrets

22. **`SecretStore` file backend** (7A-1, pure TS): `<server-data>/secrets.json` (mode 0600, in the already-private server-data directory; entries `{id, provider: "openrouter", label, projectId?, sealed, createdAt}`). **The OpenRouter key is the only secret it holds in v1** (spec §10): no Anthropic or OpenAI key, nothing of a CLI's sign-in, and nothing for a local endpoint (it needs no key). **A `SecretBox` interface `{ seal(plaintext: Buffer): Sealed; open(s: Sealed): Buffer }` is the seam for the keychain backend (5B); the file backend's box is the identity**: no cipher, no key file (XChaCha20-Poly1305 with a key beside the ciphertext protects nothing the 0600 directory and the sandbox's read denial do not already, so it is cut; spec §10). Writes are temp-then-rename, serialised through one in-process queue.
23. **`ModelGateway.withKey(projectId, sessionId, f)`** picks the project's OpenRouter key, else the server's default (Owner decision 5: per-project keys, each with its own credit limit at OpenRouter), opens the entry, calls `f(key)` for one request, and drops the buffer (overwritten). It refuses a session that is not `running` (`PROVIDER_AUTH`). The first grant per session and every refusal go to an `AuditSink` interface (5A's `Audit` in 7A-2; the `grant` kind).
24. **Console only:** `finisherd provider key set openrouter [--project <dir|id>] [--label]` reads the key from stdin (never argv), `provider key ls` shows ids, labels, projects and providers (never values), `provider key rm <id>`. Any provider other than `openrouter` is `PROVIDER_UNAVAILABLE` in v1 (`provider key set local` is refused with a fix pointing at the next command). **The local endpoint** is set by `finisherd provider local set <ollama|lmstudio|url>`, shown by `provider local show`, checked by `provider local test` (it lists the server's models through `GET /models` where offered), removed by `provider local rm`: owner-only, no secret read or written, and no record holds the URL's host or port. CLI sign-in (`provider signin`) belongs to 7D/7E and runs nothing (spec §8.6). RPCs for the UI come with sub-project 9; none returns a value.

### Journal and the agent-trace store

25. **Emission** (`journal.ts`) through `fin-node`'s napi `Journal` with `source: agent`, `session` = the `finisherd` process run, and `ctx {project, branch: root, actor: {kind: agent, session: <sessionId>, call?}}`:
    - **metadata** to the diagnostics journal: `agent.session` at `started`, `resumed`, `interrupted`, `ended` (totals at `ended`); `agent.turn` per model call; **both carry the provenance `harness`, `provider`, `model`, `weights` (and `upstream` on turns)**, read from the catalogue and the response at the time, so a later filter needs no join and no re-collection (spec §12.7); `agent.toolCall` at announcement; `agent.toolResult` at completion with `txIds` from `session_txs` and the attribution (`exact` for built-in tools); `agent.approval`;
    - **content** to the **agent-trace store** (a second `Journal` instance, `<server-data>/agent-traces/`, own cap default 2 GiB and retention default 90 days, whole-session eviction; spec §12.1), only under training consent: per turn the **new messages since the previous turn (`messagesDelta`)**, the response, and `bodyHash`; per call the args and output.
26. **Blobs:** any content field over ~4 KiB becomes a `Body::Blob` in the trace store through `attachBlob` (≤ 4 MiB; above it, truncated with `truncated: true`); `system` and `tools` are always blobs, so one copy is stored per distinct prompt (content addressing) and dropped with its last entry (D-plan Decision 20).
27. **Outcomes and verdicts** (`outcomes.ts`): **5E's `review.verdict` is the single source of verdicts**; 7 defines no verdict kind. The tracker subscribes to the history RPC results inside `finisherd` and to branch status changes, joins them to `session_txs`, stores `session_txs.outcome`, and emits `review.verdict` (`itemKind: none`) for outcomes outside a review item once that kind exists (a lookup, not a flag):
    - `history.merge` copying records whose origin txIds are in `session_txs`, into a branch outside that session's subtree, with no review item behind it → `merged`;
    - `history.undo`/`history.revert` naming such a txId → `undone`/`reverted`, `by: agent` when the actor is that session, `user` when a human principal, `critic` for a critic session;
    - `branch.set {archived: true}` of a session's root with unmerged session txs, or `agent.session.delete` → `abandoned`;
    - one verdict per `(txId, via)`; `openS` from `session_txs.at`.
    If a history result does not name the txIds it undid or copied, Task 11 adds them to the `fin-node` result (an additive field) rather than re-deriving them in TS.
28. **The closed-model-targets counsel gate** (spec §12.7; Owner decision 1). 7A defines the setting `training.closedModelTargets: pendingCounsel|cleared|excluded`, **default `pendingCounsel`**, settable only by a person (5A's human-only settings path; console `finisherd training closed-targets <state>` with the TTY challenge), audited. Nothing in sub-project 7 reads it to drop data (recording is unchanged); it exists now so the first dataset builder or upload path finds it and must honour it.

## Owner decisions (2026-11-07)

The spec's §20 records the owner's answers. What they mean for 7A:
- **1 (closed-model outputs may be training targets):** 7A records provenance on every session and turn (Task 1, Decision 25) and defines the counsel gate, default `pendingCounsel` (Decision 28). Recording is otherwise unchanged.
- **2 (CLI capture records the stream):** concerns 7D/7E; 7A's built-in capture stays exact.
- **3 (Linux kernel ≥ 6.2):** concerns 7C; Task 0's B5 tests the floor and the Landlock-disabled warn-and-run path (F2).
- **4 (`auto` may merge under a per-project policy):** built in 7B; 7A's mode scopes still never include `branch.protected` or `review.decide`.
- **5 (open egress; proxy after v1; per-project keys with spend limits):** per-project OpenRouter keys (Decisions 22–24).
- **6 (CLIs on the user's own sign-in, never copied):** concerns 7C–7E; Task 0's B7 checks the in-place design. 7A creates an empty `provider/` directory per session and nothing else.
- **7 (OpenRouter for the built-in agent, widened by F1; Finisher stores only the OpenRouter key):** Decisions 8, 19, 22–24; Tasks 4, 5, 9, 12, 13, 15.
- **F1 (local models allowed in v1, follow-up of the same day):** the one client has an OpenRouter preset and a local preset; no key for a local URL; loopback for the built-in agent only; diagnostics record `local`; weights `open` or `unknown`. Decisions 8, 15, 19, 20, 22–24; Tasks 1, 4, 5, 9, 10, 12, 13, 15.
- **F2 (Landlock-disabled hosts run everything with warnings; `agents.unsandboxed` removed):** concerns 7C; Task 0's B5 checks the warn-and-run path. Nothing in 7A refuses a session for lack of a sandbox.

## Review Focus

1. **No secret leaks:** keys never in env, argv, logs, nodes, the journal, the trace store or an RPC result; `ModelGateway` holds plaintext for one request only; the console reads keys from stdin.
2. **An agent cannot become the user:** the built-in `fin` tool runs with the agent credential only; **`fin` with `FIN_CREDENTIAL` never uses the local key and ignores `FIN_SERVER_DATA`**; `local.key` is not readable through `workspace_read` (path confinement) or through a `fin` file argument; a session cannot write outside its branch subtree through any RPC (`reach_matrix`); **the author of an agent's commit is the server's, not `FIN_AUTHOR`**.
3. **Resume never re-runs an edit:** the `FIN_CALL` idempotency default, the synthetic interrupted result, and the crash tests at every point of a tool call.
4. **The journal join:** every built-in write's `log.commit.ctx.actor.call` equals its `agent.toolCall.callId`, and `agent.toolResult.txIds` lists exactly those commits; **`session_txs` has every agent commit, whatever its attribution**.
5. **Consent:** with only diagnostics on, no `agent.*` content and no blob reaches disk (either store); with `collect: never`, nothing for that project; **purge and export never touch `control.db`, and nothing in the dataset path reads it**.
6. **Budgets stop at a safe point** and never mid-commit.
7. **Event order:** nodes, budgets and journal entries follow the provider event order per session, and a late event from a replaced provider instance is dropped.
8. **Sessions are isolated from each other:** default branch and `try` state are per session; one session's `fin try` changes nothing for another or for a person's `fin`.

---

### Task 0: Spike S9 in `archive/` (controller-run; can start now) — tier: opus (controller)

Resolves the open facts of spec §8.12 before 7C, 7D and 7E commit to details. **Nothing from it is committed except its conclusions** (AGENTS.md: spikes live in `archive/`, which is git-ignored).

**Files:** Create (git-ignored): `archive/s9/{notes.md, linux/, macos/, windows/, h2/}`. Modify: `TASKS.md` (the S9 line), and when done, the spec's §8.2–8.6 and §4.5–4.6 with the conclusions.

- [ ] **Step 1 (B5, Linux; floor kernel 6.2):** on the Linux VM, a 6.2-series kernel, and a supported kernel booted with Landlock left out of `lsm=` (which must run every profile unconfined, show the `SANDBOX_DISABLED` warning with the `lsm=` fix in `server.status` and `fin status`, record the `sandbox.unconfined` diagnostics event, and refuse nothing; spec §8.9): a seccomp filter that checks the architecture, refuses the x32/compat ABIs, io_uring, `AF_UNIX` and the other listed calls; observe denial for each; confirm libuv and the installed Codex 0.167.0 and Claude Code 2.1.319 still run (including the fallback from io_uring); run the per-CLI validation list of spec §8.3: SSSD/LDAP user lookup, gpg-agent signing, git credential-cache and fsmonitor. Record whether any newer Landlock ABI mediates pathname sockets.
- [ ] **Step 2 (B6, macOS):** under a log-only `(deny default)` profile, derive each CLI's minimal `mach-lookup` list; then confirm that with default-deny `open`, `osascript`, `launchctl` and `security` fail, that nothing starts outside the sandbox, and that another app's keychain item (a test item created for the spike) is unreadable.
- [ ] **Step 3 (B7, the user's own sign-in, never copied; spec §8.6):** with a test account a person signs in to through each CLI's own flow, in a sign-in home: where each CLI keeps its store per OS (file or keychain; whether a keychain item is keyed by the config directory; Codex with `cli_auth_credentials_store="file"`); whether a per-session `CLAUDE_CONFIG_DIR` / `CODEX_HOME` holding a **link** to the store works, and whether the CLI refreshes **in place** (not by renaming over the link); parallel sessions refreshing at once; a file link on Windows without elevation; otherwise the fallback (the sign-in home as config dir) and what it shares; that `system/init` and `account/read` report the sign-in kind and an account label; per-session capability SIDs on a session directory under a normal AppContainer (**to verify**), and that a second session's process cannot open it. **No spike script opens a credential file**; conclusions are about behaviour, never contents.
- [ ] **Step 4 (H2):** Claude Code under `-p` stream-JSON **without `--bare`**, on the user's sign-in: `--setting-sources user` with a per-session config dir, `--strict-mcp-config` and a read denial of the project's `CLAUDE.md`/`.claude/` load no planted settings, hooks, MCP servers or memory files (one of each, in the project folder and in the sign-in home); `--append-system-prompt` honoured; whether `system/init` is emitted before the first prompt and its tool list; `--max-budget-usd` print-only behaviour; `--disallowedTools` built from the init list refuses a listed tool; Codex `-c project_doc_max_bytes=0` and `developer_instructions` at the pinned version (already verified on 0.167.0).
- [ ] **Step 5:** write the conclusions into the spec (each marked *verified on <host class>*), tick `TASKS.md`. **Commit** `docs: record spike S9 conclusions` (only the spec and `TASKS.md`).

### Task 1: Agent kinds v1 in `fin-core` — tier: opus — **[7A-1]**

**Files:**
- Modify: `crates/fin-core/src/events/payloads/agent.rs` (v1 payloads), `crates/fin-core/src/events/kinds.rs` (versions 1; `agent.approval`), `crates/fin-core/src/events/names.rs` (`AGENT_ROLES`, `AGENT_TOOLS`, `AGENT_MODELS` growth), `crates/fin-store/examples/export_schema.rs`, `packages/core-types/schema/fin-core.schema.json`, `packages/core-types/src/generated.ts` (generated), `TASKS.md`.
- Test: `crates/fin-core/tests/events_agent.rs`.

**Interfaces:**
```rust
pub enum Harness { Builtin, ClaudeCode, Codex, Fake, Plugin }
pub enum AgentMode { Plan, Draft, Auto }
pub enum SessionPhase { Started, Resumed, Interrupted, Ended }
pub enum SessionTrigger { User, SavedPrompt, Subagent, Hook, Eval }
pub enum Weights { Open, Closed, Unknown }                 // the model's weights class, from the catalogue at the time (spec §12.7)
pub enum SignIn { Plan, Key, Unknown }                    // CLI harnesses only, as the CLI reports it
pub struct AgentSession { session_id: IdRef, phase: SessionPhase, harness: Harness, provider: Name /* AGENT_PROVIDERS */, model: Name,
    model_hash: Option<Hashed>, weights: Weights, sign_in: Option<SignIn>, role: Name /* AGENT_ROLES */, mode: AgentMode, parent: Option<IdRef>,
    trigger: SessionTrigger, reason: Option<Code>, totals: Option<SessionTotals> }
pub struct SessionTotals { turns: u32, tool_calls: u32, txs: u32, input_tokens: u64, output_tokens: u64,
    cost_micros: Option<u64>, wall_s: u64 }
pub enum CostBasis { Reported, Priced, Subscription, Unknown }
pub enum Capture { Exact, Stream }
pub struct AgentTurn { turn_id, run_id: IdRef, seq: u32, parent_turn_id: Option<IdRef>, harness: Harness, provider, model,
    weights: Weights, upstream: Option<Name> /* AGENT_PROVIDERS: where OpenRouter routed the call */,
    input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens: u64, latency_ms,
    ttft_ms: Option<u64>, stop_reason, cost_micros: Option<u64>, cost_basis: CostBasis, capture: Capture,
    system_hash: Hashed, tools_hash: Hashed, body_hash: Option<Hashed> /* BLAKE3 of the full request body as sent */,
    messages_delta, output, system, tools: Option<Content<Body>> /* modelInput, modelOutput, modelInput, modelInput */ }
pub struct AgentToolCall { call_id: IdRef, turn_id: IdRef, tool: Name /* AGENT_TOOLS */,
    command: Option<Name> /* CLI_COMMANDS */, args_hash: Hashed, args: Option<Content<Body>> }
pub enum Attribution { Exact, Inferred, Ambiguous, None }
pub struct AgentToolResult { call_id, ok: bool, code: Option<Code>, exit: Option<i32>, micros: u64,
    tx_ids: Vec<IdRef>, attribution: Attribution, truncated: bool, output: Option<Content<Body>> }
pub enum ApprovalKind { Rule, Budget, Provider, Plugin }
pub enum ApprovalDecision { Allow, Deny, Expired }
pub struct AgentApproval { approval_id: IdRef, session_id: IdRef, kind: ApprovalKind,
    decision: ApprovalDecision, by: DecidedBy /* user|policy */, latency_ms: u64, subject: Option<Content<Body>> /* userText */ }
// No verdict types: `agent.editVerdict` is removed (the v0 reservation is deleted from `kinds.rs` and `payloads/agent.rs`).
// 5E's `review.verdict` is the single source of verdicts (spec §12.5).
```
Spec §12.2; D-plan Decisions 3, 4, 20, 28.

- [ ] **Step 1: Write failing tests:**
  - `agent_kinds_are_version_1` (session, turn, toolCall, toolResult and the new `agent.approval` at 1; `agent.editVerdict` no longer registered; `training_only` stays on turn, toolCall, toolResult);
  - `agent_kinds_round_trip` (a full payload each; `Body::Inline` and `Body::Blob`);
  - `payload_strings_are_classified` still passes with the new fields (the existing schema walk);
  - `redact_drops_agent_content` (under diagnostics, `messages_delta`, `output`, `system`, `tools`, `args`, `subject` are gone and the hashes, `body_hash` and counts remain);
  - `blob_refs_listed` (`AnyPayload::blobs()` finds every `Body::Blob` in the new fields);
  - `agent_tables_match_patterns` and `roles_and_tools_tables_have_other`; `agent_providers_has_openrouter_and_local`;
  - **`provenance_on_every_session_and_turn`** (`harness`, `provider`, `model` and `weights` are required, never content-classed, and survive `redact` under diagnostics, so a later filter on them needs no training content);
  - `tx_ids_capped_at_64` (`validate_payload` refuses 65 with `EVENT_INVALID`).
- [ ] **Step 2: Implement.** Keep `approx_struct!` lists complete (the D1 `approx_covers_every_field` test). `AGENT_PROVIDERS` gains `openrouter` and `local`; `AGENT_MODELS` keeps the Claude ids (the ids Claude Code reports) now; Task 4's drift test adds the OpenRouter slugs, the Codex ids and the local-models table's names.
- [ ] **Step 3: Regenerate** the schema and TS types; run the gates → PASS. **Commit** `feat(core): agent event kinds v1 with harness, provenance, modes, tool kinds, attribution and approvals; retire editVerdict`.

### Task 2: Contracts: agent RPCs and scopes — tier: sonnet — **[7A-2]**

**Files:**
- Create: `packages/contracts/src/agent.ts` (imports `agent-events.ts`).
- Modify: `packages/contracts/src/rpc.ts` (the group), `packages/contracts/src/scopes.ts` (5A's file: `agent.read`, `agent.run`, `agent.approve` (human), `agent.admin` (owner); each new RPC's scope), `packages/contracts/src/index.ts`, `crates/fin-core/src/events/names.rs` (`RPC_METHODS` additions; the D4 drift test), `TASKS.md`.
- Test: `packages/contracts/src/agent.test.ts`.

**Interfaces:**
```ts
// Harness, Mode, SessionStatus, NodeKind, BudgetSpec, ProviderEvent: see agent-events.ts (7A-1)
export const Session: Schema.Struct<…>;  export const Run: Schema.Struct<…>;  export const Node: Schema.Struct<…>;
export const WatchItem: Schema.Union<[NodeAdded, Delta, StatusChanged, UsageUpdated, ApprovalOpened]>;
// RPCs: agent.session.start|prompt|steer|interrupt|resume|end|list|get|delete, agent.watch (stream),
//       agent.approval.list|answer, agent.providers, agent.self, agent.self.try
```
`Harness`, `Mode`, `SessionStatus`, `NodeKind`, `BudgetSpec` and `ProviderEvent` already exist in `agent-events.ts` (Task 6a, 7A-1); this task adds the RPC-facing schemas (`Session`, `Run`, `Node`, `WatchItem`), the RPC group, and `agent.self`, `agent.self.try` (Decision 14a). It needs 5A's `scopes.ts`.
Spec §3.5; Decisions 6, 14a, 15.

- [ ] **Step 1: Write failing tests:** `agent.self` (`project.read`) and `agent.self.try` (`project.write`) are refused for every principal kind but `agent`; every new RPC has exactly one scope (5A's `rpc_scopes_cover_every_rpc`); `agent.approval.answer` and `agent.session.delete` are refused for `local` and `agent` principals (5A's `human_only_matrix` extended); schema round trips for every `ProviderEvent` and `WatchItem` variant; `agent.watch` is listed among the stream RPCs excluded from `rpc.stats` (`rpc_stats_excludes_streams`).
- [ ] **Step 2: Implement.** `agent.session.start` from an `agent` principal is `SCOPE_DENIED` in 7A (a parent's own children arrive in 7B).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(contracts): agent runtime RPCs, schemas and scopes`.

### Task 3a: The pure session state machine — tier: opus — **[7A-1]**

**Files:**
- Create: `apps/finisherd/src/agent/session.ts`.
- Modify: `TASKS.md`.
- Test: `apps/finisherd/test/agent-session.test.ts`.

**Interfaces:**
```ts
// session.ts (pure: no I/O, no control.db)
export type SessionEvent = { kind: "runStarted"; runId: string } | { kind: "runEnded"; stop: string }
  | { kind: "approvalOpened"; id: string } | { kind: "approvalAnswered"; id: string }
  | { kind: "interrupted"; reason: string } | { kind: "resumed" } | { kind: "ended"; reason: string };
export function next(s: SessionState, e: SessionEvent): { state: SessionState; effects: Effect[] } | { error: FinError };
```
Decision 2.

- [ ] **Step 1: Write failing tests:** `state_machine_properties` (fast-check: random event sequences never reach an undefined state; `ended` absorbs; `prompt` while `running` → `SESSION_BUSY`; `resume` only from `interrupted`; every effect list is a function of the transition alone).
- [ ] **Step 2: Implement.** No imports from 5A, `ControlStore` or Effect services beyond the error type.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): pure agent session state machine`.

### Task 3b: The session store, `session_txs`, session state and crash recovery — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/{store.ts, blobs.ts}`, `apps/finisherd/src/control/migrations/agent/0001.ts`.
- Modify: `apps/finisherd/src/control/migrations.ts` (register the `agent` chain), `TASKS.md`.
- Test: `apps/finisherd/test/agent-store.test.ts`.

**Interfaces:**
```ts
export class AgentStore {
  constructor(control: ControlStore, serverData: string, clock: Clock, ids: Ids);
  createSession(s: NewSession): Session;                          // id = the session id = the credential id
  apply(sessionId: string, e: SessionEvent): Session;             // one tx, through session.ts; SESSION_BUSY / SESSION_ENDED
  appendNode(n: NewNode): Node;                                   // inline ≤ 4 KiB, else blob first
  nodes(sessionId: string, fromSeq?: number, limit?: number): Node[];
  recordTx(t: { txId: string; sessionId: string; callId: string | null; attribution: Attribution; at: number }): void;
  sessionOfTx(txIds: string[]): Map<string, SessionTx>;
  setOutcome(txId: string, outcome: Outcome): void;
  state(sessionId: string): SessionState2;                        // { defaultBranch, tries }
  setDefaultBranch(sessionId: string, branch: string): void;  recordTry(...): void;  clearTry(...): void;
  recoverAfterCrash(): { interrupted: string[] };                 // Decision 3
  sweep(now: number): number;                                     // Decision 4
  delete(sessionId: string): void;
  ledger(): LedgerStore;                                          // Task 4's interface over `budgets`
}
```
Decisions 1, 3, 4.

- [ ] **Step 1: Write failing tests:**
  - `node_blob_threshold` (4 KiB inline, 4 KiB + 1 in a blob, written before the row; a crash between the two leaves an orphan blob, swept, never a dangling row);
  - `recover_marks_interrupted_and_closes_open_calls` (a session `running` with an unanswered `toolCall` node gets a synthetic result);
  - **`session_txs_records_every_agent_commit`** (a commit with a `callId` and one with none are both recorded under the session; `sessionOfTx` for known and unknown txIds; a second session's txs never appear under the first);
  - **`session_state_is_per_session`** (two sessions on one project keep separate default branches and try lists; `clearTry` restores the previous default);
  - `retention_sweeps_ended_sessions_and_blobs`; `delete_removes_rows_and_dir`;
  - `diagnostics_purge_and_export_leave_nodes_alone` (calling 5A's purge and export paths leaves `sessions`, `runs`, `nodes` untouched);
  - `statement_budget` (5A's per-statement budget, with 10k sessions and 1M nodes in the measurement script; ≤ 1 ms p99 on the Linux VM, asserted ≤ 5 ms in the test).
- [ ] **Step 2: Implement.** All writes through `ControlStore.tx`; no bulk work on the main thread (blob writes are async file I/O before the transaction).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): agent session store, session_txs, per-session state and crash recovery`.

### Task 4: The model catalogue and the budget ledger — tier: sonnet — **[7A-1]**

**Files:**
- Create: `apps/finisherd/src/agent/catalogue/{models.json, local-models.json, catalogue.ts}`, `apps/finisherd/src/agent/budget.ts`, `apps/finisherd/src/agent/ledger.ts` (the `LedgerStore` interface and an in-memory implementation), `apps/finisherd/test/fixtures/catalogue.test.json`.
- Modify: `crates/fin-core/src/events/names.rs` (`AGENT_MODELS` = the catalogue's ids + the local-models table's names + `other`), `TASKS.md`.
- Test: `apps/finisherd/test/agent-budget.test.ts`, `crates/fin-core/tests/events_agent.rs` (`catalogue_ids_in_agent_models`, reading `models.json` and `local-models.json` at test time).

**Interfaces:**
```ts
export interface ModelEntry { id: string; harness: "builtin" | "claudeCode" | "codex"; provider: "openrouter" | "anthropic" | "openai";
  maker: string; weights: "open" | "closed"; variant?: string; window: number; maxOutput: number;
  capabilities: { tools: boolean; thinking: boolean; images: boolean; caching: "explicit" | "automatic" | "none" };
  price?: { input: number; output: number; cacheRead: number; cacheWrite: number } /* micros per MTok */ }
export class Catalogue { static load(bundled: string, extra: ExtraModel[] /* agents.models */): Catalogue; get(id: string): ModelEntry | undefined;
  readonly openrouterBaseUrl: string;
  local(name: string, extra?: ExtraModel): { model: string /* a table family name, else "other" */; modelHash?: string /* keyed */; weights: "open" | "unknown"; window: number; tools: boolean };
  price(id: string, u: Usage, reportedMicros?: number): { micros: number; basis: "reported" | "priced" | "unknown" } }
export interface LedgerStore { read(sessionId: string, kind: BudgetKind): Ledger; add(sessionId: string, kind: BudgetKind, delta: { used?: number; reserved?: number }): Ledger;
  setLimit(sessionId: string, kind: BudgetKind, limit: number): void; dailyMoney(utcDay: string): number; addDailyMoney(utcDay: string, micros: number): void }
export class Budget {
  constructor(ledger: LedgerStore, sessionId: string, limits: BudgetLimits, clock: Clock);
  canCall(projected: Usage): Allowance;            // ok | stop {kind, used, limit}
  charge(u: Usage, micros: number): Crossings;     // notices at 80 %, stop at 100 %
  raise(kind: BudgetKind, by: number): void;
}
```
Decisions 10, 19–21. 7A-2's `AgentStore.ledger()` is the `control.db` implementation (Task 3b).

- [ ] **Step 1: Write failing tests:** pricing with cache read and write; `reported` when OpenRouter's usage gives a cost, `priced` from the catalogue otherwise, `unknown` for an extra slug with neither; an extra slug's `weights` is `unknown` unless set; every bundled entry has a `weights` class and a `maker`; projected stop before a call that cannot fit; 80 % notice once per kind; 100 % stop; daily money cap across sessions and its UTC-day rollover (injected clock); `raise` after a stop; `catalogue_ids_in_agent_models`; **local models (F1):** `local_family_resolves_from_table` (the longest prefix wins, the family name is recorded, never the typed tag), `unknown_local_model_is_other_with_keyed_hash` (`weights: unknown`, or `open` only when the owner says so, never `closed`), `local_cost_is_unknown_and_charges_no_money` (token and call budgets still apply).
- [ ] **Step 2: Fill the catalogue** from OpenRouter's models page (the OpenRouter models the owner names, at least one closed and one open-weight, with each model's maker and weights class from its model card) and the CLIs' model ids, on the day (record the URLs in the commit body, not in the file); fill `local-models.json` with the open-weight families the owner names (maker, window and tools support from each model card); set `asOf`.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): model catalogue and session budgets`.

### Task 5: `SecretStore`, the `SecretBox` seam and `ModelGateway` — tier: opus — **[7A-1]**

**Files:**
- Create: `apps/finisherd/src/secrets/{store.ts, gateway.ts, box.ts}`.
- Modify: `TASKS.md`.
- Test: `apps/finisherd/test/secrets.test.ts`.

**Interfaces:**
```ts
export interface Sealed { v: 1; data: string /* base64url */ }
export interface SecretBox { seal(plaintext: Buffer): Sealed; open(s: Sealed): Buffer }   // the seam for the keychain backend (5B)
export const identityBox: SecretBox;                                                        // the file backend's box (no cipher)
export interface SecretStore { put(e: { provider: "openrouter"; label; projectId?; value: Buffer }): string; list(): SecretMeta[]; remove(id: string): void; }
export class ModelGateway {
  constructor(store: SecretStore, sessions: { isRunning(sessionId: string): boolean }, audit: AuditSink, clock: Clock);
  withKey<A>(projectId: string, sessionId: string, f: (key: string) => Promise<A>): Promise<A>;   // the project's OpenRouter key, else the default
  endpoint(preset: "openrouter" | "local", sessionId: string): { baseUrl: string; keyed: boolean };    // `local`: the owner's setting, `keyed: false`, the store is never read
}
```
Decisions 22–24. **No Rust and no cipher**: the 0600 file in the private server-data directory is the protection (the sandbox denies the directory to agents, spec §8.1).

- [ ] **Step 1: Write failing tests:**
  - `secrets.json` is created 0600 (unix) inside a 0700 directory and holds the sealed form; `list()` has no value field; round trip through `identityBox`; **`seal`/`open` are the only crossing** (a test box that XORs shows the store never touches plaintext except through `open`);
  - a corrupted entry fails with `SECRET_CORRUPT`; `put` with a provider other than `openrouter` is refused;
  - `withKey` uses the project's key when one is bound, else the server default, and is `PROVIDER_AUTH` when neither exists;
  - `withKey` refuses a non-running session (`PROVIDER_AUTH`) and audits the refusal; the first grant per session is audited once (through the injected `AuditSink`); the key never appears in thrown errors, logs captured during the test, or `JSON.stringify` of anything the gateway returns;
  - **`local` needs no key (F1):** `endpoint("local", …)` returns the owner's base URL with `keyed: false` and never calls the store (a spy), a session of the local preset starts with an empty `SecretStore`, and the first use per session is audited like a grant;
  - concurrent `put`s serialise; writes are temp-then-rename; the plaintext buffer is zero-filled after `f` settles.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): secret store with a seal/open seam and model gateway`.

### Task 6a: The adapter contract, the `ProviderEvent` schema and the fake provider — tier: opus — **[7A-1]**

**Files:**
- Create: `apps/finisherd/src/agent/{adapter.ts, fake.ts}`, `packages/contracts/src/agent-events.ts` (`Harness`, `Mode`, `SessionStatus`, `NodeKind`, `BudgetSpec`, `ProviderEvent`), `apps/finisherd/test/support/adapter-contract.ts` (a reusable contract suite any adapter must pass).
- Modify: `packages/contracts/src/index.ts`, `TASKS.md`.
- Test: `apps/finisherd/test/agent-fake.test.ts`, `packages/contracts/src/agent-events.test.ts`, `apps/finisherd/test/fixtures/fake-scripts/*.json`.

**Interfaces:**
```ts
export interface ProviderAdapter { readonly harness: Harness; detect(): Effect<Detection, FinError>;
  open(spec: SessionSpec): Effect<ProviderSession, FinError, Scope.Scope>; }
export interface ProviderSession { readonly events: Stream.Stream<ProviderEvent>; startTurn(i: TurnInput): Effect<void, FinError>;
  steer(i: UserInput): Effect<void, FinError>; interrupt(): Effect<void>; respond(id: string, a: RuntimeAnswer): Effect<void, FinError>;
  rollback(toTurn: number): Effect<void, FinError>; ref(): ProviderRef | null; }          // no fork (cut)
// fake.ts: a script is a list of steps per turn: emit events, wait for a tool result, crash, emit late events after replace, emit an unknown event
export const FakeAdapter: (scripts: FakeScript[]) => ProviderAdapter;
```
Decision 5.

- [ ] **Step 1: Write failing tests:** schema round trips for every `ProviderEvent` variant; the fake emits scripted events in order per session; a scripted spawn failure surfaces as `PROVIDER_EXITED`; late events from a replaced instance carry the old instance's binding (the runtime drops them in 6b); an **unknown event** in a script is passed through as a `notice`-able event, never a failure; `adapterContractSuite(FakeAdapter(...))` passes.
- [ ] **Step 2: Implement.** No import of `control.db`, `runtime.ts` or 5A.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): provider adapter contract, ProviderEvent schema and fake provider`.

### Task 6b: The runtime engine — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/runtime.ts`.
- Modify: `TASKS.md`.
- Test: `apps/finisherd/test/agent-runtime.test.ts`.

**Interfaces:**
```ts
export class Runtime {   // Effect service over AgentStore, ProviderAdapter(s), Budget, AgentJournal
  start(req: StartRequest, by: Principal): Effect<Session, FinError>;
  prompt(sessionId, text, by): Effect<Run, FinError>; steer(...); interrupt(...); resume(...); end(...);
  watch(sessionId, fromSeq?): Stream.Stream<WatchItem, FinError>;
}
```
Decisions 2, 3, 5, 6.

- [ ] **Step 1: Write failing tests** (all with `FakeAdapter`):
  - `events_become_nodes_in_order` (deltas streamed, completed nodes stored once);
  - `tool_call_gets_a_ulid_call_id` (announced before execution; `providerId` kept);
  - `late_events_from_replaced_instance_dropped` (arch §27.5);
  - `unknown_event_becomes_notice`;
  - `spawn_failure_fails_turn_provider_exited`;
  - `interrupt_keeps_branch_and_status_interrupted`;
  - `resume_after_crash_never_reruns_tool` (crash points: after announce, after the tool process started, after the commit, after the result; the commit count is 1 in every case);
  - `steer_is_queued_before_next_call`;
  - `watch_snapshot_then_live` (a watcher joining mid-run gets every node once);
  - `budget_stop_at_safe_point` (stop requested during a tool call takes effect after its result);
  - `budget_facts_are_in_a_message_not_the_system_prompt` (the spec the adapter receives has an identical `system` across two sessions of a role whose budgets differ).
- [ ] **Step 2: Implement.** One fiber per live session consumes its `events` stream and applies them through `AgentStore`; RPC calls enqueue commands to that fiber, so per-session order is total.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): agent runtime engine`.

### Task 7: Credentials, session branches, branch reach and path reach — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/{credentials.ts, reach.ts}`.
- Modify: 5A's `apps/finisherd/src/authorize.ts` (one call into `reach.ts` for agent principals: branches, paths, the after-stage `session_txs` write and the server-set `Tx.author`), `apps/finisherd/src/errors.ts` and `crates/fin/src/output.rs` (`BRANCH_NOT_ALLOWED` exit 7, `BRANCH_IN_USE` exit 4), `TASKS.md`.
- Test: `apps/finisherd/test/agent-credentials.test.ts`, `apps/finisherd/test/agent-reach.test.ts`.

**Interfaces:**
```ts
export const MODE_SCOPES: Record<Mode, readonly Scope[]>;     // spec §6.1, 7A subset (Decision 15.3); no project.create, no project.open
export function mintForSession(s: Session, root: string | null): Effect<{ credentialId: string; path: string }, FinError>;   // credentialId === s.id
export function revokeForSession(s: Session): Effect<void>;
export class BranchReach { allowed(principal: Principal, projectId: string, rpc: string, payload: unknown): Effect<void, FinError> }
export class PathReach { roots(principal: Principal, projectId: string): Promise<string[]> }   // project folder, workspace/<session>, referenced library roots
```
Decisions 15–18.

- [ ] **Step 1: Write failing tests:**
  - `start_creates_root_branch_and_credential` (branch named `agent/editor-<n>` from `main` head; credential file 0600 with the 5A format; principal scopes equal `MODE_SCOPES.draft`);
  - **`principal_id_is_session_id`** (`auth.whoami` under the credential returns the session id; the journal actor session equals it);
  - `plan_mode_has_no_branch_and_no_write_scope`;
  - `reach_matrix`: for an agent principal rooted at `br3`, every write RPC targeting `br3`, a branch it created under `br3`, `main` and a sibling session's branch → allowed, allowed, `BRANCH_NOT_ALLOWED`, `BRANCH_NOT_ALLOWED`; reads of any branch allowed; `history.merge {from: main, into: br3}` allowed, `{from: br3, into: main}` refused;
  - **`path_reach`**: an `asset.probe` or `project.open` naming another project's folder is `PROJECT_NOT_FOUND`; a directory outside every root is `PATH_NOT_ALLOWED`; a library the project does not reference is `PATH_NOT_ALLOWED`; the project's own library is allowed read-only;
  - **`no_project_open_or_create_in_any_mode`** (every `MODE_SCOPES` entry; `project.open` and `project.create` from an agent principal → `SCOPE_DENIED`);
  - **`author_is_set_by_server`** (a commit from an agent principal with `fin-author: someone-else` lands as `agent:editor:<id>`);
  - **`session_txs_written_after_every_agent_write`** (commit with and without a `call` header; undo; merge into the session's own root);
  - `branch_in_use` (a second live session on the same root);
  - `end_revokes_credential` (`CREDENTIAL_REVOKED` for the old file); `resume_keeps_credential` across a restart and refreshes its expiry;
  - `no_human_scope_in_any_mode` (every `MODE_SCOPES` entry against 5A's human-only list).
- [ ] **Step 2: Implement.** Lineage comes from the engine's branch index through `branch.list` (cached per project; invalidated by `branch` events on `project.events`).
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(finisherd): session branches, agent credentials by mode, branch and path reach`.

### Task 8a: `FIN_CALL` as the default idempotency key in `fin` — tier: opus — **[7A-1]**

**Files:**
- Create: `crates/fin/src/agent_env.rs`.
- Modify: `crates/fin/src/commands/builder.rs` (idem default), `crates/fin/src/main.rs` (call-site line), `TASKS.md`.
- Test: `crates/fin/tests/agent_env.rs`.

**Interfaces:**
```rust
// agent_env.rs: FIN_CALL → default idem key for single writes; `<call>.<line>` for batch lines.
pub fn default_idem_key(cmd: &Command, explicit: Option<&str>, line: Option<usize>) -> Option<String>;
```
Decision 14. Needs only the engine as it is today; nothing from 5.

- [ ] **Step 1: Write failing tests:** `fin_call_becomes_idem_key_for_writes`, `batch_lines_get_suffixed_keys`, `explicit_idem_key_wins`, `no_fin_call_no_change`, `reads_ignore_fin_call`, `retried_write_with_same_fin_call_is_a_dedupe` (two runs, one commit).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin): FIN_CALL is the default idempotency key for writes`.

### Task 8b: The built-in loop and its tools; `fin` under an agent credential — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/builtin/{loop.ts, tools.ts, fin-tool.ts, workspace-tools.ts}`, `crates/fin/src/{agent_session.rs, agent_paths.rs}`.
- Modify: `apps/finisherd/src/env.ts` (`ChildEnvExtra` gains `FIN_CREDENTIAL`, `FIN_CALL`; `FIN_SERVER_DATA` and `FIN_AUTHOR` are explicitly removed), `crates/fin/src/auth.rs` (5A's: no local-key fallback when `FIN_CREDENTIAL` is set), `crates/fin/src/commands/mod.rs` (`configured_branch` and the author: principal-derived under an agent credential), `crates/fin/src/config.rs` and `crates/fin/src/commands/try_cmd.rs` (per-session try state through `agent.self`/`agent.self.try`; `fin-config.json` untouched), `crates/fin/src/registry*.rs` (`argKinds: path`), `crates/fin/src/commands/builder.rs`, `TASKS.md`.
- Test: `apps/finisherd/test/agent-builtin-loop.test.ts`, `apps/finisherd/test/agent-tools.test.ts`, `crates/fin/tests/agent_session.rs`, `crates/fin/tests/agent_paths.rs`.

**Interfaces:**
```ts
export const BuiltinAdapter: (deps: { client: ModelClient /* the OpenRouter client */; gateway: ModelGateway; catalogue: Catalogue;
  finPath: string; serverData: string }) => ProviderAdapter;      // ModelClient, ModelRequest: Task 9
export function runFinTool(call: { callId: string; argv: string[]; stdin?: string }, env: SessionEnv): Promise<ToolOutcome>;
export function workspaceTool(name: "workspace_read" | "workspace_write" | "workspace_list", args: unknown, root: string): Promise<ToolOutcome>;
```
```rust
// agent_session.rs: the principal-derived defaults used when FIN_CREDENTIAL names an agent credential
pub fn session_default_branch(backend: &dyn Backend) -> Result<String, CoreError>;       // from auth.whoami
pub fn record_try(backend: &dyn Backend, branch: &str) -> Result<(), CoreError>;        // agent.self.try
// agent_paths.rs: confinement of client-side file arguments
pub fn confine(path: &Path, workspace: &Path, project: &Path) -> Result<PathBuf, CoreError>;   // PATH_NOT_ALLOWED
```
Decisions 6, 7, 10–14 (14a–14d), 17.

- [ ] **Step 1: Write failing tests:**
  - loop (with a scripted `ModelClient`): `end_turn_stops`, `tool_round_trip`, `parallel_reads_serial_writes`, `per_run_caps` (60 calls, 200 tools), `result_truncation_head_tail_note`, `context_stop_at_85_percent`, `retry_on_429_with_retry_after` (injected clock; same request body re-sent; no tool re-run);
  - `fin` tool: `argv_without_shell` (an argument `; rm -rf x` reaches `fin` verbatim and fails as usage), `env_is_allow_list_plus_session_vars` (no parent variable leaks; `FIN_TRAINING`, **`FIN_SERVER_DATA` and `FIN_AUTHOR`** absent even when set in `finisherd`'s own environment), `timeout_kills_tree`, `json_stdout_parsed`, `exit_and_stderr_tail`;
  - workspace tools: `rejects_absolute_and_dotdot`, `rejects_symlink_escape` (a link to `<server-data>/auth/local.key`), `case_folding_on_windows`, `size_caps`, `write_is_temp_then_rename`;
  - **B1 (`crates/fin/tests/agent_session.rs`):**
    - `agent_default_branch_is_session_root` (a write with no `--branch` under an agent credential commits to the session's root even when `fin-config.json` names another branch or `main`);
    - `try_state_is_per_session` (two sessions on one project: `fin try` in one changes neither the other's default branch nor its try list; `fin-config.json` is byte-identical before and after, and a person's `fin` with no credential still sees its own defaults);
    - `try_outside_subtree_refused` (`BRANCH_NOT_ALLOWED`); `abort_try_restores_default`;
  - **B3:**
    - **`fin_with_credential_never_uses_local_key`** (a server-data directory with a valid `local.key` readable; `FIN_CREDENTIAL` naming a missing, expired or revoked file → `UNAUTHENTICATED` exit 7 and the server's audit shows no `local` connection; `FIN_SERVER_DATA` pointing at that directory is ignored while `FIN_CREDENTIAL` is set);
    - `author_cannot_be_overridden` (with `FIN_AUTHOR=other` the commit is `agent:<role>:<id>`; `undo` acts only on that session's edits);
  - **B4 (`crates/fin/tests/agent_paths.rs`):** `file_args_confined` (`caption import --words`, `asset add`, `apply` files: a path inside the workspace or the project folder works; an absolute path outside, `..` escapes, a symlink to `<server-data>/auth/local.key` and a case-variant on Windows/macOS → `PATH_NOT_ALLOWED` before the file is opened, verified with an open-counter); `every_path_argument_is_marked` (a registry drift test: each command with a client-side file argument has `argKinds: path`); the same check runs in the built-in tool for arguments it recognises; without an agent credential nothing changes.
- [ ] **Step 2: Implement.** The loop is an adapter like any other: it emits `ProviderEvent`s, including `capture {request, response}` per model call, and calls tools through the runtime's tool executor so the runtime sees `toolStarted`/`toolCompleted` with the call id it minted.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): built-in loop with a native fin tool; fin under an agent credential keeps per-session state, no local fallback and confined paths`.

### Task 9: The OpenAI-compatible client (OpenRouter and local presets) and the fake model server — tier: sonnet — **[7A-1]**

**Files:**
- Create: `apps/finisherd/src/agent/builtin/{request.ts, compat.ts}`, `apps/finisherd/test/support/fake-model-server.ts`, `apps/finisherd/test/fixtures/model-streams/{openrouter,local}/*.sse`.
- Modify: `apps/finisherd/package.json` (`openai` exact pin: 7.47.0 until 7.53.0 is ≥7 days old; no `@anthropic-ai/sdk`), `pnpm-lock.yaml`, `TASKS.md`.
- Test: `apps/finisherd/test/agent-model-clients.test.ts`.

**Interfaces:**
```ts
export interface ModelRequest { system; tools; messages; model; maxTokens; temperature?; reasoning? }   // Decision 7 (neutral form)
export type KeySource = <A>(f: (key: string) => Promise<A>) => Promise<A>;                               // Task 5's gateway in 7A-2; a constant in tests; `null` for the local preset (no key)
export interface ModelClient {
  stream(req: ModelRequest, key: KeySource | null, signal?: AbortSignal): AsyncIterable<ModelStreamEvent>;
  readonly wireBody: (req: ModelRequest) => unknown;   // what is sent, for capture
}
```
`openaiCompatClient({preset: "openrouter" | "local", baseUrl})`: the one `ModelClient` (Chat Completions; `compat.ts`); the preset switches on the OpenRouter-only features (usage cost, `upstream`, `reasoning`, `cache_control`) and sends a fixed placeholder bearer, with no key, for `local`. `startFakeModelServer(scripts): Promise<{ baseUrl; requests(): RecordedRequest[]; close() }>` on `127.0.0.1:0`, speaking Chat Completions as OpenRouter serves it, and in a `local` mode as Ollama and LM Studio serve it (a `usage` object or none, no cost, no `upstream`, a refusal of tools, `GET /models`, a connection refused). Decisions 7–9. Needs no store, no gateway and nothing from 5.

- [ ] **Step 1: Write failing tests** against the fake server: text streaming; a tool call split across deltas; two parallel tool calls; usage incl. cache read/write and reasoning tokens, **OpenRouter's reported cost (`costBasis: reported`) and the upstream provider name**; reasoning details sent back unchanged on the next call; `cache_control` breakpoints present for a `caching: explicit` model and absent otherwise; `include_usage`; 429 with `retry-after`, 503, a mid-stream disconnect (one retry, a `notice`); a malformed event → `PROVIDER_PROTOCOL`; 401 → `PROVIDER_AUTH`; context-length error → `MODEL_CONTEXT_EXCEEDED`; **local preset (F1):** no key and no `Authorization` value but the fixed placeholder reaches the server; usage reported or estimated (`usageEstimated`); `costBasis: unknown`; no `cache_control`, `reasoning` or `upstream`; a tool refusal → `MODEL_UNKNOWN {noTools}`; connection refused → `PROVIDER_UNAVAILABLE {provider: local}`; a redirect to another origin is not followed; the base URL appears in no capture, error or log; **`wire_body_equals_recorded_request`** (the capture is the exact bytes the server received, modulo JSON key order, which the client fixes) **and `body_hash_matches`** (the BLAKE3 of those bytes equals the `bodyHash` the client reports).
- [ ] **Step 2: Implement.** The `openai` SDK client is created per request with the key from the `KeySource`, `maxRetries: 0` (the loop owns retries) and the preset's base URL (the catalogue's for OpenRouter, the owner's setting for `local`; no key for `local`). Re-read OpenRouter's API documentation for usage accounting, reasoning and caching on the day; note differences in the commit body.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): OpenAI-compatible Chat Completions client with OpenRouter and local presets, and fake model server`.

### Task 10: Journal emission, the agent-trace store and attribution — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/journal.ts`.
- Modify: `crates/fin-node/src/journal.rs` (napi `attachBlob(bytes, mediaType)` if D4 did not add it; **options to open a second `Journal` with its own directory, cap and retention**, if D-plan's `Journal` has none: check first, Step 0), `apps/finisherd/src/agent/runtime.ts` (call sites), 5A's `authorize.ts` (an agent principal's `call` header is accepted and passed to the engine as the actor's call; spec §12.3), `TASKS.md`.
- Test: `apps/finisherd/test/agent-journal.test.ts`, `crates/fin-node/tests/journal_blob.rs`, `crates/fin-node/tests/journal_second_instance.rs`.

**Interfaces:**
```ts
export class AgentJournal {
  constructor(diagnostics: NapiJournal, traces: NapiJournal, ids: Ids);
  session(s: Session, phase: SessionPhase, totals?: SessionTotals): void;
  turn(t: TurnRecord): void;            // metadata always (diagnostics); content as messagesDelta + bodyHash only when the gate has training
  toolCall(c: ToolCallRecord): void; toolResult(r: ToolResultRecord): void;
  approval(a: ApprovalRecord): void;
}
```
Decisions 25, 26.

- [ ] **Step 0:** read D-plan's `Journal` open options; if they lack per-instance cap and retention, add them (additive) so the trace store is a second instance of the same code.
- [ ] **Step 1: Write failing tests** (a real `fin-node` journal in a temp server data):
  - `off_means_off` (both purposes off: no `events/` and no `agent-traces/` directory after a whole scripted session);
  - `diagnostics_only_has_no_content_or_blobs` (neither store holds content);
  - **`provenance_on_every_record`** (every `agent.session` and `agent.turn` entry carries `harness`, `provider`, `model`, `weights`, and turns `upstream` when OpenRouter named one, under diagnostics as well as training; a filter on `weights: closed` over a recorded session selects exactly its closed-model turns; **a local session (F1)** records `provider: local`, a table family name or `other` plus a keyed hash as the model, `weights` `open` or `unknown`, `costBasis: unknown`, and its base URL appears in no record);
  - `training_records_deltas_and_full_body_hash` (turn *n* stores only its new messages; replaying the deltas reproduces the exact request body whose BLAKE3 equals `bodyHash` and the fake model server's recorded bytes; `capture: exact`);
  - **`trace_store_is_separate`** (a long scripted session filling the trace store to its cap evicts whole old sessions from `agent-traces/` and never removes a diagnostics entry; the diagnostics journal's 128 MiB / 30-day rules are unchanged);
  - `system_and_tools_blobs_deduplicate` (two sessions with one role → one blob each, refcount 2; purging one session's entries keeps the blob);
  - `large_tool_output_is_a_blob` (over ~4 KiB), `over_4_mib_truncated`;
  - `collect_never_drops_everything_for_that_project`;
  - **`call_join`** (every `log.commit` of a built-in write has `ctx.actor.call` = the `agent.toolCall.callId`, and the `agent.toolResult.txIds` are exactly those commits, `attribution: exact`);
  - `agent_header_cannot_claim_another_session` (5A rule: the session comes from the principal; only `call` from the header);
  - **`purge_and_export_do_not_touch_conversations`** (after `diagnostics purge --all` and a content export, `sessions`, `runs`, `nodes` rows and blobs are unchanged);
  - **`dataset_builder_never_reads_control_db`** (the builder's input module — a stub crate `fin-dataset-inputs` that exports only the journal and trace-store readers — is checked by an import-graph gate for any path to `control.db` accessors, and by a file-access spy that runs its reader against a server-data directory and asserts `control.db` is never opened).
- [ ] **Step 2: Implement.** Payloads are built after the gate check; content is moved, not cloned, into the payload where possible.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): record sessions, turns, tool calls and results; agent-trace store with deltas, joined by FIN_CALL`.

### Task 11: The outcome tracker, feeding 5E's `review.verdict` — tier: opus — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/outcomes.ts`.
- Modify: `apps/finisherd/src/history.ts` (publish typed results to the tracker; call-site lines), `crates/fin-node/src/history.rs` and the contracts' history result schemas only if a result lacks the txIds it undid or copied (additive fields, Decision 27), `TASKS.md`.
- Test: `apps/finisherd/test/agent-outcomes.test.ts`.

**Interfaces:** `class OutcomeTracker { observeHistory(rpc, payload, result, principal): void; observeBranch(event): void; observeSessionDeleted(s): void }`; it writes `session_txs.outcome` and, when the kind `review.verdict` is registered (5E; a lookup), emits it with `itemKind: none`, `via` and `sessionId` (spec §12.5). **It defines no verdict kind of its own.** Decision 27.

- [ ] **Step 1: Write failing tests** against a real engine: an agent session commits three txs; a person merges its branch into `main` with `fin merge` (no review item) → three `merged`; the agent undoes one of its own → `undone` by `agent`; a person reverts one on `main` → `reverted` by `user`; a second session archived unmerged → `abandoned`; a cherry-pick of one tx into `main` → `merged`; merging a child branch into its own session's root → no outcome (inside the subtree); a `review.merge` (when 5E exists) emits nothing from the tracker (5E already emitted it); a critic session's fix undoing an agent tx → `by: critic`; idempotent under repeated events; **without 5E's kind registered, outcomes are stored in `session_txs.outcome` and nothing is emitted**; `openS` from the injected clock.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): outcome tracking from merges, undos, reverts and abandoned branches`.

### Task 12: RPC handlers, console commands and start-up wiring — tier: sonnet — **[7A-2]**

**Files:**
- Create: `apps/finisherd/src/agent/{rpc.ts, console.ts}`, `apps/finisherd/src/secrets/console.ts`.
- Modify: `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/main.ts` (layers; `recoverAfterCrash` before the transports listen; the hourly sweep), 5A's console dispatcher, `TASKS.md`.
- Test: `apps/finisherd/test/agent-rpc.test.ts`, `apps/finisherd/test/agent-console.test.ts`.

**Interfaces:** console: `finisherd agent start --project <dir|id> [--model] [--role] [--mode] [--base] [--title] [--prompt <text> | --prompt-file <f>] [--budget-usd] [--budget-tokens]`, `agent ls [--project] [--status]`, `agent show <id> [--nodes] [--json]`, `agent prompt|steer <id> <text>`, `agent interrupt|resume|end <id>`, `agent watch <id>`, `agent approve <id> allow|deny [--add <amount>]`, `agent delete <id>`; `provider key set openrouter [--project]|ls|rm` (Decision 24); `provider local set <ollama|lmstudio|url>|show|test|rm` (Decision 24, owner-only); `agent start --provider openrouter|local`; `training closed-targets [pendingCounsel|cleared|excluded]` (Decision 28; no argument shows the state). Text output for people, `--json` for scripts; errors as sp5 §4.6.

- [ ] **Step 1: Write failing tests:** each RPC's happy path and its codes (`SESSION_NOT_FOUND`, `SESSION_BUSY`, `SESSION_ENDED`, `MODE_INVALID`, `MODEL_UNKNOWN`, `HARNESS_UNKNOWN`); `agent.providers` lists `builtin` with whether an OpenRouter key is set (server and per project, never a value) and `fake` only in test builds; `provider key set anthropic` is `PROVIDER_UNAVAILABLE` and `provider key set local` is refused with a fix pointing at `provider local set`; **local endpoint (F1):** `provider local set` validates the URL (`http`/`https`, a host, no credentials inside) and is refused for `local` and `agent` principals, `agent.providers` lists the local preset with whether an endpoint is set and whether it answers (never the URL), `agent start --provider local` with no endpoint set is `PROVIDER_UNAVAILABLE`, and no secret is read or written; **`closed_model_targets_gate`** (default `pendingCounsel`; changing it needs a human principal with the TTY challenge and is audited; `local` and `agent` principals are refused); console `agent watch` renders nodes as lines and exits when the session ends; `provider key set` reads stdin and refuses an argv value; start-up recovery runs before the first connection is accepted.
- [ ] **Step 2: Implement.** The `fake` harness is registered only when `FIN_TEST_HOOKS=1` and the build has test hooks (5A's `check-test-hooks` gate covers it).
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(finisherd): agent RPCs and console commands`.

### Task 13: e2e `agent` scenario — tier: sonnet — **[7A-2]**

**Files:**
- Create: `scripts/e2e/agent.mjs`, `scripts/e2e/fixtures/agent-streams/*.sse`.
- Modify: `scripts/e2e.mjs` (register), `TASKS.md`.

- [ ] **Step 1: Write the scenario.** `finisherd` with `FIN_TRAINING=on` (its own env) and the fake model server; a project from the reel fixtures:
  1. `finisherd provider key set openrouter` (a dummy key on stdin; the fake model server stands in for OpenRouter's base URL) and `agent start --mode draft --prompt "tighten the intro"`;
  2. the scripted model calls `fin view --budget 2000`, then `fin item trim …`, then `fin caption import …`, then ends;
  3. check: the session branch has two commits by `agent:editor:<id>`; `main` is untouched; `fin` with the agent credential cannot commit to `main` (`BRANCH_NOT_ALLOWED`, exit 7); **a `fin item …` with no `--branch` from the session lands on its branch; a person's `fin try` in another shell leaves the session's default untouched; `fin caption import --words /etc/hosts` is `PATH_NOT_ALLOWED`; with `FIN_CREDENTIAL` revoked, `fin` is `UNAUTHENTICATED` rather than falling back to `local`; `session_txs` lists both commits;**
  4. `finisherd diagnostics export --content --kind agent.toolCall --kind log.commit` (content export is owner-only, sp5 §4.6): the call join of Task 10 holds, and the exported turn records are deltas whose replay matches `bodyHash`;
  5. kill `finisherd` during the third tool call; restart; the session is `interrupted`; `agent resume`; the commit count is unchanged and the run completes;
  6. `fin merge --from <branch>` as `local`; both txs have `session_txs.outcome = merged` (and a `review.verdict` entry once 5E's kind exists);
  7. a second session with a 1-cent budget stops with `BUDGET_EXCEEDED` and a budget approval; `agent approve <id> allow --add 1` resumes it;
  8. **the local preset (F1):** `provider local set` with the fake model server's URL in its local mode and `agent start --provider local` on an empty `SecretStore`: the session completes, the secrets file is never opened (a spy), the exported records say `provider: local` with a table family name or a keyed hash and `weights` `open` or `unknown`, no cost, and no record holds the URL.
- [ ] **Step 2: Run** `rtk pnpm e2e` on Linux → PASS. **Commit** `test(e2e): agent session from prompt to merged verdict with crash and budget`.

### Task 14: Spec, architecture pointer and tracker — tier: sonnet — **[7A-1 and 7A-2: one pass after each]**

**Files:** Modify: `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (§3–5, §10, §12, §13 rewritten "as built" for 7A-1 and 7A-2 in turn; status line; the closing note on 7C's policy, launcher and suites being startable now), `docs/specs/2026-11-01-finisher-architecture-design.md` (a §27 item pointing to sub-project 7 §1–13 for 7A, and replacing §21.1's "Reserved (v0) for sub-project 7" list by a pointer), `README.md` (running an agent from the console), `TASKS.md` ("Follow-ups for 7B/7C" from this plan's leftovers).

- [ ] **Step 1: Write** the as-built sections from the code (names, defaults, codes), and the follow-ups.
- [ ] **Step 2: Run** the gates → PASS. **Commit** `docs: record sub-project 7A as built`.

### Task 15: Cross-platform validation with real model APIs (controller-run; needs hosts and keys) — tier: opus (controller) — **[7A-2]**

- [ ] **Step 1:** On macOS, Windows 11 (as a non-elevated user) and the Linux VM: `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e`.
- [ ] **Step 2:** With the owner's OpenRouter key, one real session on a closed model and one on an open-weight model on the reel project: the edit lands on the session branch, the turn's `costMicros` matches OpenRouter's activity page within rounding, caching shows cache reads on the second call (for the model marked `caching: explicit`), `weights` and `upstream` are recorded as expected, and the journal's call join holds. Also, with a local server (Ollama or LM Studio) holding one open-weight model, one real session: it completes or fails cleanly (`MODEL_UNKNOWN {noTools}` for a model without tool support), `provider: local` and `weights` `open` or `unknown` are recorded, no cost is charged and no record holds the base URL. Record measured latencies and costs in `archive/` and only the conclusions in the spec.
- [ ] **Step 3:** Record results in TASKS.md; fix waves as needed; final whole-branch review. **Commit** `docs: 7A validation results`.
