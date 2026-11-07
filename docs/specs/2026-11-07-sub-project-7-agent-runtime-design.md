# Sub-project 7: Agent runtime — Design

- **Status:** revision 2 (2026-11-07). Answers an independent review of revision 1 (§23 maps every finding), applies the coordinator's cross-design ownership decisions (§1.6) and reconciles with sub-project 5 revision 3 (§1.7). Nothing here is built yet.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §8 (agent runtime), §9 (`fin`), §10 (hooks), §16 (secrets, audit), §21.1 (the reserved agent kinds, `FIN_CALL`), §22 (agents, sandbox) and §27.5–27.7 (spikes S7, S8). Where the two disagree, this document wins for sub-project 7, and arch §27 gets a pointer when each batch merges.
- **Builds on:** sub-project 4 (4A–4C on stacked branches), the diagnostics hooks D1–D7 (`docs/superpowers/plans/2026-11-07-diagnostics-and-training-data-hooks.md`, cited as *D-plan Decision n*), and the designs of sub-project 5 (`docs/specs/2026-11-07-sub-project-5-control-plane-design.md`, *sp5 §n*) and sub-project 6 (`docs/specs/2026-11-07-sub-project-6-media-engine-design.md`, *sp6 §n*). Both are drafts on their own branches (sub-project 8, `sp8-design`, *sp8 §n*, is the third sibling); this design assumes them as written and lists what it needs from them (§1.4) and asks of them (§1.7).
- **House style:** as `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`. Each batch gets its own plan; each batch section is rewritten "as built" when it lands.

---

## 1. Goal, scope and batches

### 1.1 Goal

Sub-project 7 makes agents first-class editors of a Finisher project:
- **three ways to run an agent behind one contract:** the bundled Codex app-server, the user-installed Claude Code CLI, and Finisher's own loop on user API keys (Anthropic Messages, OpenAI Responses, one OpenAI-compatible adapter), all emitting the same events (D7, D28, D30);
- **sessions** that are persisted, interrupted on a crash, resumable from any device, each on **its own branch** and identified to the server by its credential (the agent principal's id is the session id, §5.2), with **subagents** on child branches and **roles** and **modes** that map to sub-project 5 scopes;
- an **OS sandbox** (`fin-sandbox`, profiles `agent`, `hook`, `worker`, `plugin`) around every agent process tree on macOS, Linux and Windows, which is the guarantee (arch §27.6), with provider permission rules as a second layer. Its job is defensive: an agent (or a prompt injected into one) must not read the user's secrets, other projects or other sessions, nor persist anything that later runs outside the sandbox;
- **hooks** on the events of arch §10 (v1 cuts: `PreOp`/`PostOp` and `modify` replies, §9.1), **workspace artifacts** registered with provenance, the **full `fin`** (`apply`, `batch`, `context`, `guide`, subagents, `mcp`; no `fin tx`, §11.2), **budgets** for tokens, money and render minutes, and an **eval harness** that scores agents' edits;
- **training capture:** every agent turn, tool call and result as deltas in a dedicated agent-trace store (own cap and retention, §12.1), and every verdict on an agent's edits (from 5E's `review.verdict`, §12.5), recorded under the consent of D34 and joined to the commits they caused through `FIN_CALL` and txIds, so a later dataset builder can fine-tune an open-weight video-editing model on the outputs it may legitimately use (Owner question 1).

### 1.2 Non-goals

- **The conversation UI, approval cards and the branch bar** (sub-project 9). §3.5 and §7.3 define the RPCs and streams they consume.
- **The critic's rubrics, the QA gate and review markers** (sub-project 10). Sub-project 7 provides the `critic` role, read-only sessions and child-branch fixes; sub-project 10 provides what the critic looks at and how it scores.
- **The review workflow itself** (sp5 5E), and **verdicts**: 5E's `review.verdict` is the one source (§12.5). Agents request reviews; people decide them.
- **Plugin-provided providers, roles and hook handlers** (sub-project 8). The contracts here accept them; nothing loads them yet. (The `plugin` sandbox profile *is* in scope: §8.10.)
- **Session fork, `fin tx`, a user-selected newer Codex, a Docker uid sandbox, `PreOp`/`modify` hooks, reading the macOS unified log, a structure-distance eval score, and an encrypted secrets file** (cut in revision 2; §23).
- **Uploading anything and the dataset builder** (D34 phasing). §12 only records, and keeps the builder's join possible.
- **Hosted model credits and metering** (`finisher-cloud`, arch §5 `ModelGateway`, `Metering`).
- **An egress allow-list proxy** (Owner question 5).

### 1.3 Success, for every batch

- Every capability is driven and tested through `fin` → `finisherd`, or through `finisherd` console commands (sp5 §4.6) for the human side, with **fake providers** (§16). Real providers run only in validation tasks and in the eval harness, never in CI.
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS, Windows 11 and the Linux VM; the sandbox escape suites (§8.11) pass on all three.
- No UI work. No host names, IPs, user names or key names in tracked files, fixtures and recorded transcripts included.

### 1.4 Batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **7A-1** Pure runtime pieces (before 5A) | agent kinds v1, the model catalogue and budget ledger, `SecretStore` with the `seal`/`open` seam, the model API clients and the fake model server, the pure session state machine, the adapter contract and `fake.ts`, and the `FIN_CALL` idempotency key in `fin` | D1–D7 only | 7A-2 |
| **7A-2** Runtime core and the built-in agent (after 5A) | contracts and RPCs, the session store, the runtime engine, sessions on their own branches with minted agent credentials (principal id = session id), mode scope sets, the built-in loop and its `fin` tool with per-session default branch and confined file arguments, journal and agent-trace emission, outcome tracking into 5E's `review.verdict`, `finisherd agent …` console commands | 5A, 7A-1 | everything below |
| **7B** Subagents, roles, rules, approvals | session rollback (§5.3), roles as data, `fin agent spawn|wait|ls|send|stop`, budget reserves for children, saved prompts, command-pattern rules enforced on the server through the exported registry, approvals answerable from any device, review requests and verdicts from 5E, context compaction for the built-in loop | 7A, 5E | 7D, 7E, 10 |
| **7C** OS sandbox | `fin-sandbox` (policy, launcher, the `agent`, `hook`, `worker` and `plugin` profiles), Seatbelt, Landlock + seccomp, AppContainer with the ACL broker, per-session directories and provider config dirs, the agent listener, violation reporting, escape suites in CI, the `Spawner` for 5C, the `worker` profile for sp6 6G and the `plugin` profile for sp8 | policy, launcher and suites: nothing (they can start now); wiring into sessions: 7A-2, 5A | 7D, 7E, 7F, sp6 6G, sp8 8B |
| **7D** Claude Code adapter | detection, version floor and capability probe, stream-JSON with the control protocol, permission prompts, interrupt and resume, the Windows brokered shell tool, `--bare` API-key sessions, settings isolation | 7C (7B for approvals) | 7H |
| **7E** Codex adapter | the bundled app-server (the pinned version only), the vendored T3 client checked against a schema generated from the pinned binary, sign-in over the protocol, approvals, resume and rollback | 7C (7B for approvals) | 7H |
| **7F** Hooks | the arch §10 events except `PreOp`/`PostOp`, `allow|block|context` replies, command handlers in the `hook` profile, project trust, audit and timeline entries | 7C, 5C (`JobDone`), 5E (merges) | 8, 10 |
| **7G** Workspace artifacts and the full `fin` | `fin asset add|update` from `workspace/` with provenance, the drafts bin, `fin workspace ls|gc`, `fin apply`, `fin batch` over every write, `fin context`, `fin guide`, `fin note`, `fin mcp`, `fin budget`; wiring of `fin look`/`listen`/`render` | 7A-2, 5C, sp6 6A (6C, 6D, 6E for listen, SVG/Lottie validation, render) | 7H, 11 |
| **7H** Eval harness | tasks (≥ 20, real permissively licensed speech), scorers, runners per provider and model, paired comparisons with a bootstrap, reports in `archive/evals/` | 7D, 7E, 7G | 11 |

**Order:** **7A-1 now** (it needs D1–D7 and nothing of 5) → (5A lands) → 7A-2 → 7B → (7D ∥ 7E) → 7F → 7G → 7H, with **7C starting now, in parallel**: its policy type, launcher, per-OS profiles and escape suites need only the new `fin-sandbox` crate and the spike of §8.12, not the runtime or 5A. Only the *wiring* (a session's policy, the agent listener, the ACL broker's pipe SIDs) waits for 7A-2 and 5A. 7D and 7E touch disjoint adapter directories and can run in parallel. 7G's sub-project 6 parts land as those batches do; its `fin` parts that need nothing from 6 (`apply`, `batch`, `context`, `guide`, `note`, `mcp`) can move ahead of 7F.

**What 7 needs from 5 and 6, precisely:**
- **5A:** principals, `control.db`, the audit log, the `agent` credential format and verifier, `FIN_CREDENTIAL`, scopes and `HUMAN_ONLY`, exit 7, and **a mint service that accepts a caller-supplied credential id** (7A-2 passes the session id, so the principal id *is* the session id, §5.2). 7A-2 mints agent credentials through the 5A store (sp5 §4.1 "sub-project 7 mints").
- **5C:** the `JobDone` bus (hooks, 7F) and job ownership by principal (render-minute budgets).
- **5E:** `review.request`, **`review.verdict` (the single source of verdicts, §12.5)** and protected branches (7B; landing in 7A works through a person's `fin merge`).
- **6A/6C/6D/6E:** `fin look`, `fin listen`, SVG sanitising and template checks, `fin render` (7G). **6G** consumes 7C's sandbox for workers (sp6 Owner question 6).

### 1.5 What exists today (the baseline)

- **`fin`** (`crates/fin`): one registry (`registry.rs`, `registry_edit.rs`, `registry_settings.rs`) generating the map (`map.rs`, < 1.5k tokens, asserted), `fin help` and `fin schema`; `fin batch` over four commands (`BATCHABLE`); `fin view --budget` (`view/budget.rs`, plan 4C Decision 46); stable exit codes 0/2/3/4/5/6 (`output.rs`); the endpoint from `FIN_SERVER` or `finisherd.json` (`endpoint.rs`). D6 adds actor headers from `FIN_SESSION`/`FIN_CALL`; 5A adds the handshake, `FIN_CREDENTIAL` and exit 7. Three facts about today's `fin` matter for agents: the default branch and the `fin try` list live in the shared `fin-config.json` in server data (`crates/fin/src/commands/mod.rs:78-80`, `config.rs:24-30`, `try_cmd.rs:73-74`), so one agent's `fin try` would change what every other `fin` run on the project writes (§5.1); `Tx.author` comes from `$FIN_AUTHOR` (`commands/mod.rs:107-110`), so an agent could write as someone else (§5.2); and some commands read files named on the command line by `fin` itself, in the client (`caption import --words`), outside any server-side path check (§5.6).
- **`finisherd`:** `childEnv()` (`env.ts`) with the arch §27.7 allow-list and `FIN_SERVER`/`FIN_SESSION`; no agent code.
- **`fin-core::events`:** the reserved v0 kinds `agent.session`, `agent.turn`, `agent.toolCall`, `agent.toolResult`, `agent.editVerdict` (`payloads/agent.rs`), the `Agent` source, `Body` (inline or blob), and the tables `AGENT_PROVIDERS` and `AGENT_MODELS` (`names.rs`).
- **Spikes:** S7 proved both CLI protocols (arch §27.5); S8 proved Landlock and AppContainer against 90 escapes each, and decided the Windows shell tool is brokered (TASKS.md, S8 follow-up). The installed CLIs on the Linux VM today are Codex CLI 0.167.0 and Claude Code 2.1.319.

### 1.6 Cross-design ownership (coordinator decisions, 2026-11-07)

| Owner | Owns | Others' part |
|---|---|---|
| sp5 (5C) | the job system: `fin-jobs` (protocol types, the envelope) and the **single `fin-workers <family>` binary**, the scheduler, the kill tree, warm workers | 5C spawns every process through a `Spawner` that **sp7's `fin-sandbox` implements** (§8.10); unsandboxed with the env allow-list until it exists |
| sp6 | the **`media` worker family** and the **shared-memory crate** (`fin-shm`) | sp6's `worker` profile requirements (sp6 §3.5) are met by §8.10; 7 never reads a mapped file |
| **sp7** | **`fin-sandbox` and the ACL broker**, with the profiles **`agent`, `hook`, `worker` and `plugin`** | sp8's `plugin` profile requirements (sp8 §16.1) and sp6's worker needs are the contract of §8.10 |
| sp8 | plugin processes, manifests, the plugin command registry | plugin commands are named **`plugin:<id>/<cmd>`** in command rules (§7.2); sp8's `hooks:modify`/`PreOp` is not built in v1 (§9.1) |

### 1.7 Reconciliation with sub-project 5 revision 3

- **The socket lives in `<server-data>/run/fin.sock`** and nothing else of server data is reachable from a sandbox (sp5 §4.1, §4.11). Seatbelt names that one path; the Linux agent listener (§8.3) exists only because Landlock cannot mediate pathname sockets and seccomp closes `AF_UNIX`.
- **Windows reuses 5A's native named-pipe listener in `fin-node`** (`PipeServer`, first-instance flag, explicit DACL, peer SID check). 7C adds per-project container SIDs through `PipeServer.allow(sid)`; there is no second pipe implementation (§8.4).
- **The HMAC handshake is transport-independent** (sp5 §4.2): the Unix socket, the pipe and the Linux agent listener all run `server.hello`/`auth.prove` with the credential key; the canonical `endpoint` (`tcp:127.0.0.1:<port>` for the listener) is inside both MACs. 7 adds no authentication of its own.
- **`FIN_CREDENTIAL` discovery:** `fin` looks for the key in `FIN_CREDENTIAL`, else `local.key` (sp5 §4.8). **7 tightens this for agents: when `FIN_CREDENTIAL` is set, `fin` never falls back to the local key** (§8.5).
- **Human-only operations are in 5A** (sp5 §4.6, console with TTY and typed challenge). 7 makes them hold (the sandbox denies `auth/`, §7.5) and adds its own (§7.5).
- **What 7 asks of 5** (each is a one-line change in 5's plan, recorded here so neither side forgets):
  1. **5A:** the mint service takes an optional `id` (the session id); agent credential expiry on a mint from 7 may exceed the 24 h idle / 7 d absolute defaults, and a resume refreshes it.
  2. **5A:** `project.open` and `project.create` can be left out of a credential (today `project.open` rides on `project.read`/`project.write`, sp5 §4.5); for an `agent` principal the stage-4 reach rule also refuses a directory whose project is not in `projects`.
  3. **5A:** `Tx.author` for `agent` and `mcp` principals is set by the server from the credential's `author`, and `fin-author` is ignored (§5.2); `auth.whoami` returns the principal's `branches` and `defaultBranch` (§5.1).
  4. **5A:** the pending class: `APPROVAL_REQUIRED` from the `ask` stage becomes **`APPROVAL_PENDING`, exit 8** ("wait, then repeat"), because exit 7 means "do not retry" (§7.3).
  5. **5E:** `review.verdict` gains `verdict: undone|reverted|abandoned`, `via`, `sessionId?` and `itemKind: none`, so outcomes outside a review item have one home (§12.5).

---

## 2. Principles

1. **The sandbox is the guarantee; everything else is a second layer.** Provider approval policies can be fooled (arch §27.6). Scopes stop what an agent asks the server; the OS sandbox stops what its processes do to files; neither depends on the model's good behaviour.
2. **An agent is a principal, never the user.** Every agent session has its own `agent` credential (sp5 §3.2), whose id is the session id and whose scopes come from its mode. Nothing an agent runs can obtain the `local` or `console` key, and `fin` under an agent credential never falls back to them (§8.5).
3. **Agent edits land on branches and reach protected branches only through a person** (D14). A session writes its own branch subtree; `main` changes by review or by a person's merge.
4. **One event model for every provider.** CLI agents and API models produce the same nodes, journal entries and budgets; what a provider cannot report is marked as such, never invented (§12.6).
5. **What the product needs is stored as product state; what training needs is a copy under consent.** Conversations are kept for resume and display whatever the consent; the journal records its copy only as D34 allows (§12.1).
6. **Every link a dataset needs is made at the source.** Tool calls carry the `FIN_CALL` id that their commits carry; every agent commit is recorded against its session by the server whatever the call attribution (`session_txs`, §5.2); when the call cannot be set exactly, the attribution says so (§12.3).
7. **Fakes first.** Every adapter is built against a fake that speaks its protocol, including its failure modes, before a real provider runs (§16).
8. **Every error is `{code, message, details, fix?}`** with a stable `fin` exit code.
9. **Defence in depth against our own agents is the point, and the tests prove denial.** The sandbox suites (§8.11) pass only with an *observed* denial; no protection is described without a test that would fail without it.

---

## 3. Runtime model

### 3.1 Where the runtime runs

- **In `finisherd`** (TypeScript, Effect 4), under `apps/finisherd/src/agent/`, as arch §7 assigns orchestration to TS. It owns sessions, the adapters, budgets, approvals, hooks and the verdict tracker.
- **Rust** provides the sandbox launcher and policy (`crates/fin-sandbox`, 7C), the Windows ACL broker (in `fin-node`, 7C), the eval scorers (`crates/fin-eval`, 7H), and the event types (`fin-core::events`).
- **Agent processes** (a provider CLI and everything it spawns) are children of `finisherd`, each tree started through `fin-sandbox` (from 7C) with `childEnv()`. The built-in agent has no process of its own: its loop runs in `finisherd` and its tools spawn `fin` (§4.4).

### 3.2 Sessions, runs and nodes

T3's vocabulary (arch §8.1):

```text
Session {id ULID, projectId, harness, provider, model, role, mode, branch, parent?, status,
         budget, credentialId, providerRef?, createdBy {principal, actor}, createdAt, title?}
Run     {id ULID, sessionId, seq, trigger: prompt|steer|subagentResult|reviewFeedback|hook|eval,
         status, startedAt, endedAt?, headAtStart (branch seq), stopReason?, usage}
Node    {id ULID, runId, seq, kind, providerId?, payload, createdAt}
  kind: userMessage | assistantMessage | reasoning | plan | toolCall | toolResult | approval
        | subagent | usage | notice | error
```

- A **session** is one agent with one harness and model, rooted on one branch (§5.1). A **run** is one user prompt (or one other trigger) and everything the agent does until it stops. A **node** is one item of the conversation.
- **Status:** `starting` → `idle` ⇄ `running` → `ended`; `running` → `waitingApproval` → `running`; any live state → `interrupted` on a provider exit, a `finisherd` crash or a budget stop; `interrupted` → `idle` by resume. `ended` is final (by a person, by the agent finishing a subagent brief, or by a fatal error).
- **`toolCall` nodes get a Finisher call id** (a ULID) at the moment the provider announces the call. That id is `FIN_CALL` (§12.3). The provider's own id is kept as `providerId` (it is not a ULID, so the journal keyed-hashes it).

### 3.3 Storage

- `control.db` (sp5 §3.1) gains `sessions`, `runs`, `nodes`, `approvals`, `budgets`, `session_txs` and `session_state` tables in a sub-project-7 migration chain (`agent`, independent of 5's, sp5 §3.1). **Node payloads up to ~4 KiB are stored inline**; larger ones (tool output, file contents, images, long messages) go to `<server-data>/agents/<sessionId>/blobs/<blake3>` and the node holds a reference, so the hot rows and the WAL stay small (sp5's 1 ms statement budget).
- **This is product state, not telemetry:** it exists so a session can be resumed, shown on another device and audited by its owner. It is not gated by consent or by `collect: never`, never uploaded, and deleted with the session (`agent.session.delete`, owner-only) or by the session retention setting (default: 90 days after `ended`). **Diagnostics purge and export do not touch conversations**, and the dataset builder never reads `control.db` (§12.1); sub-project 9's "what we collect" page must say so in those words.
- `<server-data>/agents/<sessionId>/` also holds the per-session directories of §8.5 (credential, temp, provider state). It lives on the server's local disk, never in the project folder (arch §6.7, sp5 principle 5).
- `workspace/<sessionId>/` in the project folder stays the agent's scratch (arch §6.1, D22).

### 3.4 Crash and resume

- On start, `finisherd` marks every session that was `running`, `waitingApproval` or `starting` as `interrupted` with reason `serverRestart`, and every open run as `interrupted`.
- **Resume** reopens the provider with its own resume reference (`providerRef`): Claude Code's session id (`--resume`), Codex's thread id (`thread/resume`), or, for the built-in agent, the messages rebuilt from the nodes. A run interrupted mid-tool gets a synthetic `toolResult` node `{ok: false, code: SESSION_INTERRUPTED}` so the model sees what happened.
- **Never re-run a tool call on resume.** A commit that may have landed is found by its idempotency key: the built-in `fin` tool passes `--idem-key <callId>` on writes, so a retried call is a dedupe, not a second edit.

### 3.5 Streams and RPCs

| RPC | Scope | Notes |
|---|---|---|
| `agent.session.start {projectId, harness, model?, role, mode, base?, title?, prompt?, budget?}` | `agent.run` | creates the branch and credential (§5) and optionally the first run |
| `agent.session.prompt {sessionId, text, attachments?}` | `agent.run` | starts a run; `SESSION_BUSY` while one runs (use `steer`) |
| `agent.session.steer {sessionId, text}` | `agent.run` | into the running turn where the provider supports it, else queued |
| `agent.session.interrupt`, `.resume`, `.end {sessionId}` | `agent.run` | interrupt keeps the branch (arch §8.6) |
| `agent.session.rollback {sessionId, toRun}` (7B) | `agent.run` | §5.3 |
| `agent.self {}`, `agent.self.try {action: record|setDefault|clear, branch}` | `project.read` / `project.write`, agent principals only | the calling session's own default branch and try list (§5.1) |
| `agent.session.list {projectId?, status?}`, `.get {sessionId}` | `agent.read` | |
| `agent.watch {sessionId, fromNode?}` (stream) | `agent.read` | nodes, deltas (message and tool output, ≤ 20 Hz), status, usage, approvals |
| `agent.session.delete {sessionId}` | `agent.admin` (owner) | removes product state; the journal keeps what consent allowed |
| `agent.approval.list`, `agent.approval.answer {id, decision, remember?}` | `agent.read` / `agent.approve` (human) | §7.3 |
| `agent.providers {}` | `server.read` | detection, versions, sign-in state, capabilities (§4.1) |

- **New scopes** (added to sp5's closed vocabulary): `agent.read`; `agent.run` (start and drive sessions: people, and in 7B a parent agent for its own children only); `agent.approve` (human); `agent.admin` (owner: delete sessions, provider settings, sign-in, unsandboxed overrides).
- Partial deltas are streamed but not stored; the completed node is.

---

## 4. Providers

### 4.1 The adapter contract

Forked in shape from T3's `ProviderAdapterV2` (arch §8.2), written in Effect 4:

```ts
interface ProviderAdapter {
  readonly harness: Harness;                 // "builtin" | "claudeCode" | "codex" | "fake" | plugin id (8)
  detect(): Effect<Detection, ProviderError>; // installed, version, supported range, signed in, capabilities
  open(spec: SessionSpec): Effect<ProviderSession, ProviderError, Scope>;
}
interface ProviderSession {
  readonly events: Stream<ProviderEvent>;     // one ordered stream per session
  startTurn(input: TurnInput): Effect<void, ProviderError>;
  steer(input: UserInput): Effect<void, ProviderError>;
  interrupt(): Effect<void>;
  respond(requestId: string, answer: RuntimeAnswer): Effect<void, ProviderError>;
  rollback(toTurn: number): Effect<void, ProviderError>;   // capability-gated (7B)
  ref(): ProviderRef | null;                  // what resume needs
}
interface Capabilities { steer; rollback; resume; reasoning; images; parallelTools;
                         costReported; exactCapture; nativeSubagents }
```

- **`ProviderEvent`** is a closed union: `turnStarted`, `messageDelta`, `message {role, blocks}`, `reasoningDelta`, `reasoning {text?, summary?, encrypted?}`, `plan`, `toolStarted {providerId, tool, input}`, `toolOutputDelta`, `toolCompleted {providerId, ok, output}`, `request {id, kind: approval|userInput, tool?, input?}`, `usage {input, output, cacheRead, cacheWrite, reasoning, costMicros?}`, `turnCompleted {stopReason}`, `turnFailed {error}`, `exited {code, signal}`, and `capture {request, response}` (the exact model traffic, built-in only). The runtime turns them into nodes; adapters never write nodes.
- **Binding** (arch §27.5): every event is bound to the process (or HTTP stream) that emitted it; events from a replaced process are dropped; a spawn failure or exit fails the pending turn with `PROVIDER_EXITED`.
- **Unknown events become notices.** A CLI message the adapter does not know (a new event type from a newer CLI version) is recorded as a `notice` node with its type name and never fails the turn; only a message the adapter *needs* and cannot parse is `PROVIDER_PROTOCOL` (§4.5 version policy).
- **`SessionSpec`** carries the model, the role prompt (§6.2), the tool set, the mode, the working directory `workspace/<sessionId>/`, the session environment (§8.5), the sandbox policy (§8), the budget remaining, and the provider ref when resuming. **Budget facts are never part of the system prompt**: they go in a message at the start of each run (§4.4), so the system prompt stays byte-stable, cacheable and deduplicated in the trace store.

### 4.2 CLI agents and API models

| | Built-in (API) | Claude Code (CLI) | Codex (CLI) |
|---|---|---|---|
| Process | none; HTTP from `finisherd` | `claude -p` per session, stream-JSON on stdio | `codex app-server`, newline-delimited JSON-RPC on stdio |
| Auth | Finisher's `SecretStore` via `ModelGateway` (§10) | **an API key by default** (`--bare` + `apiKeyHelper`, §4.5); the CLI's own subscription login only if Owner question 6 allows it, in a per-session config dir (§8.6) | an API key passed over the protocol (§10); the CLI's own login only if Owner question 6 allows it, in a per-session `CODEX_HOME` (§8.6) |
| Tools | Finisher's: `fin`, `workspace_*` (§4.4) | its own (Bash, Read, Edit, Grep, WebFetch…), restricted by flags and by the sandbox | its own (shell, apply_patch, …), likewise |
| Streaming | SSE from the API | `stream_event` lines with `--include-partial-messages` | `item/*` and `turn/*` notifications |
| Approvals | the runtime decides before running a tool | `can_use_tool` control requests via `--permission-prompt-tool stdio` | `*/requestApproval` → `accept|decline` |
| Usage and cost | exact, from the response | reported per result (`usage`, `total_cost_usd`) | token usage notifications; cost priced from the catalogue |
| Capture fidelity | exact requests and responses | the stream: messages, tool calls and results, thinking where emitted; no system prompt | the stream: items, reasoning summaries; no system prompt |
| Rollback | Finisher-side | undo the session's transactions, resume (provider history is left as is) | `thread/rollback` where offered |

### 4.3 Tool protocol: `fin`, not MCP, with MCP as an adapter

- **The built-in agent gets one native tool, `fin {argv: string[], stdin?: string}`**, plus `workspace_read`, `workspace_write` and `workspace_list` confined to `workspace/<sessionId>/`. Finisher executes `fin` directly (no shell, no quoting, identical on Windows), with `FIN_CALL` set to the call's id. The system prompt carries the `fin` map (< 1.5k tokens) and the role's skills; `fin help <group>` and `fin schema` are there for the rest. One tool keeps the tool definitions small and cacheable; generated per-command tools are an eval variant (7H), not the default.
- **CLI agents use `fin` through their own shell** (D16): the binary is on the session `PATH`, the session env names the credential (§8.5). This is the path every agent already knows, and the one the eval harness measures.
- **`fin mcp`** (7G) is a stdio MCP server generated from the same registry (arch §9.12): one MCP tool per command group, arguments as the `fin schema` input, output as the same JSON. It is offered to CLI agents as configuration (`--mcp-config` for Claude Code, `mcp_servers` for Codex) and becomes the default per provider only if the eval harness shows it better (fewer calls, fewer errors at equal success). It also gives exact `FIN_CALL` attribution (§12.3), because `fin mcp` mints the call id per request.
- **Rejected:** MCP as the primary interface (contradicts D16; schemas for every command cost tokens on every turn; MCP adds a long-lived process per session); provider-native tool definitions written per provider (three copies of the registry).

### 4.4 The built-in agent

- **Loop:** prompt → model call (streamed) → tool calls (read-only ones may run in parallel, writes in order) → results appended → next call, until `end_turn`, the per-run cap on model calls (default 60), a budget stop or an interrupt.
- **Model APIs** (D30): Anthropic Messages (`@anthropic-ai/sdk`), OpenAI Responses (`openai`), and one OpenAI-compatible Chat Completions adapter (`openai` with a base URL) for OpenRouter, Ollama, LM Studio and similar. Finisher builds each request body itself and records it exactly (`capture` events); the SDKs are transport, streaming and retry.
- **Prompt caching:** Anthropic cache breakpoints on the system prompt and tools and on the last stable message; OpenAI caching is automatic. Cache tokens are reported separately and priced as the catalogue says.
- **Budget facts in a message.** Remaining tokens, money and render minutes, and the run's call caps, go in a short user-role message at the head of each run (and again after a budget raise), never in the system prompt: the system prompt and tools are identical across a role's sessions, which keeps the prompt cache warm and lets the trace store keep one copy (§12.1).
- **Context:** tool results over 16 KiB are cut to a head and tail with a note naming the node (the full output stays in the node and, under training, in the trace store). `fin view` is called with a `--budget` derived from the remaining window (at most 4k tokens or 5 % of the window). At 85 % of the window the run stops with `MODEL_CONTEXT_EXCEEDED` in 7A; 7B adds compaction (a summary turn that replaces older runs, recorded as a node and in the journal).
- **Retries:** 429 and 529/overloaded are retried with the server's `retry-after` or backoff (1 s × 2ⁿ, ≤ 60 s, 5 attempts); other 4xx fail the run with the mapped code (§15).

### 4.5 Claude Code (7D)

- **Launch** (arch §27.5; flags verified against 2.1.319): `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --model <m> --settings <session settings file> --strict-mcp-config [--mcp-config <fin mcp>] --append-system-prompt <role prompt> --disallowedTools <list> --session-id <uuid>`; resume with `--resume <id>`.
- **`--bare` for API-key sessions.** Verified: `--bare` accepts only an API key or `apiKeyHelper`; it skips hooks, plugin sync, auto-memory and the discovery of project and user `CLAUDE.md` files and settings. That is exactly the isolation Finisher wants (an injected `CLAUDE.md` or hook in the project folder is not loaded), and it needs no keychain. API-key sessions therefore always run `--bare` with `apiKeyHelper` in the session settings (§10). A subscription login cannot run `--bare`; it is available only if Owner question 6 allows it, and then the project-level settings sources are disabled with `--setting-sources ""` (or the narrowest value the CLI accepts, pointing at a directory holding only Finisher's file) and the project's `CLAUDE.md` is excluded by the read policy where the CLI allows it.
- **The role goes in `--append-system-prompt`**, never in a `CLAUDE.md` file written into the workspace (the agent could rewrite it and it would outlive the session) and never mixed with budget facts (§4.1).
- **Budget:** `--max-budget-usd` exists **only in print mode** (`-p`), applies per process, and is not the budget authority: it is a backstop set to the session's remaining money, and the runtime's ledger (§13) decides. If a later adapter ever ran the CLI interactively the flag would be absent.
- **Native subagents are disabled** (`Task`) so that every agent has a branch, a credential and a budget (§6.3).
- **Version policy (floor + probe, not a pinned range):**
  1. a **floor version** per Finisher release, checked at detection (`PROVIDER_VERSION {floor}` below it);
  2. a **capability probe**: the first time a (version, flags) pair is seen, `finisherd` starts the CLI under the real sandbox with the session's flags and reads the **`system/init` message** (tools, MCP servers, model, permission mode, slash commands). The adapter requires the fields it uses (`tools`, `session_id`, `model`) and the control-protocol messages `can_use_tool` and `result`; a missing capability is `PROVIDER_VERSION` with the missing item named. (Whether init is emitted before any prompt is checked in the spike of §8.12; if not, the probe sends a one-token no-op prompt under a 1-cent budget.) The result is cached per version;
  3. **unknown events become notices** (§4.1), so a newer CLI that adds a stream event keeps working;
  4. **the disallowed-tool list is built from the init message's tool list**: every tool in `init.tools` that is not in the role's allow-set (the editor's `Read`, `Edit`, `Write`, `Grep`, `Glob`, `Bash` where it exists, `WebFetch` per rules) is passed in `--disallowedTools`, `Task` and anything new included. A tool a future version adds is denied by default until the allow-set names it.
  A recorded-transcript contract suite per *tested* version (§16) backs this; versions between tested ones run on the probe alone, with a `notice` on first use.
- **Settings isolation (Decision 14):** the user's own Claude Code settings, hooks, MCP servers and memory files are not loaded. The session settings file lives in the session directory (read-only to the sandbox); the CLI's config dir is the **per-session directory of §8.6**, never the user's `~/.claude`.
- **Windows:** Git Bash/MSYS cannot start in an AppContainer (arch §27.6), so the Bash tool is replaced by a **brokered shell**: an MCP tool `shell {command}` served by `fin-sandbox` outside the container, which runs PowerShell inside a fresh container for the session and returns its output (TASKS.md S8 follow-up). The built-in `Bash` is disallowed on Windows.
- **Licence and terms:** never bundled; detected on `PATH` or at a configured location (arch §18); run unmodified, with the vendor's flags and settings only. Anthropic's published terms on subscription logins are the subject of Owner question 6.

### 4.6 Codex (7E)

- **Bundled** app-server, the **pinned version only** (Apache-2.0; licence and NOTICE shipped; binary dependencies re-checked at each pin, arch §8.2). The JSON-RPC client is vendored from T3's `effect-codex-app-server` and pinned. A user-installed or user-selected newer `codex` is not used (cut): upgrading is a Finisher release.
- **Drift gate:** `codex app-server generate-json-schema` of the pinned binary is committed; a test checks the vendored client's messages against it.
- **Configuration per session** (`-c` overrides, never the user's `config.toml`): `CODEX_HOME` = the per-session directory of §8.6;
  - **`-c project_doc_max_bytes=0`**: the project's `AGENTS.md` (attacker-controlled content in a shared project) is not loaded (verified: Codex 0.167.0 accepts it);
  - **the role goes in `developer_instructions`** (verified accepted by 0.167.0), not in a file in the workspace;
  - `sandbox_mode = "danger-full-access"` **inside** Finisher's sandbox when nested Seatbelt or Landlock is refused, otherwise `workspace-write` as the second layer (7E verifies which per OS); `approval_policy` routed to the runtime; `shell_environment_policy.inherit = "none"` with `set` = the session env of §8.5, so commands see `FIN_SESSION`, `FIN_CREDENTIAL` and `FIN_SERVER` and nothing else.
- **Sign-in:** an API key sent over the protocol's login call (never in the environment, §10), by default; the user's ChatGPT account through the CLI's own login only if Owner question 6 allows it.

### 4.7 The model catalogue

- Data, bundled and versioned (`apps/finisherd/src/agent/catalogue/models.json`): per model the provider, API, context window, maximum output, capabilities, and prices per million input, output, cache-read and cache-write tokens, with an `asOf` date.
- `fin-core::events::names::AGENT_MODELS` must hold every catalogue id (a drift test); a model outside the catalogue (a local Ollama model) is recorded as `other` plus a keyed hash of its id, so it groups without being stored raw.
- Role defaults pick models per harness (§6.2); the user overrides per session.

---

## 5. Sessions, branches and landing edits

### 5.1 One branch subtree per session

- **Start** creates a branch from `base` (default: the head of `main`; or a branch the starter can read) named `agent/<role>-<n>` (display only; ids stay `br<n>`, 4B). The session's **root branch** is recorded on the session.
- The session may write its root branch and branches it creates beneath it (`fin try`, `fin branch new --from` its own). Writes to any other branch are `BRANCH_NOT_ALLOWED` (exit 7). This is enforced by the agent credential's branch reach (sp5 `agent_sessions.branches`), checked by the same authorisation wrapper as scopes, with branch lineage from `branch.list`.
- **One live session per root branch.** Starting a session on a branch another live session owns is `BRANCH_IN_USE`.
- `plan` mode creates no branch: it reads `base`.
- **With an agent credential, `fin`'s default branch is the session's root, taken from the principal.** `fin` learns it from `auth.whoami` (the principal carries `branches` and `defaultBranch`), so a command with no `--branch` writes the root, never `main`. **Try state is per session and lives on the server** (`session_state {sessionId, defaultBranch, tries JSON}` in `control.db`, read and written through `agent.self` and `agent.self.try`, §3.5), never in the shared `fin-config.json` (`config.rs`, `try_cmd.rs`): under an agent credential `fin` neither reads nor writes that file, so one agent's `fin try` cannot change what a person's or another session's `fin` writes, and a stale `fin-config.json` cannot point an agent at `main`. A `fin try` records its branch (which must be in the session's subtree) and sets the session's default; aborting a try restores the previous default.

### 5.2 The credential, the principal and what is recorded

- **The agent principal's id equals the session id.** `agent.session.start` mints an `agent` principal through 5A's mint service with the **caller-supplied id = the session id** (§1.7), the scope set of the mode (§6.1), `projects: [projectId]`, `branches: [root]` (descendants implied), an expiry that outlasts the session (a resume refreshes it). 5A already makes an agent's actor `{kind: agent, session: <credential id>}` (sp5 §3.4), so the actor session, the journal session, the control.db session and the credential are one id: there is no second identifier to forge or to join on. `FIN_SESSION` in the environment is informational; the server never trusts it.
- It writes the credential file to `<server-data>/agents/<sessionId>/credential.json` (0600; readable inside the session sandbox only, §8.5) and sets `FIN_CREDENTIAL` to its path.
- Ending or deleting the session revokes the credential; a resumed session keeps it (agent keys derive from the master and survive restarts, sp5 Decision 3).
- **`session_txs` records `(txId, sessionId, callId?, at)` for every commit, undo, redo, revert, cherry-pick and merge an agent principal makes**, written by the server's authorisation wrapper after the engine answers (stage 7), keyed by the principal's id, **whatever the call attribution** (§12.3): a commit with `callId` null is still the session's. The verdict tracker, rollback and the journal join read this table, not a header.
- **`Tx.author` is set by the server for agent principals**, to `agent:<role>:<sessionId>` from the credential's `author` (sp5 §3.2 `Principal.author`). **`FIN_AUTHOR` and `fin-author` headers cannot override it** (§1.7 item 3). Because `undo`/`redo` act on "your" edits by author (`commands/history.rs`), an agent's undo can touch only its own session's transactions.

### 5.3 Turn checkpoints and rollback (7B)

- Each run records `headAtStart` (the root branch's seq) and, through `session_txs`, the txIds its tool calls produced.
- **Rollback to run *r*** undoes, newest first, every transaction in `session_txs` after *r* started (4B undo units, as the session's author), then rolls the provider back (`rollback` capability) or, for the built-in agent, drops the later nodes from the next request. A conflicting undo fails the rollback with `CONFLICT` and leaves both sides as they were.
- **Session fork is cut** (v1): a person who wants an alternative starts a new session with `base` set to a branch or checkpoint. Provider-level resume-fork flags are not used.

### 5.4 How edits land

1. The agent works on its branch; every write returns its delta (arch §9.4).
2. At the end of a brief, the agent calls `fin review request --title … [--summary …]` (5E; in 7A, `fin branch ls` shows the branch and the session ends with a summary node). The `BranchReady` hook runs (7F). A review's accept/reject/merge outcomes are 5E's `review.verdict` entries (§12.5).
3. **A person** reviews the change list (5E: per-transaction accept or reject, then merge) or, before 5E, merges with `fin merge --from <branch>` as the `local` principal or from the console.
4. **Agents never merge into a protected branch** (sp5 §8.5: no `branch.protected`, no `review.decide`). In `auto` mode too (Owner question 4).
5. A parent agent may merge a child's branch into its **own** branch (7B), which is how subagent work composes before review.

### 5.5 Concurrency

- Sessions on separate branches run in parallel (D18), limited per server (default 4 live provider processes, 8 built-in sessions; settings) and per provider by its rate limits (§13.3).
- All writes go through the project's single writer (arch §6.5); a rebase conflict on `tx.commit` is answered to the agent, which re-plans (sp5 §8.4: not a review item).

### 5.6 Path reach and the files `fin` reads itself

- **Server-side reach** (sp5 §4.4 stage 4) for an `agent` principal is **its own project and the libraries that project references**, nothing else: the project folder, the session workspace `workspace/<sessionId>/`, and the roots of registered libraries that the project's assets live in. Another project's folder, an unreferenced library and any other directory are `PATH_NOT_ALLOWED` (exit 7), and `project.create` and `project.open` are not in any mode's scope set (§6.1): the session's project is opened by `finisherd` at start. A directory naming another project is `PROJECT_NOT_FOUND`, which does not reveal that it exists.
- **Client-side file arguments.** Some `fin` commands read a file named on the command line in the `fin` process itself, before any RPC (`fin caption import --words <file>`, `fin asset add <path>`, `--spec`/`apply` files). **With an agent credential, `fin` confines every such argument to the workspace and the project folder**: the path is made absolute, symlinks resolved (case-folded on Windows and case-insensitive macOS volumes), and must lie under `workspace/<sessionId>/` or the project folder; otherwise `PATH_NOT_ALLOWED` before the file is opened. This is a convenience layer (a CLI agent's own `Read` tool is bounded by the sandbox, §8, not by `fin`); it exists so `fin` is never a confused deputy that reads a file the agent's own process could not (a file outside the sandbox's read policy would fail in the sandbox anyway, and the built-in agent's `fin` tool runs in `finisherd`, where it is the only bound). The same check runs in the built-in `fin` tool before it spawns `fin`, for arguments it can recognise from the registry, and a test lists every command with a client-side path argument (a drift test over the registry's `argKinds: path`).

---

## 6. Roles, modes, subagents, saved prompts

### 6.1 Modes and their scopes

| Mode | Scopes (sp5 §4.4 vocabulary, plus §3.5) | Sandbox writes | Typical use |
|---|---|---|---|
| `plan` | `server.read`, `project.read`, `library.read`, `jobs.read`, `diagnostics.read` (redacted); `jobs.submit` for `look` jobs only | `workspace/<session>/` only | read, analyse, propose; no ops |
| `draft` | `plan` + `project.write` (own branches, §5.1), `jobs.submit` (proxies, analysis, `look`, `listen`), `review.write`, `agent.run` for own children (7B) | `workspace/<session>/`, `workspace/shared/` | edit on its own branch |
| `auto` | `draft` + `render` and `critic` jobs within the render-minute budget | as `draft` | edit, render, self-review within budget |

- No mode grants a human-only scope, `branch.protected`, `review.decide`, `jobs.control`, `project.admin`, **`project.create`, `project.open`**, `library.admin`, `settings.admin`, `auth.admin` or `plugin.install` (sp5 §4.4 "agent"; §1.7 item 2). A role may narrow its mode, never widen it. Path reach is §5.6.
- **`plan` may write its own workspace** (notes, scratch): `workspace/` is never canonical state (D22). "Read-only" in arch §8.3 means read-only to the project.
- `auto`'s critic needs sub-project 10; until then `auto` behaves as `draft` plus render.

### 6.2 Roles

- **Data, not code** (`apps/finisherd/src/agent/roles/*.json`, later contributed by plugins): `{name, title, prompt (skill references, `fin guide` topics), modeCeiling, models: {builtin, claudeCode, codex}, rules (allow/ask/deny, §7.2), budget, tools}`.
- **Bundled:** `editor` (the default main agent), `critic`, `fixer`, `clip-finder`, `caption-stylist`, `colourist`, `mixer` (arch §8.3).
- **The critic has a `draft` ceiling confined to a child branch** (revision 2: a `plan`-only critic could criticise but never repair, and `plan` creates no branch). A critic session reads any branch of its project; its root branch is a **new child of the branch under review** (`<reviewed>/critic-<n>`), so its credential's reach excludes the reviewed branch itself, and its fixes arrive as a second branch a person (or the reviewed branch's owner, by `fin merge` into its own branch) can take or drop. Alternatively the critic **spawns a `fixer`** (7B, `draft`, same child-branch rule) and only reports. In both cases a critic edit that replaces or undoes an agent's transaction is recorded through 5E's verdict kind with `by: critic`, `via: critic` (§12.5). Default mode for a critic is `plan`; `draft` is chosen by the session starter or by the role's saved prompt. Their prompts are skills borrowed from Uncut (D26), versioned with the server (arch §9.1).
- `AGENT_ROLES` (a new name table) holds the bundled names plus `other` for plugin roles.

### 6.3 Subagents (7B)

- **`fin agent spawn --role <r> --brief <text> [--mode <m>] [--budget <b>] [--model <m>]`** from a running session starts a child session whose root branch is created from the parent's current branch head. The child's mode is at most the parent's; its credential reaches only its own subtree. Returns `{session, branch}` at once.
- `fin agent wait <id>… [--any] [--timeout <s>]` returns each child's final summary node, status and branch; `fin agent send <id> <text>` steers; `fin agent stop <id>`; `fin agent ls` lists the parent's children.
- The parent composes by `fin merge --from <child-branch>` into its own branch. Children cannot spawn beyond depth 3 or past 8 live descendants per root session (settings).
- **Provider-native subagents** (Claude Code `Task`, Codex's equivalents) are disabled: they would share the parent's branch, credential and budget invisibly. The clip finder's "one child branch per short" (arch §13) is this mechanism.
- The UI's session tree (arch §8.1) is `parent` links.

### 6.4 Saved prompts (7B)

`/make-shorts 5` and similar (arch §8.6) are data: `{name, role, mode, template with typed parameters, budget}`, expanded by the server into `agent.session.start` + a first prompt. Only people (UI, console) start them; `fin` stays agent-only.

---

## 7. Permissions, rules, approvals, human-only operations

### 7.1 Layers, from the outside in

1. **OS sandbox** (§8): what processes can touch.
2. **Scopes and branch reach** (sp5, §5.1, §6.1): what the credential may ask the server.
3. **Command rules** (§7.2): allow/ask/deny on command patterns, per role, project and user.
4. **Hooks** (§9): policy code (`PreCommand`, `PreMerge`; `PreOp` is cut in v1).
5. **Provider permission rules** (Claude Code allow/deny, Codex approval policy): a convenience, never relied on.

### 7.2 Command rules on the server

- Arch §4 rule 6 enforces rules "in the command registry, whatever the transport", but an agent with a credential can call RPCs without `fin`. So rules are enforced **in `finisherd`'s authorisation wrapper**, against command names.
- **The registry is exported:** `fin schema --registry` writes every command's name, group, the RPC(s) it calls and the payload fields that identify it; the result is committed as `packages/contracts/registry.json` with a drift gate, like the generated types. The server maps each incoming RPC + payload back to its command name (`commandOf`), so a rule on `render` or `plugin.install` holds whether the call came from `fin`, `fin mcp`, a hand-written client or the built-in tool. **Plugin commands are named `plugin:<id>/<cmd>`** (sp8 §3.8, for example `plugin:@acme/tools/trim-silence`): `commandOf` returns that name for the plugin-command RPC, `fin schema --registry` lists it, and rules match it by glob (`deny plugin:@acme/*`, `ask plugin:*/publish`).
- **Rule syntax:** `{allow|ask|deny: "<command glob>", where?: {branch?, mode?, preset?, bytesOver?, costOver?}}`, layered `managed` > `project` > `user` > `local` > role (arch §8.3); the first `deny` wins, then `ask`, then `allow`; no match is `allow` within the scopes. Project rules are honoured only for trusted projects (§9.4).
- **Defaults:** `deny plugin.install` (arch §8.3), `ask render` over 10 minutes of output in `draft`, `ask asset.add` of a download without a recorded licence (7G), `ask workspace.gc`.

### 7.3 Approvals

- An `ask` (a rule, a budget extension, a provider permission the policy routes to people, §7.4) creates an **approval**: `{id, sessionId, kind: rule|budget|provider|plugin, subject (command and resolved arguments, server-written), argsHash, requestedAt, expiresAt (default 30 min), decision?, by?, remember?}`. The card text is generated by the server from the subject, never worded by the agent (arch §8.3).
- **The request does not block an RPC, and pending is not denied.** `fin` gets **`APPROVAL_PENDING` (exit 8, a new class meaning "wait, then repeat")**, with `details.approval`. Exit 7 stays for "do not retry" (`APPROVAL_DENIED`, `RULE_DENIED`, …); an agent that sees 7 must stop, an agent that sees 8 must wait.
- **`fin approval wait <id> [--timeout <s>]` blocks at most 90 s** (the cap is below every CLI tool timeout: Claude Code's Bash default is 120 s, the built-in tool's is 120 s) and returns **8 `APPROVAL_PENDING` on timeout, which is repeatable**: the agent calls it again. It exits 0 on allow, 7 `APPROVAL_DENIED` on deny or expiry. The built-in loop waits on the approval itself (status `waitingApproval`) and gives the model the outcome as the tool result.
- **A grant is one-time and exact: keyed by `(sessionId, command, argsHash)`**, where `argsHash` is the hash of the resolved arguments the approval card showed. The first matching request consumes it (committed in the same transaction as the write); a second identical request asks again, a request with different arguments asks again, and another session never inherits it. A grant expires with the approval.
- Answered from any device (`agent.approval.answer`, scope `agent.approve`, human only), or the console (`finisherd agent approve <id> allow|deny`). `remember: session|project` adds a **rule** (a separate, visible, revocable object), not a standing grant; adding a rule is human-only.
- Approvals are product state in `control.db` and journal entries (`agent.approval`, §12.2).

### 7.4 Provider permission requests

- Claude Code `can_use_tool` and Codex `requestApproval` are answered by the runtime: **allow** what the sandbox contains (file edits in the workspace, shell commands, reads), **deny** what the mode forbids (writes outside the workspace in `plan`), and route to an approval what a rule marks `ask` (by tool and pattern, for example `WebFetch` or `Bash(curl *)`).
- The decision is recorded as an `approval` node with `by: policy|user`.

### 7.5 Human-only operations

sp5 §4.5 makes consent changes, content export, purge, `project default`, confirmed takeovers, relocations, library administration, pairing and plugin installation human-only in 5A, ahead of the arch §21.1 phasing. Sub-project 7 adds what only it can:
- **making them hold:** the sandbox denies agents read access to server data (`local.key`, `console.key`, `credential.master`, `settings.json`, the journal, the agent-trace store, the audit log, `control.db`), so an agent cannot obtain a human credential (sp5 §4.6 residual), and `fin` under an agent credential never falls back to the local key (§8.5);
- **new human-only operations:** answering approvals; starting, ending, deleting sessions on behalf of a person (`agent.run` is human, except a parent for its own children); provider sign-in and API keys; `agents.unsandboxed` overrides (Owner question 3); trusting a project's hooks; adding command rules (`remember`).

---

## 8. The OS sandbox (7C)

Everything in this section is **defensive hardening of Finisher's own sandbox**: it describes what an agent's process tree is *prevented* from doing and the tests that must observe the prevention. It contains no attack procedures; the escape suites (§8.11) are written as "attempt X, observe denial" and live in the repository's tests.

### 8.1 The policy

One pure type in `fin-sandbox::policy`, built by the runtime per session, per hook command, per job and per plugin call, applied by the launcher per OS:

```rust
pub enum Profile { Agent, Hook, Worker, Plugin }          // §8.10
pub struct SandboxPolicy {
    pub profile: Profile,
    pub write: Vec<PathBuf>,        // workspace/<session>, workspace/shared, the session dir's tmp/ and provider/
    pub read: ReadPolicy,           // Only(Vec<PathBuf>) for every profile; Everything{except} exists for tests and the owner override only
    pub protect: Vec<PathBuf>,      // .finisher/, assets/, finisher.project.json: no write, no link (arch §27.6)
    pub deny_read: Vec<PathBuf>,    // a second layer inside Only: server data (except the session dir), sensitive home paths
    pub unix_sockets: Vec<PathBuf>, // the only pathname sockets it may connect to (macOS: run/fin.sock)
    pub net: Net,                   // Open | Loopback{ports} | None
    pub mach: Vec<String>,          // macOS: the explicit mach-lookup allow-list (empty = nothing)
    pub devices: Vec<DeviceRule>,   // GPU nodes and the like (worker, plugin with a grant)
    pub limits: Limits,             // processes, memory, open files, wall clock (job object / rlimits)
}
```

- **Writable:** the session workspace, `workspace/shared/` (not in `plan`), a private temp directory, and the session's own provider directory inside `<server-data>/agents/<sessionId>/` (§8.5, §8.6).
- **Readable, by allow-list (`Only`)** — revision 2: a deny-list over "everything" always misses a path, so **CLI agents read only**:
  1. the **toolchains**: the CLI's own install tree and runtime, the shell and coreutils on the session `PATH`, `git`, `fin`, and the system libraries, frameworks and fonts those need (per-OS lists, data, produced by the spike of §8.12 and re-validated per CLI version);
  2. **the project folder** (read-only except the write roots) and, read-only, **the roots of the libraries the project references** (§5.6);
  3. **the provider directory** of this session (§8.6) and the session's `credential.json`, `settings/` and `tmp/`.
  Everything else — the rest of the home directory, other projects, other sessions' directories, other apps' data, mounted volumes — is simply not in the policy. `deny_read` still lists server data and the sensitive paths (`~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gcloud`, `~/.kube`, `~/.docker`, `~/.netrc`, `~/.config/gh`, browser profiles, password-manager data, the user's own `~/.claude` and `~/.codex`) as a **second layer** that must also hold if a future toolchain entry were widened.
- **Never writable, never linkable:** `.finisher/`, `assets/`, `finisher.project.json`, the session's own settings and credential files.

### 8.2 macOS: Seatbelt

- `/usr/bin/sandbox-exec -p <profile>` with parameters (`-D`), as S8 did and as Codex and Chromium do. The profile is generated from the policy: **`(deny default)`**, then explicit allows: the read roots of §8.1, the write roots, `(deny file-write* file-link* (subpath protected))`, `(deny file-read* (subpath denied))` followed by the allow of the session directory (last match wins).
- **`mach-lookup` is denied by default, with an explicit allow-list** (`SandboxPolicy.mach`, data per profile). The earlier draft left it open "for the Keychain"; that left an agent able to ask the system's services to act for it. With it closed, a sandboxed process cannot ask `launchd`, LaunchServices (`open`, which would start Terminal or any app *outside* the sandbox), the Apple Events service (`osascript`), the Security server (keychain items of other apps) or `tccd` to do anything. The allow-list holds only what the supported CLIs need to run (DNS and network configuration, the trust evaluation for TLS, logging), produced in the spike (§8.12) by running each CLI under a log-only profile and reviewing the entries one by one. As defence in depth, `process-exec` of `open`, `osascript`, `launchctl`, `security` and the setuid helpers is denied too; the mach denial, not the exec denial, is the guarantee.
- **Network:** outbound IP is open for the `agent` profile except **loopback, which is denied** (`(deny network-outbound (remote ip "localhost:*"))` after the general allow), with a single exception for Finisher's own listener where one is used. `fin` reaches `finisherd` through the **unix socket `<server-data>/run/fin.sock`** by `path-literal` (sp5 §4.11), plus `file-read-metadata` on the path's ancestors. Local dev servers, databases and other apps' loopback services are therefore unreachable.
- **Keychain:** with API-key sessions and per-session provider directories (§8.6) the CLI needs no keychain at all, so there is **no keychain access in the default profile**. A subscription login on macOS (Owner question 6) would need one, and is not built until that is answered: the entry would be a Finisher-owned item whose access list names Finisher only.
- `sandbox-exec` is deprecated but present on macOS 14–26; its removal is a tracked risk (§22).

### 8.3 Linux: Landlock + seccomp

- **Landlock** (`landlock` crate) for files, as S8 confirmed (90/90, ABI 4): write roots enumerated around protected roots (Landlock only adds access); read granted on the enumerated roots of §8.1 (so server data and the sensitive home paths are never under a granted tree); `REFER` only within write roots; `TRUNCATE` handled (ABI ≥ 3). Where ABI ≥ 4, TCP `connect` is limited to the listener's port and ports 80 and 443 (the agent's model API and downloads); other loopback services on other ports are unreachable.
- **seccomp** (`seccompiler`) for what Landlock cannot see. The filter:
  - **checks the architecture first.** It accepts only the native ABI: on x86-64 it refuses 32-bit-compat calls and **the x32 ABI** (syscall numbers with the x32 bit set are a second number space that a filter keyed on native numbers would not match), and on aarch64 the 32-bit compat ABI. A call from any other ABI kills the process (`SECCOMP_RET_KILL_PROCESS`, which the launcher reports, §8.8);
  - **`socket()` policy:** `AF_UNIX` is refused (`EACCES`) for the `agent` and `hook` profiles. Landlock does not mediate `connect()` on pathname sockets (sp5 §14 item 2), and **Landlock's scoping (ABI 6) covers abstract sockets and signals, not pathname sockets, so the D-Bus session bus, `ssh-agent`, `gpg-agent`, `/var/run/docker.sock` and X11 sockets, all pathname sockets, are closed by this refusal and by nothing else.** Only `AF_INET`/`AF_INET6` are allowed (`agent`); `AF_NETLINK` is refused unless validation shows a CLI needs `NETLINK_ROUTE` (then that protocol only); `AF_PACKET`, `AF_VSOCK`, `AF_ALG`, `AF_BLUETOOTH` and the rest are refused. `socketpair()` stays allowed (libuv stdio);
  - **`io_uring` is refused entirely** (`io_uring_setup`, `io_uring_enter`, `io_uring_register`). Its operations include creating sockets and opening files without going through the filtered `socket()`/`connect()` entry points, so allowing it would bypass the socket policy above. libuv falls back to its thread pool when io_uring is unavailable (the per-CLI validation confirms it);
  - also refused: `ptrace`, `process_vm_*`, `pidfd_getfd` (taking descriptors from other processes of the same user, including `finisherd`), `keyctl`/`add_key`, `bpf`, `perf_event_open`, `userfaultfd`, mount, namespace and `setns` calls, `personality`, `kexec_*`.
- **The agent listener (Linux only):** because `AF_UNIX` is closed, `fin` inside the sandbox reaches `finisherd` over a **loopback TCP listener** that accepts only `agent` and `mcp` credentials. It is just another transport for sp5's handshake (§1.7): the canonical endpoint `tcp:127.0.0.1:<port>` is inside both MACs. `FIN_SERVER=tcp:127.0.0.1:<port>`; `fin`'s endpoint parser gains the `tcp:` form.
- **Floor:** Landlock ABI ≥ 3 (kernel ≥ 6.2) for CLI agents and hook commands. Below it, or without Landlock, see §8.9.
- **Per-CLI validation list (7C, run for every supported CLI version on the Linux VM).** The `AF_UNIX` refusal and the narrowed environment can break tools that quietly depend on a daemon. The validation records, and fixes in the launcher's environment or the session's git configuration, each of: **SSSD/LDAP user lookup** (NSS reaches SSSD over a unix socket, so `getpwuid` for a directory user can fail; the launcher exports `HOME`, `USER`, `LOGNAME`, `SHELL` explicitly and the suite checks the CLI and `git` still run); **gpg-agent signing** (`git commit -S` needs the agent socket; the session's git configuration sets `commit.gpgsign=false` and a private `GNUPGHOME`, and the user's `~/.gnupg` is denied regardless); **git credential-cache** (a daemon on a unix socket) and **fsmonitor** (a daemon, and a hook program named in repository configuration, which in a shared project is untrusted input): the session sets `GIT_CONFIG_GLOBAL` to a Finisher file, `GIT_CONFIG_NOSYSTEM=1`, `core.fsmonitor=false`, an empty `credential.helper`; plus the CLI's own telemetry, update and keyring paths.
- **Docker:** Docker's default seccomp profile allows the Landlock syscalls (since 2022), and a stricter filter can be installed under `no_new_privs`; the image's validation row checks both on the reference NAS.
- **bubblewrap is not used:** it is LGPL-2.0-or-later and cannot ship (AGENTS.md), and it needs unprivileged user namespaces, which Ubuntu 24.04's AppArmor restricts (arch §27.8). This replaces the "bwrap fallback" of arch §8.4.

### 8.4 Windows: AppContainer

As S8 confirmed (arch §27.6): a normal AppContainer (not LPAC), **one container profile per project** (`finisher-<project-uuid>`), allow-only ACLs granted through **one ACL broker in `fin-node`** with a write-ahead log and a startup sweep, protected roots outside write roots (or non-inheriting, with delete-child removed from the parent), a private TEMP and home, a private window station and desktop, a kill-on-close job object with process and memory limits, standard handles only, copies (never moves) into `assets/`.
- **Sessions of one project share the container SID**, so an ACL granted to that SID would let session A touch session B's directory. **Each session therefore gets a per-session capability SID** (derived from the session id), passed in the process's security capabilities, and the ACLs on `<server-data>/agents/<sessionId>/` and the session's workspace grant that capability SID, not the container SID. **To verify** (spike, §8.12): that ACEs for derived capability SIDs are honoured for a normal (non-LPAC) AppContainer, the cost of the broker's grant and revoke per session, and that a process of session B cannot open session A's directory.
- **`fin` → `finisherd`: 5A's native named-pipe listener in `fin-node` is reused** (sp5 §4.3: `PipeServer`, `FILE_FLAG_FIRST_PIPE_INSTANCE`, explicit DACL, `PIPE_REJECT_REMOTE_CLIENTS`, peer SID check). 7C only adds the per-project container SIDs through `PipeServer.allow(sid)`; no second pipe implementation. Loopback TCP is blocked for AppContainers.
- **Shell:** brokered (§4.5).
- **Credential Manager** is denied inside the container (S8 follow-up), which protects Finisher's secrets there.

### 8.5 The session environment and directories

`<server-data>/agents/<sessionId>/`:
- `credential.json` (0600; read-only to the sandbox), `settings/` (provider settings Finisher writes; read-only), `tmp/` (`TMPDIR`/`TEMP`), `provider/` (the session's own CLI config directory, §8.6; writable), `git/` (the session's git configuration), `blobs/` (product state, §3.3; denied to the sandbox).

The environment is `childEnv()` plus exactly: `FIN_SERVER` (the unix socket, the agent listener or the pipe), `FIN_SESSION` (the session id; informational only), `FIN_CREDENTIAL` (the credential path), `FIN_CALL` (only for processes Finisher starts per tool call: the built-in `fin` tool, `fin mcp` requests, the brokered shell), `TMPDIR`, and the provider's directory variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, both pointing into the session directory). `FIN_TRAINING`, `FIN_DIAGNOSTICS`, **`FIN_SERVER_DATA`**, `FIN_AUTHOR`, `SSH_AUTH_SOCK`, `GPG_AGENT_INFO`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`, `WAYLAND_DISPLAY` and every `*_API_KEY` never pass (arch §27.7, D-plan Decision 14b).

**`fin` under an agent credential never reaches the local key.** Today `fin` finds server data from `FIN_SERVER_DATA` or the per-OS default and falls back to `local.key` when no credential is found. Three rules close that:
1. **When `FIN_CREDENTIAL` is set, `fin` never falls back to the local key.** A missing, unreadable, expired or revoked credential file is `UNAUTHENTICATED` (exit 7), not a retry as `local`; the server-data default is not even consulted for keys.
2. **The tool clears `FIN_SERVER_DATA`.** The built-in `fin` tool deletes it from the environment it builds (`childEnv()` already omits it), and every CLI adapter's launch environment is checked for it; if it is set anyway, `fin` ignores it while `FIN_CREDENTIAL` is set and takes the endpoint from `FIN_SERVER` alone.
3. **The server decides the author** for agent principals (§5.2), so `FIN_AUTHOR` has no effect.
A test in `fin` covers (1) and (2) (§16.2).

### 8.6 Provider configuration isolation: per session

S8 made `~/.claude` and `~/.codex` writable to the sandbox. That is a **persistence escape**: an injected agent could add a hook or an MCP server to the user's own Claude Code or Codex configuration, which then runs **unsandboxed** the next time the user starts that CLI. Revision 1 fixed that with one Finisher-owned directory per CLI, but a directory *shared by every session* has the same flaw between sessions: one injected session could plant a hook that a later session (possibly with a wider mode) then runs. Therefore:
- **Each session gets its own CLI configuration directory** (`<server-data>/agents/<sessionId>/provider/`, set through `CLAUDE_CONFIG_DIR` and `CODEX_HOME`), created empty at session start, writable by that session only, deleted with the session. Settings files in it are written by Finisher before launch.
- **Sign-in state is copied in and synced back, nothing else.** Finisher keeps the master sign-in (the CLI's auth file, from `finisherd provider login <harness>`, run with the master directory `<server-data>/providers/<harness>/`, which no sandbox can read). At session start `finisherd` copies **only the auth file** into the session directory (0600). While the session runs and at its end, `finisherd` (outside the sandbox) checks whether the session's auth file changed (a token refresh); if so it parses it against the CLI's known auth shape (size-capped, opened without following links), and replaces the master copy atomically **only if it is newer**. No other file is ever copied back, and nothing from the session directory is ever executed or loaded by `finisherd`. A session that tampers with its own provider directory harms only itself.
- **API-key sessions copy nothing**: the key reaches the CLI through `apiKeyHelper` (Claude Code, `--bare`) or the app-server's login call (Codex), §10.
- The user's own `~/.claude` and `~/.codex` are denied for read and write.
- **Windows:** the session directory carries the per-session capability SID of §8.4, so another session of the same project cannot open it (**to verify**).
- **To verify in the spike (§8.12):** whether refresh-token rotation makes parallel sessions' copies of one login invalid (then subscription sessions would have to be serialised, or the sync-back become a lock); whether `CLAUDE_CONFIG_DIR` separates the macOS Keychain entry (revision 1's open point). Both matter only if Owner question 6 permits subscription logins at all; **by default Finisher uses API keys, so neither blocks 7C**.

### 8.7 Network

Agents need egress: their own model API, downloads of media and fonts. In v1 egress is **open** for the `agent` profile and **closed** for `hook`, `worker` and `plugin`. What the sandbox bounds is what an injected agent can *read* before sending: secrets, server data and other projects are out of reach (§8.1), **other local services are out of reach** (macOS: loopback denied except Finisher's listener; Linux ABI ≥ 4: TCP ports limited; Windows: no loopback for AppContainers), but **the project's own media and the agent's workspace are readable and can be sent anywhere, and so can whatever the CLI itself must hold to work: for an API-key session the key (it exists in the CLI's memory), for a subscription session its sign-in token.** Owner question 5 states this residual risk in those terms. An egress allow-list is the later fix.

### 8.8 Violations

- A denied file or socket operation is an `EACCES`/`EPERM`/`ACCESS_DENIED` to the process; the CLI sees a failed tool call. **Finisher does not read the macOS unified log** (cut): it is slow, racy and a privacy surface of its own, and the escape suites (§8.11), not log scraping, are the evidence that denials hold.
- `finisherd` writes an audit entry `sandbox.violation {session, op, profile, root kind}` (sp5 reserved kind) and a `notice` node only for events the launcher or the host observes directly: a seccomp `KILL_PROCESS` (an out-of-policy ABI or syscall, seen as `SIGSYS`), a job-object limit notification (memory, process count), the ACL broker refusing a grant, the launcher failing to apply a policy (`SANDBOX_UNAVAILABLE`), and the server-side denials that already exist (`PATH_NOT_ALLOWED`, `BRANCH_NOT_ALLOWED`, `auth.denied`). Paths are stored as root kind and keyed hash only.
- Tampering that slips past is still caught: assets are content-addressed and verified on use; `.finisher/` segments are hash-chained (arch §8.4, sub-project 4 §3).

### 8.9 Hosts without a usable sandbox

- **The built-in agent always runs:** its tools are `fin` (a credentialed RPC client) and confined workspace file tools executed by `finisherd`; it has no shell. It needs no OS sandbox.
- **CLI agents and hook commands require the sandbox.** Without it (Landlock ABI < 3, no Landlock, `sandbox-exec` missing, AppContainer creation failing), starting one is `SANDBOX_UNAVAILABLE {profile}` with a `fix` naming the kernel or OS requirement. **There is no uid-sandbox fallback in Docker** (cut): a second isolation mechanism with its own gaps for one deployment is not worth carrying; the Docker image runs on a kernel with Landlock or runs the built-in agent.
- An owner-only override `agents.unsandboxed: allow` (audited, shown on every session) runs CLI agents with the env allow-list and scopes only (Owner question 3).

### 8.10 Profiles: `agent`, `hook`, `worker`, `plugin`

`fin-sandbox` (sub-project 7 owns it and the ACL broker; §1.6) exposes one launcher and four profiles over the one `SandboxPolicy`. 5C calls it through the `Spawner` trait at every process start (sp5 §6.2), so the same launcher confines agents, hooks, workers and plugins.

| | `agent` | `hook` | `worker` | `plugin` |
|---|---|---|---|---|
| Reads | §8.1 allow-list | project read-only, the hook's own file, its toolchain | library roots and project `sources/` (per-file where the OS allows), font cache, media cache (read), the job's `in/` | the plugin's store directory, the runtime (finisherd's Node, or the Electron framework, for `node` plugins), system libraries and frameworks (`/usr/lib`, `/lib`, `/System/Library`, `%SystemRoot%\System32` and the like), ONNX Runtime's library, the model files the manifest names, the call's `in/` |
| Writes | session workspace, `workspace/shared`, temp, provider dir | a private temp | the job's `out/` and a private temp | the call's `out/`, the plugin's per-project data directory (with `fs pluginData readWrite`), a private temp |
| Network | open IP; loopback limited (§8.7) | none, unless declared (then the listener only) | **none** | **none**: Linux seccomp refuses `AF_INET`, `AF_INET6`, `AF_NETLINK` and `AF_UNIX` (`socketpair` stays for stdio); macOS `(deny network*)`; Windows AppContainer with **no** `internetClient`, `internetClientServer` or `privateNetworkClientServer` capability and no loopback exemption |
| Processes | limited tree | limited | **creation denied** (macOS `process-exec`/`process-fork`; Linux exec limited to the worker binary by Landlock `EXECUTE` and `clone` without `CLONE_THREAD` refused; Windows active-process limit 1); helpers are siblings 5C starts | limited to 8 (memory 2 GiB default, up to the manifest's declared need ≤ 16 GiB, 1,024 open files) |
| Devices | none | none | GPU: macOS Metal and the IOKit user clients it opens (`IOSurfaceRootUserClient`, the accelerator client), IOSurface, and the CoreMedia/VideoToolbox services hardware decode and encode use; Linux `/dev/dri/renderD*` read/write, `/dev/dri/card*` read-only for enumeration where Mesa needs it, the Vulkan ICD and driver files, `/sys/class/drm` and `/sys/dev/char` reads, `/dev/nvidia*` in 6G; Windows D3D11/D3D12 and Media Foundation in a normal AppContainer (to be proven in 6B's validation) | **only with the `gpu` grant**: Linux `/dev/dri/renderD*` (never `card*`) and `/dev/nvidia*` when CUDA is used; macOS only the IOKit user clients Metal needs (the `IOGPU`/`AGXDevice` classes) under `iokit-open`; Windows through the container's graphics access |
| macOS services | the explicit `mach-lookup` allow-list of §8.2 | as `agent`, smaller | the Metal/VideoToolbox services | **denies `mach-lookup` of `com.apple.tccd`, `com.apple.tccd.system` and `com.apple.coreservices.appleevents`, denies `appleevent-send`, and grants no read root inside TCC-protected folders**, so requests fail closed with no prompt (sp8 §3.2) |
| Windows grants | per-project container, per-session capability SID (§8.4) | per call | **per-job ACL grants** through the broker | **one container profile per (plugin, project); ACL grants on the call or job directory added before the call and removed after it, write-ahead logged like the session grants; `media:file` grants on a single source file per job** |
| Never | server data other than the session dir, the credentials of others, `auth/` | credentials unless `fin` is declared | server data, credentials, the agent listener | server data, another plugin's data, another project's plugin data |

- The `worker` and `plugin` columns are the contract sub-projects 6 (§3.5) and 8 (§16.1) wrote; where this table and theirs differ, theirs states the *requirement* and this one the *implementation*, and a difference is a bug in whichever is later.
- **Cross-OS honesty:** the macOS and Windows GPU rows are unproven (S8 tested no GPU work); 6B's validation and the plugin escape suite prove them. Until the `worker` profile passes its suite on an OS, 5C spawns workers unsandboxed with the env allow-list and `server.status` says so (sp5 §6.2).
- `SANDBOX_UNAVAILABLE {profile: plugin}` is what sp8 §16.1 uses to refuse registry-installed code plugins on an OS where the profile cannot be applied.

### 8.11 Escape suites

The S8 suites become tests (arch §22). Each case is "attempt the operation, observe a denial"; a case that does not observe a denial fails, and none is accepted on the absence of an error. They run in CI on all three OSes, in each profile it applies to, and in the validation tasks as a non-elevated user.
- **S8 core:** the 90 file escapes (protected roots, links, renames, truncation, read denials); the Windows-specific paths (junctions, ADS, 8.3 names, `\\?\`, admin shares, WMI, `schtasks`).
- **Server data:** reading `local.key`, `console.key`, `credential.master`, `settings.json`, `control.db`, the journal and the agent-trace store; writing the user's `~/.claude/settings.json` and `~/.codex/config.toml`; truncating a segment; hard-linking out of `assets/`.
- **Between sessions:** a session cannot read or write another session's directory, credential, provider directory or temp (Linux and macOS by roots; Windows by the capability SID, **to verify**); a session's tampered provider directory is not copied back anywhere (§8.6).
- **Linux:** connecting to a D-Bus session bus, an `ssh-agent` socket, a `gpg-agent` socket, a fake `docker.sock` and an X11 socket (each a pathname socket) is refused; creating a socket **through io_uring** is refused (the io_uring entry points themselves are refused); a call made through the **x32 or a 32-bit compat entry** is refused or kills the process; `pidfd_getfd` of a host process, `ptrace`, `bpf` are refused; with Landlock ABI ≥ 4, a connect to a loopback port outside the allowed set is refused.
- **macOS:** with `mach-lookup` default-deny, **the agent cannot start a process outside the sandbox** — `open` of an application or of Terminal, `osascript`, `launchctl` each fail, and the suite checks the process tree to prove nothing new ran outside the sandbox — and **cannot read another application's keychain item** (the suite creates a test item whose access list excludes the sandboxed process and attempts to read it with the system tools; and `~/Library/Keychains` is unreadable); loopback ports other than Finisher's listener are refused; TCC-protected folders and Apple Events fail closed.
- **Windows:** the pipe cannot be opened by another user's process or another project's container; Credential Manager is denied; the shell broker refuses a command outside its container.
- **`worker` and `plugin` profiles:** the plugin suite of sp8 §16.1 (TCP, UDP, raw, netlink and unix-socket connects; reading server data, another plugin's data, another project's plugin data, the project folder, `~/.ssh` and Documents; writing to `in/`; replacing an `out/` file by a symlink, junction or hard link to a host file; truncating a ring; GPU device nodes without `gpu`; `ptrace` of the host; TCC access to Documents, camera and microphone; an Apple Event to Finder; spawning past the process limit) and the worker suite (network, process creation, reading outside the manifest's files, writing outside `out/`).
- **The ACL broker:** crash and concurrency tests (WAL replay, the S8 lost-grant race), per-job grant add/remove under a crash between the two.

### 8.12 Spike S9 (in `archive/`; can run now, before any 7C code)

A controller task that resolves the open facts of this section on real hosts, with notes and scripts under `archive/` (git-ignored, AGENTS.md); only durable conclusions come back into this section when 7C, 7D and 7E land "as built". **Checks:**
- **B5 (Linux, kernels 6.2 and newer, the Linux VM and one older-LTS NAS if available):** the seccomp filter refuses io_uring and the x32/compat ABIs as described; libuv and the CLIs fall back cleanly; the per-CLI validation list of §8.3 (SSSD/LDAP lookup, gpg-agent signing, git credential-cache and fsmonitor) against Codex 0.167.0 and Claude Code 2.1.319; whether any newer Landlock ABI mediates pathname sockets.
- **B6 (macOS 14 and newer):** the minimal `mach-lookup` allow-list per CLI; that `open`, `osascript`, `launchctl` and keychain reads fail under it.
- **B7:** a per-session `CLAUDE_CONFIG_DIR` and `CODEX_HOME` with the auth file copied in and synced back; refresh-token rotation under parallel sessions; whether `CLAUDE_CONFIG_DIR` separates the macOS Keychain entry; on Windows, per-session capability SIDs on a session directory under a normal AppContainer (**to verify**).
- **H2:** Claude Code `--bare` with `apiKeyHelper` under `-p` stream-JSON; that `--append-system-prompt` is honoured under `--bare`; whether `system/init` is emitted before the first prompt and what its tool list holds; `--max-budget-usd` behaviour; that a tool missing from the allow-set and listed in `--disallowedTools` is refused; Codex `-c project_doc_max_bytes=0` and `developer_instructions` re-checked at the pinned version (both verified on 0.167.0).

---

## 9. Hooks (7F)

### 9.1 Events and replies

Arch §10's table holds **except `PreOp`/`PostOp`, which are cut from v1** (they sit on the commit path, and their only reason to exist beyond `PreCommand`/`PostCommand` is the `modify` reply, also cut). Sub-project 8's `hooks:modify` grant is therefore not implemented in v1 (§1.6). Wiring:

| Event | Fired by | May |
|---|---|---|
| `SessionStart`, `SessionEnd` | the runtime | add context; log |
| `UserPrompt` | `agent.session.prompt`, saved prompts | add context; block |
| `PreCommand`, `PostCommand` | the authorisation wrapper, for every command from an `agent` principal (and for every principal when a hook asks for `all`) | block with reason; add context |
| `BranchReady` | `review.request` from a session, or a session ending with commits | extra QA; gate the critic |
| `PreMerge`, `PostMerge` | `history.merge`, `review.merge` | enforce policy; notify |
| `PreRender`, `PostRender`, `JobDone` | the jobs bus (5C) | validate; publish; notify |
| `Approval`, `Notification` | approvals, session status changes | route to a phone or a chat service |

- **Replies:** `allow | block {reason} | context {text}`. **`modify` replies are cut from v1**: a hook that rewrote a payload would make what the agent asked and what was committed differ in ways the training record, the audit log and the agent could not see. A later release can add them with per-hook human-only grants and both payloads in the audit log.
- **Cost:** a hook point with no registered hook costs one map lookup. Handlers have 10 s by default, 60 s at most. A timeout is a visible `HOOK_TIMEOUT` notice; whether it blocks is per hook (`onTimeout: allow|block`, default `allow` except `PreMerge`).
- **Order:** managed, user, project, local, plugin; within a layer, file order. The first `block` stops the chain; `context` replies concatenate.

### 9.2 Handlers

- **Commands:** JSON on stdin (`{event, v, session?, project, payload}`), one JSON reply on stdout; run through `fin-sandbox` in the **`hook` profile** (§8.10) with the cleared environment, `net: None` unless declared, the project read-only, a private temp directory, the timeout, and no credential unless the hook declares `fin` (then a `plan`-scoped agent credential of its own).
- **Plugin RPC methods:** sub-project 8, same reply shape and timeouts.

### 9.3 Configuration

`hooks` in `managed` settings, `<server-data>/settings.json` (user), `finisher.hooks.json` beside `finisher.project.json` in the project folder (project: it travels with the folder, so it is trust-gated, §9.4), and `<server-data>/projects/<id>/hooks.json` (local).

### 9.4 Project trust

- Project hooks and project command rules stay **disabled until the owner trusts the project** (arch §10): `control.db` `project_trust {projectId, hooksHash, trustedAt, by}`. A change to the project's hooks (hash) disables them again until re-trusted. Trusting is human-only (`finisherd project trust <id>`, later the UI).
- Hook blocks are audited (`hook.block`, sp5 reserved; `hook.modify` stays reserved and unused).

---

## 10. Secrets and `ModelGateway`

- **`SecretStore`** (TS interface; backends: the OS keychain through 5B's `keyring` binding, and a **file `<server-data>/secrets.json`, mode 0600**, for headless servers, arch §16). 7A-1 ships the file backend behind a **`SecretBox` seam** (`seal(plaintext) → Sealed`, `open(Sealed) → Buffer`); the file backend's box is the identity and the 0600 mode of the private directory is the protection. **The encrypted file (XChaCha20-Poly1305 with a key file beside it) is cut**: a key stored in the same directory under the same user adds nothing against same-user code, and the sandbox already denies the directory to agents (§8.1); it would only add a dependency, a rotation story and the look of protection. The seam lets the keychain backend (5B) seal for real later. Entries: `{provider, label, kind: apiKey|baseUrl+apiKey, createdAt}`; values are never returned by any RPC.
- **`ModelGateway.grant({provider, sessionId, purpose})`** returns a short-lived in-memory handle used by the built-in adapter for one request; the key is read at call time and never leaves `finisherd` (arch §16). Every grant is counted per session; the first grant of a session and every denied grant are audited (`grant`, sp5 reserved).
- **API keys are the default for CLI agents; a subscription login is a separate decision** (Owner question 6). When a CLI uses a key kept in Finisher:
  - **Codex:** the key goes over the app-server's login call on stdin.
  - **Claude Code:** `--bare` (§4.5) with `apiKeyHelper` in the session settings, which runs `fin credential model-key anthropic` and obtains a grant over the session's own transport. The key then exists inside that sandbox (the CLI must hold it), which is stated in the UI and in Owner question 5; it is never in the environment, a file or the journal.
- Keys entered and listed only by the owner (`agent.admin`; `finisherd provider key set|ls|rm`).

---

## 11. Workspace artifacts and the full `fin` (7G)

### 11.1 Artifacts

- `fin asset add workspace/<session>/logo.svg [--source-url <u> --licence <spdx|unknown>]` hashes the file, validates it (SVG sanitised and rendered by resvg, Lottie templates through `templateCheck`, media probed and proxied: sp6 §3.4, §11), copies it into `assets/` (write-once, copied on Windows, arch §27.6) and records `asset.add` with **provenance** `{author: session, role, tool: callId, prompt node, sourceUrl?, licence}`. `fin asset update <asset> <file>` adds a version.
- **Limits:** 2 GiB per media file, 16 MiB per SVG or Lottie, types from `fin-probe`'s allow-list; `ASSET_TOO_LARGE`, `ASSET_TYPE_UNSUPPORTED`.
- **Drafts bin:** `finisherd` lists unregistered files in `workspace/` live (polling on network roots, arch §8.5) as a `workspace.watch` stream for the UI.
- **`fin workspace ls [--session]`**, **`fin workspace gc`** (asks; deletes only after a person approves, arch §8.5).
- Unknown-licence assets are flagged for QA (sub-project 10) and by the default `ask` rule.

### 11.2 Commands completed in 7G

- **`fin apply <spec.json|->`** (arch §9.5): a declarative edit list (`{edits: [{cmd, args}], expect?: {rev}}`), validated in full offline by the linked `fin-core`, committed as one transaction; paper edits from quotes (`{cmd: "paper", quotes: ["…"], from: asset}`), resolved against transcripts (sub-project 8) to word ranges and echoed.
- **`fin batch`** accepts every write command (today four), each line one transaction. **`fin tx begin|commit|abort` is cut**: a server-side open transaction held across tool calls needs expiry, recovery after a crash and a story for what a half-open transaction means to rollback and the training record, and `fin apply` already commits a validated edit list atomically in one call.
- **`fin context`:** project, branch, session, mode, role, budget left, open approvals, children, recent notes, in ≤ 800 tokens.
- **`fin guide <task>`:** the role's skills by topic, versioned with the server.
- **`fin note <text>`:** a note node in the session and a workspace file; content, not a marker.
- **`fin budget`:** remaining tokens, money, render minutes, wall clock.
- **`fin mcp`** (§4.3), **`fin agent …`** (7B), **`fin approval wait`** (7B), **`fin credential model-key`** (§10).
- **The map stays under 1.5k tokens** (its test); new groups appear in the map by name only.

---

## 12. Diagnostics and training capture

### 12.1 Three stores and what each is for

- **Product state** (§3.3, `control.db`): every node, for resume, display and audit by the owner. Not gated by consent, never uploaded.
- **The diagnostics journal** (D-plan Decisions 15–20; 128 MiB / 30 days): the *metadata* of agent events (counts, durations, hashes, codes), written under diagnostics consent like every other kind.
- **The agent-trace store** (new in revision 2): the *content* of agent turns, tool calls and results, written **only under the training consent** of D34. Revision 1 put content in the diagnostics journal as blobs; one long session (tool outputs, images, whole requests re-sent every turn) would have evicted every other diagnostic in a few hours. The trace store is **a second `Journal` instance of the same code in its own directory (`<server-data>/agent-traces/`) with its own cap (default 2 GiB, a setting) and its own retention (default 90 days)**; eviction removes whole sessions, oldest first, never half a conversation. Purge and export of diagnostics address both stores' *entries*; neither touches product state.
- **What the trace store holds is deltas plus a hash.** An agent request repeats everything before it. Each `agent.turn` content record stores only the **new** messages since the previous turn of the run (`messagesDelta`), the response, and **`bodyHash`, the BLAKE3 of the full request body exactly as sent**, so the dataset builder can rebuild the full request by replaying deltas and check the result against the hash (a mismatch drops the example). `system` and `tools` are blobs stored once per distinct content (§12.2). Budget facts are not in the system prompt (§4.1), which is what lets one system blob serve every session of a role.
- **Conversations are not touched by purge or export of recorded data**: `diagnostics purge` and `diagnostics export` never read or delete `control.db` nodes, and **the dataset builder never reads `control.db`** (a test asserts it, §16.2). Sub-project 9's "what we collect" page must say so in plain words: *conversations stay on your server for resume until you delete the session or the retention period ends; deleting recorded data does not delete them.*
- The journal applies D34 to both stores: metadata under diagnostics; content (prompts, outputs, tool arguments and results) only under training, which only a person can turn on; nothing for a project with `collect: never` or unknown. `agent.turn`, `agent.toolCall` and `agent.toolResult` stay training-only kinds (their metadata part is written under diagnostics).

### 12.2 Kinds (version 1, replacing the v0 reservations)

| Kind | Meta | Content (training only) |
|---|---|---|
| `agent.session` | `sessionId`, `phase: started|resumed|interrupted|ended`, `harness: builtin|claudeCode|codex|fake|plugin`, `provider` (`AGENT_PROVIDERS`), `model` (`AGENT_MODELS`), `modelHash?`, `role` (`AGENT_ROLES`), `mode: plan|draft|auto`, `parent?`, `trigger: user|savedPrompt|subagent|hook|eval`, `reason?` (code), totals at `ended` (turns, tool calls, txs, tokens, `costMicros`, `wallS`) | — |
| `agent.turn` | `turnId`, `runId`, `seq`, `parentTurnId?`, provider, model, input/output/cache-read/cache-write/reasoning tokens, `latencyMs`, `ttftMs`, `stopReason`, `costMicros?`, `costBasis: reported|priced|subscription|unknown`, `capture: exact|stream`, `systemHash`, `toolsHash`, **`bodyHash`** (BLAKE3 of the full request body as sent; exact capture only) | `messagesDelta` (the turn's new input messages), `output`, `system`, `tools` — each a `Body`, a blob when over ~4 KiB; `system` and `tools` are always blobs, so identical prompts are stored once by content address. All in the trace store (§12.1) |
| `agent.toolCall` | `callId` (the `FIN_CALL` ULID), `turnId`, `tool` (`AGENT_TOOLS`: `fin`, `shell`, `read`, `write`, `edit`, `search`, `webFetch`, `webSearch`, `mcp`, `workspace`, `other`), `command?` (`CLI_COMMANDS`, when the call runs `fin`), `argsHash` | `args` |
| `agent.toolResult` | `callId`, `ok`, `code?`, `exit?`, `micros`, `txIds`, `attribution: exact|inferred|ambiguous|none` (§12.3), `truncated` | `output` |
| `agent.approval` (new) | `approvalId`, `sessionId`, `kind: rule|budget|provider|plugin`, `decision: allow|deny|expired`, `by: user|policy`, `latencyMs` | `subject` |
| ~~`agent.editVerdict`~~ | **retired** (revision 2): verdicts are 5E's **`review.verdict`**, the single source (§12.5) | — |

- These change the reserved v0 payloads in place (allowed for v0, D-plan Decision 3) and set version 1 when 7A-2's emitters land. `agent.approval` is a new kind; the reserved v0 `agent.editVerdict` is removed (it was never emitted).
- **New tables** in `fin_core::events::names`: `AGENT_ROLES`, `AGENT_TOOLS`; `AGENT_MODELS` grows to the catalogue (§4.7). `harness` is a closed enum.

### 12.3 `FIN_CALL` and attribution

- **Exact:** the call id is set as `FIN_CALL` for every process Finisher starts per tool call (the built-in `fin` tool, `fin mcp` requests, the brokered shell). `fin` sends it in its actor header (D6); 5A lets an `agent` principal set only `call`; every `log.commit` and `cli.command` of that run carries it, and `agent.toolResult.txIds` lists the commits. `attribution: exact`.
- **Inferred (CLI agents' own shells), with a defined window and key.** Neither Claude Code nor Codex lets a host set environment per tool call, so the runtime matches by what it observed:
  1. **The window** of a shell `toolCall` opens at the runtime's **allow decision** for it (`can_use_tool` answered allow, or the approval granted) and closes at its **tool result**.
  2. **The match key is the argv hash.** From the allowed command string the runtime derives, for each `fin` invocation it can see statically, the hash `fin` itself sends with its requests (`argsHash`, the hash of its resolved arguments, D-plan Decision 23a). A request from the session's principal whose `argsHash` equals one derived from an open call, **inside that call's window**, belongs to that call: `attribution: inferred`.
  3. **Several open calls with the same hash, or no derivable hash** (the command is built by shell expansion), **→ `ambiguous`** on each candidate result, the txIds listed on all of them, no call on the commit.
  4. **Anything outside a window gets `none`**: in particular **processes a shell tool left running in the background** (`&`, `nohup`, a detached script) whose `fin` calls arrive after the tool result. Their commits are still the session's (`session_txs`, §5.2) with `attribution: none` and no call id.
- **Rejected:** rewriting the agent's shell command through the permission callback to prefix `FIN_CALL=…` (it changes what the model wrote, shows in its transcript, and corrupts the training record).
- `fin mcp` makes CLI attribution exact; the eval harness weighs that (§4.3). The dataset builder can drop every non-exact join.

### 12.4 Large content

- `Body::Blob` through `Journal::attach_blob` (D-plan Decision 20), into the **trace store** (§12.1), for anything over ~4 KiB; the 4 MiB blob cap holds, larger outputs are truncated with `truncated: true` (the product node keeps the full output).
- Images given to or produced by a model are blobs (`image/png`, `image/webp`); frames from `fin look` reach the journal only through these tool results.

### 12.5 Edit verdicts: 5E's `review.verdict` is the single source

- Sub-project 5E defines and emits **`review.verdict`** (sp5 §3.4): per-transaction `accepted`, `rejected`, `merged`, `dismissed`, `changesRequested`, with `txIds`, `by: user|agent|critic` and training-only `reason`. **Sub-project 7 defines no second verdict kind**; the reserved `agent.editVerdict` is retired (§12.2).
- What a review item does not cover still has to be recorded, or the dataset loses its strongest negative signals (undone, reverted, abandoned) and every plain `fin merge`. **7's outcome tracker therefore emits `review.verdict` entries too**, for outcomes outside a review item, using the extensions 7 asks of 5E (§1.7 item 5): `itemKind: none`, `verdict: undone|reverted|abandoned|merged`, `via: undo|revert|archive|merge|critic`, `sessionId`. One schema, one reader.
- **The tracker** (`verdicts.ts`) observes the history RPC results inside `finisherd` and branch status changes, and joins them to `session_txs` (§5.2):
  - `merged` when a merge copies an agent transaction into a branch outside the session's subtree and **no review item produced it** (a review's merge is already a 5E verdict); `by` from the merging principal;
  - `undone` / `reverted` when an undo or revert names it (`by`: a person, the agent itself, or a critic session);
  - `abandoned` when the session's root branch is archived, or the session is deleted, with unmerged transactions;
  - `by: critic`, `via: critic` when a critic's child-branch fix undoes or replaces them (10).
- **Until 5E's kind exists** (7A-2 lands before 5E), the tracker records each outcome in `session_txs.outcome` (product state) and emits nothing; it emits once the kind is registered (a lookup, not a flag). One verdict per `(txId, via)`; a later verdict does not erase an earlier one (the builder reads the sequence).

### 12.6 Capture fidelity per provider

- **Built-in:** `capture: exact`: the request bodies as sent (system, tools, messages) and the response as received.
- **CLI agents:** `capture: stream`: what the CLI's stream reports. Claude Code reports messages with thinking blocks where it emits them, tool uses and results, and usage; Codex reports items, reasoning summaries and usage. Neither reports its system prompt or tool definitions, so `system`/`tools` are absent and `systemHash` is the hash of what Finisher added (role prompt, settings). A recording proxy for exact CLI traffic is Owner question 2.

### 12.7 The dataset builder's join (kept possible, not built)

Example = state at `log.commit.baseRev` + instruction (the run's `userMessage` and the turns before the call, joined by `turnId` → `callId` → `ctx.actor.call` and `txIds`) + intent (`log.commit.intent`); label = the `review.verdict` sequence for those txIds (D-plan Decision 34's undo/merge/abandon signals live in the same kind, §12.5). Provenance fields (`harness`, `provider`, `model`, `capture`) let the builder choose which outputs may be training **targets** (Owner question 1: only human-authored edits and open-weight model outputs; closed-model sessions contribute their context and verdict statistics, never their outputs as targets).

---

## 13. Cost and budgets

### 13.1 Budgets

- **Kinds:** input+output tokens (cache reads weighted by their price ratio), money (`costMicros`), render minutes (5C jobs owned by the session's principal), wall clock, model calls per run, tool calls per run.
- **Scopes:** per session (from the role, overridable at start), carved for children from the parent's remaining budget (a reserve that returns unused on the child's end), and per server per day for money (owner setting; off by default for subscription logins, whose usage is shown in tokens only).
- **Enforcement:** at 80 % a `notice` node and a `Notification` hook; at 100 % the runtime interrupts the current turn at the next safe point (after the running tool call), sets the session `interrupted` with `BUDGET_EXCEEDED`, and opens a `budget` approval to extend. Claude Code also gets `--max-budget-usd` as a backstop; the flag works **only in print mode**, per process, so it cannot be the budget authority (§4.5).
- **Accounting** is per `agent.turn` from usage events; a CLI that does not report cost is priced from the catalogue (`costBasis: priced`); a subscription login is `costBasis: subscription` with notional cost shown but not charged against money budgets unless the user asks.

### 13.2 Ledger

`control.db` `budgets {scope, kind, limit, used, reserved}` updated per turn in one statement; `fin budget` and `agent.session.get` read it.

### 13.3 Rate limits

Per provider and key: concurrent requests (default 4) and a token bucket from the provider's headers where present; 429 backoff for the built-in agent (§4.4); CLI agents handle their own and their waits show as `notice` nodes.

---

## 14. The eval harness (7H)

### 14.1 What it scores

Arch §9.13 and §22: CLI changes and agents' edits. A **task** (`evals/tasks/<id>/task.json`, tracked) is a brief, a starting project (a `fin batch` script over fixtures), the mode and role, and a **scorer spec**. **Fixtures are real speech under permissive licences** (revision 2: synthetic speech cannot exercise transcript-driven editing, which is most of what agents do): candidate corpora are Common Voice (CC0) and LibriSpeech (CC BY 4.0), and permissively licensed open-movie footage with dialogue for video; **no corpus with a non-commercial, no-derivatives or share-alike term is admitted** (AGENTS.md: evaluation data must be permissively licensed too), each fixture's licence is recorded in its manifest and re-checked at each pin, and no media is committed — fixtures resolve through the local fixture cache (`FIN_MEDIA_FIXTURES`, sp6 §13.3). Synthetic media (sp6's generator) remains for tasks about structure, not speech. Results go to `archive/evals/<date>/` (git-ignored); only conclusions reach a spec.

### 14.2 Scorers (`crates/fin-eval`, linking `fin-core`)

- **State checks:** predicates over the final snapshot of the session branch (`item count by kind`, `caption coverage of a range`, `no CAPTION_OVERFLOW`, `duration within ±0.5 s of target`, `a clip trimmed to word w:240 ± 1 frame`), each pass/fail with a reason.
- **Reference comparison** (when the task has a reference edit):
  - **word keep F1:** label each transcript word of the sources kept or dropped by the timeline's coverage of it, compare with the reference (robust to ripple positions);
  - **cut F1:** output-timeline cut points matched within ±2 frames.
  The **structure-distance score is cut** (tracks, item kinds, caption blocks, graphics slots): it rewards copying a reference's layout, not editing well, and two equally good edits differ widely on it.
- **QA** (sub-project 10): the deterministic gate's findings, once it exists.
- **Process:** success (all required checks), model calls, tool calls, `fin` errors by code, retries (a command repeated after an error), undos, tokens, cost, wall time, approvals asked.
- **Judge (optional):** the critic role's rubric score, reported beside the others, never a gate.

### 14.3 Runs and reports

- `pnpm eval --tasks <glob> --harness builtin,claudeCode,codex --model <m>… --n 3` drives sessions through the console credential, scores each final branch, and writes JSONL plus a summary per harness × model × task.
- **Comparing two variants** (two CLI builds, two models, `fin mcp` against shell): **paired by task**, over **at least 20 tasks** with **n ≥ 3 runs per task and variant**. For each task the per-variant pass rate and process metrics are averaged over its runs, the paired difference is taken per task, and the report gives the mean and median difference with a **bootstrap** (10,000 resamples of tasks, percentile 95 % interval). *Better* means: the interval for the pass-rate difference excludes a loss larger than 2 percentage points **and** the interval for calls or tokens per success excludes an increase. Fewer than 20 tasks produce a report marked *descriptive*, with no verdict. The Wilson interval of revision 1 treated runs as independent and ignored that every variant ran the same tasks.
- **In CI:** the harness itself runs against the scripted fake provider (§16), which tests runners and scorers, not models. Real-provider runs are manual, with the owner's keys.

---

## 15. Error codes

| Exit | Codes |
|---|---|
| 2 | `AGENT_ARGS` (bad `fin agent` usage) |
| 3 | `SESSION_NOT_FOUND`, `ROLE_UNKNOWN`, `MODE_INVALID` (above the role's ceiling or the parent's mode), `HARNESS_UNKNOWN`, `MODEL_UNKNOWN`, `SAVED_PROMPT_UNKNOWN`, `APPLY_INVALID {index, error}`, `ASSET_TOO_LARGE`, `ASSET_TYPE_UNSUPPORTED`, `APPROVAL_NOT_FOUND` |
| 4 | `SESSION_BUSY`, `SESSION_ENDED`, `BRANCH_IN_USE`; `CONFLICT` for a failed rollback |
| 5 | `PROVIDER_UNAVAILABLE` (not installed), `PROVIDER_VERSION {floor | missing capability}`, `PROVIDER_AUTH` (not signed in, key missing or refused), `PROVIDER_RATE_LIMITED {retryAfterMs}`, `PROVIDER_OVERLOADED`, `PROVIDER_PROTOCOL` (a message the adapter cannot map: drift), `PROVIDER_EXITED {code}`, `MODEL_CONTEXT_EXCEEDED`, `SANDBOX_UNAVAILABLE {profile, os, need}`, `SESSION_INTERRUPTED` (resumable), `WAIT_TIMEOUT`, `HOOK_TIMEOUT` (when blocking) |
| 7 | `APPROVAL_DENIED`, `RULE_DENIED {rule, layer}`, `BRANCH_NOT_ALLOWED {branch, root}`, `BUDGET_EXCEEDED {kind, used, limit}`, `HOOK_BLOCKED {hook, reason}`, `MODE_FORBIDS {mode, command}`, `DEPTH_EXCEEDED` |
| 6 | `INTERNAL` (an adapter bug, as elsewhere) |
| **8 (new: pending)** | `APPROVAL_PENDING {approval}` (from the `ask` stage and from `fin approval wait` on timeout): wait, then repeat; never a denial (§7.3) |

Every 7-class `fix` names who can act (a person, with the console command or the approval id). Provider failures leave the session `interrupted` and resumable (arch §19).

---

## 16. Testing

### 16.1 Fake providers

- **`fake` harness (in-process, TS):** a scripted provider (`{on: turnStarted, emit: [...]}` scripts with waits, tool calls, deltas, usage, requests, crashes, late events from a replaced instance). It tests the runtime: the state machine, persistence, resume, budgets, approvals, attribution, verdicts and journal emission.
- **Fake model server (loopback HTTP, TS):** replays scripted SSE streams in the Anthropic Messages, OpenAI Responses and Chat Completions formats, including 429 with `retry-after`, 529, mid-stream disconnects, malformed events, tool calls split across deltas, cache usage fields. The built-in adapters run against it with real SDKs; tests never leave loopback (D-plan rule).
- **Fake CLIs (`fake-claude`, `fake-codex`, small Rust binaries in `crates/fin-agent-fakes`):** speak stream-JSON with the control protocol (including a `system/init` with a scripted tool list, and unknown events), and newline-delimited JSON-RPC with the app-server methods of arch §27.5, from a script file; they run under the real sandbox launcher, so 7C's policies are exercised by the same tests (`fin` calls from inside, writes outside the workspace).
- **Recorded transcripts:** per supported real CLI version, captured on a validation host by a controller task, scrubbed (no paths, user or host names; checked by the same grep as the D-plan's "never raw" test), committed as contract fixtures for the adapters' mapping. Only stream messages are recorded, never a provider's system prompt.

### 16.2 Layers

| Layer | Tests |
|---|---|
| `fin-core::events` | kinds v1 round trip; every content field classed; `AGENT_*` tables; catalogue ⊆ `AGENT_MODELS` |
| Runtime (vitest) | state machine properties (random event scripts never leave a session in an impossible state; resume after any crash point never re-runs a tool), budget ledger, approvals (pending code, ≤ 90 s waits, one-time grants keyed by `(session, command, argsHash)`), rules × layers, attribution exact/inferred/ambiguous/none (the window, the argv-hash key, background processes), `session_txs` for every agent commit, `Tx.author` set by the server, the outcome tracker against scripted commit outcomes, **the dataset builder's input module never opens `control.db`** (an import-graph gate now; a file-access spy test once the builder exists) |
| Adapters | fake model server and fake CLIs; recorded-transcript contract suites; the Codex schema drift gate |
| Sandbox | §8.11 escape suites on every OS and every profile (observed denial or fail); policy unit tests (the `Only` read allow-list, enumeration around protected and denied roots, last-match profile order, the mach allow-list as data); the seccomp filter's architecture check and the io_uring refusal as unit tests of the compiled filter; ACL broker crash and concurrency tests (WAL replay, the S8 lost-grant race); the per-CLI validation list of §8.3 |
| `fin` | `tcp:` endpoint, `FIN_CALL` and `FIN_CREDENTIAL`; **`fin_with_credential_never_uses_local_key`** (with `FIN_CREDENTIAL` set to a missing, expired or revoked file and a valid `local.key` readable in the default server data, `fin` answers `UNAUTHENTICATED` and never connects as `local`; with `FIN_SERVER_DATA` set to a directory holding a `local.key`, it is ignored while `FIN_CREDENTIAL` is set); agent default branch and per-session try state (`fin-config.json` neither read nor written); client-side path arguments confined (§5.6); `FIN_AUTHOR` ignored for an agent principal; new commands, the map budget |
| e2e | an `agent` scenario per batch, driven through the console and `fin`, with the fake providers |
| Validation | real Codex, Claude Code and the three APIs on macOS, Windows and the Linux VM, as a non-elevated user, with the sandbox on |

---

## 17. Licences and dependencies

Checked against the npm and crates.io registries on 2026-11-07; each batch's plan re-runs `cargo deny` and the JS licence gate.

| Component | Use | Licence | Decision |
|---|---|---|---|
| Codex CLI 0.167.0 (`@openai/codex`, repo) | bundled app-server | Apache-2.0 | ship (7E); re-check the binary's dependencies at each pin |
| Claude Code | user-installed CLI | proprietary | drive, never bundle (D28) |
| `@anthropic-ai/claude-agent-sdk` 0.3.x | — | "SEE LICENSE IN README.md" | not used (D28) |
| `@anthropic-ai/sdk` 1.6.0 | built-in Anthropic adapter | Apache-2.0 | use (7A) |
| `openai` 7.53.0 | built-in OpenAI Responses and compatible adapters | Apache-2.0 | use (7A) |
| `ai` (Vercel AI SDK) 7.0 | — | Apache-2.0 | rejected: hides the exact request bodies we record; one more abstraction over three APIs |
| `@modelcontextprotocol/sdk` 1.32.2 | — | MIT | not needed: `fin mcp` is Rust |
| `rmcp` 3.9.0 | `fin mcp` | Apache-2.0 | candidate (7G); a hand-written stdio server if its async runtime grows `fin` too much |
| `landlock` 0.4.7 | Linux file sandbox | MIT OR Apache-2.0 | use (7C; S8 used 0.4) |
| `seccompiler` 0.5.0 | Linux syscall filter | Apache-2.0 OR BSD-3-Clause | use (7C) |
| `extrasafe` 0.5.1 | — | MIT | rejected: a wrapper over the two above with less control |
| `birdcage` 0.8.1 | — | GPL-3.0-or-later | excluded |
| bubblewrap | — | LGPL-2.0-or-later | excluded from shipping; not used (§8.3) |
| `@anthropic-ai/sandbox-runtime` 0.0.81 | — | Apache-2.0 | reference only (its Linux path needs bubblewrap) |
| `windows-sys` 0.61.2 | AppContainer, job objects, ACLs, pipe DACLs | MIT OR Apache-2.0 | use (7C; already in the tree) |
| Seatbelt (`sandbox-exec`) | macOS sandbox | OS component | use |
| `chacha20poly1305` | — | Apache-2.0 OR MIT | **not used**: the encrypted secrets file is cut (§10) |

---

## 18. Crate and file map

| Place | 7A | Later |
|---|---|---|
| `fin-core::events` | agent kinds v1, `agent.approval`, `AGENT_ROLES`, `AGENT_TOOLS`, `AGENT_MODELS` growth | — |
| `apps/finisherd/src/agent/` | `session.ts`, `store.ts` (migration), `runtime.ts` (runs, events → nodes), `adapter.ts` (contract), `fake.ts`, `builtin/{loop.ts, tools.ts, anthropic.ts, openai.ts, compat.ts}`, `catalogue/{models.json, catalogue.ts}`, `budget.ts`, `credentials.ts` (mint, branch reach), `journal.ts` (emission, attribution), `verdicts.ts`, `rpc.ts`, `console.ts` | `roles/`, `rules.ts`, `approvals.ts`, `subagents.ts` (7B); `sandbox.ts` (7C); `claude/` (7D); `codex/` (7E); `hooks/` (7F); `workspace.ts` (7G) |
| `apps/finisherd/src/secrets/` | `store.ts` (with the `SecretBox` seam), `gateway.ts` | keychain backend (with 5B) |
| `@finisher/contracts` | `agent.ts` (RPCs, node and event schemas), scopes | `registry.json` (7B) |
| `fin-node` | — (no `secrets.rs`; the second `Journal` instance for the trace store, if the napi `Journal` needs options) | `acl_broker/` (7C), per-project SIDs on 5A's `PipeServer` (7C) |
| `crates/fin-sandbox` (new; **owned by 7**) | — (7C can start now) | `policy`, `profile::{agent,hook,worker,plugin}`, `macos`, `linux`, `windows`, launcher binary, the `Spawner` for 5C (7C) |
| `crates/fin` | `--idem-key` from `FIN_CALL` (7A-1); under an agent credential: no local-key fallback, session default branch and try state, confined path arguments, server-set author (7A-2) | `tcp:` endpoint (7C), `agent`, `approval` (exit 8), `workspace`, `apply`, `context`, `guide`, `note`, `budget`, `mcp`, `credential`, `schema --registry` (7B, 7G) |
| `crates/fin-agent-fakes` (new) | — | `fake-claude`, `fake-codex` (7D, 7E) |
| `crates/fin-eval`, `packages/eval` (new) | — | scorers, runner (7H) |

---

## 19. Decisions taken here

| # | Decision | Section |
|---|---|---|
| 1 | The runtime lives in `finisherd` (TS); Rust provides the sandbox, ACL broker, scorers and event types | §3.1 |
| 2 | Conversations are product state in `control.db`, independent of consent; the journal holds the consented copy | §3.3, §12.1 |
| 3 | Tool calls get a Finisher ULID at announcement; that id is `FIN_CALL` | §3.2, §12.3 |
| 4 | The built-in agent's writes use `--idem-key <callId>`; resume never re-runs a tool | §3.4 |
| 5 | One native `fin` tool (argv, no shell) for the built-in agent; CLI agents use `fin` over their shell; `fin mcp` is an adapter promoted only by evals | §4.3 |
| 6 | Finisher builds model requests itself and uses the official SDKs as transport | §4.4 |
| 7 | A session owns a branch subtree; writes elsewhere are `BRANCH_NOT_ALLOWED` | §5.1 |
| 8 | Rollback undoes the session's transactions after the turn; **session fork is cut** | §5.3 |
| 9 | `plan` may write its own workspace | §6.1 |
| 10 | Provider-native subagents are disabled; subagents are Finisher sessions on child branches | §6.3 |
| 11 | Command rules are enforced on the server against an exported, drift-gated registry | §7.2 |
| 12 | Approvals never block an RPC: `APPROVAL_PENDING` (exit 8, not 7), `fin approval wait` ≤ 90 s and repeatable, one-time grants keyed by `(session, command, argsHash)` | §7.3 |
| 13 | Linux sandbox = Landlock (ABI ≥ 3) + seccomp refusing AF_UNIX sockets, **io_uring and the x32/compat ABIs**; `fin` reaches the server over a credentialed loopback listener; no bubblewrap | §8.3 |
| 14 | CLIs run with **per-session** config dirs (auth file copied in and synced back); the user's own CLI configs and settings are neither loaded nor writable | §4.5, §8.6 |
| 15 | The built-in agent needs no OS sandbox; CLI agents and hook commands require one (no uid-sandbox fallback) | §8.9 |
| 16 | Windows shell for CLI agents is brokered (S8 follow-up) | §4.5 |
| 17 | Agent kinds become v1 with harness, named roles, `plan|draft|auto`, tool kinds, attribution, capture fidelity, system/tools as deduplicated blobs; `agent.approval` added; `agent.editVerdict` retired | §12.2 |
| 18 | **5E's `review.verdict` is the single source of verdicts**; 7's tracker emits it for outcomes outside review items | §12.5 |
| 19 | Budgets interrupt at a safe point and open an extension approval | §13.1 |
| 20 | The eval harness scores branch state, reference similarity (word keep F1, cut F1) and process; judges never gate; comparisons are paired over ≥ 20 tasks with a bootstrap, on permissively licensed real speech | §14 |
| 21 | The agent principal's id is the session id; the server records every agent commit in `session_txs` and sets `Tx.author` | §5.2 |
| 22 | Under an agent credential `fin` never falls back to the local key; the default branch is the session root; try state is per session on the server; client-side file arguments are confined | §5.1, §5.6, §8.5 |
| 23 | CLI agents read an allow-list (toolchains, the project, the provider dir), not "everything except"; macOS `mach-lookup` is default-deny | §8.1, §8.2 |
| 24 | Training content goes to a dedicated agent-trace store as deltas plus a full-body hash; budget facts are in a message | §12.1, §4.4 |
| 25 | `fin-sandbox` and the ACL broker are 7's, with the `agent`, `hook`, `worker` and `plugin` profiles | §1.6, §8.10 |
| 26 | Claude Code runs `--bare` with an API key by default; a version floor plus a capability probe replaces a pinned range | §4.5 |
| 27 | v1 cuts: session fork, `fin tx`, a user-selected newer Codex, the Docker uid sandbox, `PreOp`/`modify` hooks, the unified-log reader, the structure-distance score, the encrypted secrets file | §23 |

---

## 20. Owner questions

1. **Which model outputs may become training targets?** Anthropic's commercial terms forbid using its services "to build a competing product or service, including to train competing AI models", and its help centre lists using outputs as training targets for models as an example; OpenAI's terms carry a similar clause. An open-weight video-editing model may or may not count as "competing". Options: (a) record everything under the training opt-in, but the dataset builder takes **targets only from human-authored edits and from open-weight model outputs**; sessions run on closed models supply only **context** (instructions, project state, tool results) and **verdict statistics** (that an edit was merged, undone or abandoned), never their outputs as targets; (b) record closed-model outputs only as metadata; (c) treat them as targets. **Recommendation: (a), with counsel before the first upload or training run.** Trade-off: (a) keeps the instruction context and the people's verdicts, the parts that are clearly the user's, and loses closed-model edits *even when a person merged them* (a merge does not make the output the user's to train on); (b) loses the instruction context the builder joins on; (c) risks the provider accounts the product depends on. (Revision 1 said targets could come from "what people kept", which would have admitted merged closed-model edits; that is withdrawn.)
2. **How exact must CLI agents' LLM traffic be?** The CLIs' streams lack their system prompts and tool definitions (§12.6). Options: (a) record the stream (messages, tool calls and results, thinking and reasoning summaries where emitted); (b) route each CLI through a local recording proxy (`ANTHROPIC_BASE_URL`, a Codex model-provider base URL) that sees the exact requests. **Recommendation: (a) in v1**, with exact capture coming from the built-in agent. Trade-off: (b) captures everything but puts Finisher in the path of the user's subscription traffic and credentials, records the vendors' proprietary prompts, may break with any CLI update and may conflict with subscription terms; (a) loses the hidden prompt, which a fine-tuned model would not reuse anyway.
3. **What runs where no OS sandbox is available?** Landlock ABI ≥ 3 needs kernel 6.2+: Ubuntu 22.04's original kernel (5.15, inside D31's floor) and Debian 12 (6.1) fall short, and many NAS kernels have no Landlock at all. Options: (a) the built-in agent everywhere; CLI agents and hook commands only with a sandbox, else `SANDBOX_UNAVAILABLE`, plus an audited owner override to run them unsandboxed; (b) run CLI agents anyway with a warning; (c) raise D31's Linux floor to kernel 6.2+. **Recommendation: (a).** Trade-off: some NAS and older-Linux users cannot use Codex or Claude Code without the override (there is no uid-sandbox fallback), against an injected agent being able to rewrite `.finisher/` and read server keys on exactly the always-on machines that hold the most media.
4. **May `auto` mode merge into `main` without a person?** Options: (a) never in v1: every agent edit reaches protected branches through review or a person's merge; (b) an owner-enabled policy per project lets `auto` merge when the QA gate and the critic pass. **Recommendation: (a).** Trade-off: "cook me five shorts" ends with five reviews to approve rather than five finished edits on `main`; (b) removes the click but makes a fooled critic the last line before the user's main timeline. Revisit with sub-project 10's data.
5. **Network egress for agents in v1, and what an injected agent can take.** The `agent` sandbox profile keeps secrets, server data, other projects and other local services out of reach (a read allow-list; loopback denied on macOS except Finisher's listener, ports limited on Linux), but egress stays open, so an injected agent can send out **(i) the project's media and the agent's workspace, and (ii) whatever its CLI must hold to work: the API key of an API-key session, or the sign-in token of a subscription session, which includes Finisher's own provider OAuth token if one was copied into the session's config directory (§8.6).** The key of (ii) is the one the user gave Finisher for that purpose and can be revoked at the provider; the footage of (i) cannot be un-sent. Options: (a) open egress in v1, documented in those words; (b) an egress proxy in `finisherd` with an allow-list (the provider's API, user-approved domains, downloads through `fin`), enforced by Seatbelt rules on macOS, Landlock TCP rules (ABI ≥ 4) on Linux and AppContainer capabilities on Windows. **Recommendation: (a) for v1, (b) after v1; and use per-project API keys with provider-side spend limits for CLI sessions.** Trade-off: (b) closes the largest remaining injected-agent risk but costs a proxy that must understand each CLI's traffic (WebSockets, streaming, certificate pinning), Windows loopback exemptions that need elevation, and a raised Linux floor.
6. **May Finisher drive Claude Code on the user's own subscription login?** Anthropic's published terms for Claude Code ("Legal and compliance", fetched 2026-11-07) say OAuth authentication "is intended exclusively for purchasers of Claude Free, Pro, Max, Ultra, Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications"; that developers building products on Claude's capabilities "should use API key authentication"; that Anthropic "does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, Max, or Ultra plan credentials on behalf of their users"; and that developers "may not collect, store, or intermediate Claude.ai credentials or session tokens — sign-in to a Claude account must complete through Anthropic's own flow". They also say the terms do not "prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription", and that the binary must not be modified and its built-in authentication methods must not be removed or restricted. **Finisher copying a sign-in file into per-session directories and syncing it back (§8.6), or running `--bare` (which accepts only an API key), may or may not fall on the wrong side of those lines; Finisher cannot decide that.** Options: (a) API keys only for Claude Code (and Codex), subscription logins not supported; (b) support the user's own subscription login in the unmodified CLI with the CLI doing its own sign-in in a directory Finisher does not copy credentials into (one login per session, started by the user); (c) support it with the copy-and-sync design of §8.6. **Recommendation: (a) until Anthropic confirms (b) or (c) in writing; the built-in agent and the CLI adapters default to API keys regardless.** Trade-off: (a) costs users who would rather spend a subscription than pay per token, and the cost-basis `subscription` stays unused; (b) and (c) are the more convenient and the riskier for the user's account. The same question for OpenAI's terms on ChatGPT-plan logins in Codex is asked of counsel together with this one.
7. **May Finisher start a provider login itself, and what is the sign-in experience in sub-project 9?** Even with API keys the user must give Finisher a key, and with any login the sign-in must "complete through the provider's own flow". Options: (a) Finisher only *stores a key the user pastes* (console `finisherd provider key set`, a UI field in 9), and never launches a login; (b) Finisher launches the CLI's own login (`finisherd provider login <harness>`, a button in 9 that opens the provider's page in the user's browser and lets the CLI complete it), without ever reading the browser flow or the resulting token except to copy the auth file as §8.6 describes. **Recommendation: (a) for v1; (b) only if question 6 is answered (b) or (c).** Sub-project 9 needs the answer to design onboarding: with (a), a "Providers" screen with key fields, a spend-limit hint and a link to the provider's console; with (b), a "Sign in with <provider>" button, device status, and sign-out. Trade-off: (a) is one more setup step for people with subscriptions and none for people with keys; (b) is friendlier and puts Finisher near the credential flows the terms restrict.

---

## 21. Contradictions and gaps found in earlier documents

1. **The reserved `agent.session` payload** (`role: main|subagent|critic|other`, `mode: interactive|headless|other`) does not match arch §8.3's named roles and `plan|draft|auto` modes, and has no harness: Codex and the built-in agent on an OpenAI key both say `provider: openai`. Resolved by §12.2.
2. **`agent.toolCall.command` is a `fin` command name**, but CLI agents mostly call their own tools (shell, edit, web fetch), which the reserved payload cannot express. Resolved by `tool` + optional `command` (§12.2).
3. **"System prompt and tool definitions by hash"** (arch §21.1) keeps no way back to the content, so training would lose them. Resolved by deduplicated blobs (§12.2).
4. **`FIN_CALL` "set by the agent harness for one `fin` run"** (D-plan Decision 23a) is impossible for Claude Code's and Codex's own shells. Resolved by exact-where-Finisher-executes plus inferred attribution (§12.3).
5. **Arch §8.4's "Landlock (bwrap fallback)":** bubblewrap is LGPL-2.0-or-later and cannot ship (AGENTS.md), and needs user namespaces that Ubuntu's AppArmor restricts (arch §27.8). Resolved by §8.3 and §8.9 (no fallback).
6. **Arch §27.6 calls Landlock "confirmed"**, but its escapes covered files only. Landlock does not mediate pathname-socket `connect()`, so D-Bus (Secret Service), `ssh-agent`, `docker.sock` and X11 were reachable. Resolved by seccomp and the agent listener (§8.3), and new escape cases (§8.10).
7. **Arch §27.6 makes `~/.claude` and `~/.codex` writable**, which lets an agent persist hooks that later run unsandboxed in the user's own terminal. Resolved by §8.6.
8. **D31 admits Ubuntu 22.04 (kernel 5.15)**, below the Landlock ABI that controls truncation. Owner question 3.
9. **Arch §8.4's env allow-list** names only `FIN_SERVER` and `FIN_SESSION`; `FIN_CALL` (D-plan), `FIN_CREDENTIAL` (sp5) and the provider directory variables are also needed, and `FIN_SERVER` cannot name a socket inside server data the sandbox denies. Resolved by §8.5.
10. **Arch §8.4 "a per-session 0600 credential file inside the session sandbox"** suggests `workspace/`, which is in the project folder and may be synced. Resolved by `<server-data>/agents/<sessionId>/` (§8.5).
11. **Arch §8.1 "Session = one agent on one branch"** vs `fin try` sub-branches, critic fix branches and subagent children. Resolved as a branch subtree (§5.1).
12. **Arch §4 rule 6 enforces rules "in the command registry, whatever the transport"**, but the registry lives in `fin`, and an agent can call RPCs directly. Resolved by the exported registry and server-side `commandOf` (§7.2).
13. **Arch §5 and §16 keep keys inside `finisherd`**, but Claude Code using an API key must hold it. Resolved by grants through `apiKeyHelper`, stated to the user (§10).
14. **Arch §8.2 lets a user select a newer `codex`**, while the vendored client is pinned. Resolved by dropping the option: the pinned bundled version only (§4.6).
15. **Arch §8.3 `auto` "runs the critic"**: the critic's rubrics and QA are sub-project 10, and render is sp6 6E. `auto` is partial until both land (§6.1).
16. **Arch §21.1 phasing puts human-only operations in sub-project 7**; sp5 moved them to 5A. What remains for 7 is making them hold (§7.5).
17. **`AGENT_MODELS` holds only Claude models**, so every Codex or OpenAI session would be `other`. Resolved by the catalogue drift test (§4.7).
18. **Arch §8.4's provider config directories and per-session home** conflict with Claude Code keeping its macOS login in the Keychain; whether a separate config dir separates the Keychain item is unverified (§8.6, 7D).

19. **`fin`'s local-key fallback and shared state** (found in review): with `FIN_CREDENTIAL` unset or unusable `fin` falls back to `local.key`; the default branch and `fin try` list live in the shared `fin-config.json`; `Tx.author` follows `$FIN_AUTHOR`; some commands read client-side files with no path check. Resolved by §5.1, §5.2, §5.6, §8.5.
20. **Revision 1's `mach-lookup` allow "for the Keychain" and its `~`-wide read access** let an agent ask the OS to start processes outside the sandbox and read most of the user's files; a deny-list always misses a path. Resolved by default-deny mach and a read allow-list (§8.1, §8.2).
21. **A Linux seccomp filter on `socket()` and native syscall numbers** is bypassed by io_uring's socket operation and by the x32 ABI; Landlock's scoping does not cover pathname sockets. Resolved by §8.3.
22. **A shared provider directory per CLI** lets one session plant configuration that a later one runs. Resolved by per-session directories (§8.6).
23. **Training content in the 128 MiB / 30-day diagnostics journal** would evict every other diagnostic within hours. Resolved by the agent-trace store (§12.1).
24. **Two verdict kinds** (`review.verdict` in sp5, `agent.editVerdict` here) would disagree. Resolved by one (§12.5).
25. **Exit 7 for a pending approval** told an agent "do not retry" for a request that will be answered. Resolved by exit 8 (§7.3).
26. **Revision 1 said training targets could come from "what people kept"**, which would have admitted closed-model edits merged by a person. Withdrawn (Owner question 1).
27. **Claude Code's subscription login in a third-party product** is restricted by Anthropic's published terms in ways revision 1's per-session credential copying may violate. Default to API keys; Owner questions 6 and 7.

---

## 22. Risks

| Risk | Mitigation |
|---|---|
| CLI protocol drift (Claude Code ships often; stream-JSON and the control protocol are not versioned contracts) | version gate; recorded-transcript contract suites per version; `PROVIDER_PROTOCOL` rather than guessing; the built-in agent as fallback |
| macOS default-deny `mach-lookup` breaks a CLI (a service we did not list) | the allow-list is derived per CLI in the spike by running under a log-only profile, committed as data, re-validated per CLI version; failures are `PROVIDER_PROTOCOL`-class start failures, not silent widenings |
| Windows per-session capability SIDs do not work as hoped for a normal AppContainer | marked *to verify* (§8.4); fallback: one container profile per session, at the cost of more broker grants |
| Apple removes `sandbox-exec` | the profile generator is independent of the launcher; `sandbox_init` from the launcher as a second path; tracked per macOS beta |
| seccomp's AF_UNIX refusal breaks a CLI or a tool it runs (SSSD user lookup, gpg-agent signing, git credential-cache and fsmonitor) | the per-CLI validation list (§8.3) and spike S9; a narrow allow-list (`socketpair` only) and a test per CLI version; fallback: Landlock ABI 6 scoping (abstract sockets only) plus the env clearing, with the pathname-socket gap recorded |
| Nested sandboxes (Codex's own Seatbelt/Landlock inside ours) refuse to apply | 7E measures it; Codex's sandbox off inside ours where nesting fails; ours is the guarantee either way |
| `CLAUDE_CONFIG_DIR` does not isolate the macOS Keychain login | 7D verifies; fall back to the user's existing login read-only, which the sandbox cannot write |
| Inferred attribution mislabels commits under parallel shell calls | `ambiguous` instead of a guess; `fin mcp` makes it exact; the builder can drop non-exact joins |
| Training capture grows fast (tool outputs, images) | a dedicated agent-trace store with its own cap and whole-session eviction; deltas plus a full-body hash; blobs deduplicated by content; the diagnostics journal and product state are separate |
| Budgets overrun between checks (a long tool call, a big turn) | checks per turn and per tool call; `--max-budget-usd` on Claude Code; per-day money cap |
| Provider terms and training use | Owner question 1 (targets only from human edits and open-weight outputs); provenance on every turn so the builder can filter |
| Provider terms on subscription logins | API keys by default; Owner questions 6 and 7; the copy-and-sync design of §8.6 is built only if they are answered yes |
| 7A is large (runtime, three APIs, budgets, secrets) | split: 7A-1 (pure pieces, before 5A) and 7A-2 (after 5A); the plan's tasks are independent behind the adapter contract; the fake provider lets the runtime merge before the APIs |

---

## 23. Review findings (revision 2) and where they are resolved

| Finding | Resolution |
|---|---|
| **B1** an agent's `fin` used the shared `fin-config.json` default branch and tries | §5.1 (default branch = session root from the principal; try state per session on the server), 7A-2 Task 8 |
| **B2** no reliable session identity for commits | §5.2 (principal id = session id; `session_txs` for every agent commit), 7A-2 Tasks 3, 7 |
| **B3** `fin` could fall back to the local key; `FIN_AUTHOR` forgeable | §8.5 (no fallback; `FIN_SERVER_DATA` cleared; test), §5.2 (server-set `Tx.author`) |
| **B4** client-side file arguments and path reach | §5.6 (confined to workspace and project folder; reach = project + referenced libraries), §6.1 (no `project.create`/`project.open`) |
| **B5** Linux seccomp gaps | §8.3 (io_uring, x32/compat ABI, `pidfd_getfd`; D-Bus pathname sockets not covered by Landlock scoping; 7C per-CLI list), §8.12 |
| **B6** macOS mach-lookup open | §8.2 (default-deny with allow-list), §8.11 (Terminal/osascript/launchctl and keychain tests) |
| **B7** shared provider dir; Windows session isolation | §8.6 (per-session dir, auth file in and synced back), §8.4 (capability SID, to verify) |
| **H1** CLI read scope; loopback; residual risk | §8.1 (allow-list), §8.2/§8.3 (loopback), Owner question 5 |
| **H2** CLI integration details | §4.5 (`--bare`, `--append-system-prompt`, print-only budget flag, floor + probe, unknown events, init-derived deny list), §4.6 (`project_doc_max_bytes=0`, developer instructions) |
| **H3** approvals | §7.3 (exit 8 pending, ≤ 90 s repeatable waits, one-time grants) |
| **H4** training store | §12.1 (agent-trace store, deltas + full-body hash), §4.1/§4.4 (budget facts in a message) |
| **H5** training targets; subscription terms; sign-in | Owner questions 1, 6, 7 |
| **H6** verdict source | §12.5 (5E's `review.verdict`; `agent.editVerdict` retired) |
| **H7** critic | §6.2 (`draft` ceiling on a child branch, or spawns a fixer) |
| Conversations vs purge/export; inline limit | §3.3, §12.1 (purge/export do not touch conversations; 9's page; builder never reads `control.db`; inline ~4 KiB) |
| Eval design | §14 (paired, ≥ 20 tasks, bootstrap; permissively licensed real speech) |
| `FIN_CALL` inference | §12.3 (window, argv-hash key, background `none`) |
| Cuts | §1.2, §19 decision 27 |
| 7A split | §1.4 (7A-1 before 5A; 7A-2 after), the 7A plan |
| Cross-design ownership; sp5 reconciliation | §1.6, §1.7, §8.10 |
