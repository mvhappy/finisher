# Sub-project 7: Agent runtime — Design

- **Status:** draft for review (2026-11-07). Nothing here is built yet.
- **Parent:** `docs/specs/2026-11-01-finisher-architecture-design.md` (cited below as *arch §n*). This document refines arch §8 (agent runtime), §9 (`fin`), §10 (hooks), §16 (secrets, audit), §21.1 (the reserved agent kinds, `FIN_CALL`), §22 (agents, sandbox) and §27.5–27.7 (spikes S7, S8). Where the two disagree, this document wins for sub-project 7, and arch §27 gets a pointer when each batch merges.
- **Builds on:** sub-project 4 (4A–4C on stacked branches), the diagnostics hooks D1–D7 (`docs/superpowers/plans/2026-11-07-diagnostics-and-training-data-hooks.md`, cited as *D-plan Decision n*), and the designs of sub-project 5 (`docs/specs/2026-11-07-sub-project-5-control-plane-design.md`, *sp5 §n*) and sub-project 6 (`docs/specs/2026-11-07-sub-project-6-media-engine-design.md`, *sp6 §n*). Both are drafts on their own branches; this design assumes them as written and lists what it needs from them (§1.4).
- **House style:** as `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`. Each batch gets its own plan; each batch section is rewritten "as built" when it lands.

---

## 1. Goal, scope and batches

### 1.1 Goal

Sub-project 7 makes agents first-class editors of a Finisher project:
- **three ways to run an agent behind one contract:** the bundled Codex app-server, the user-installed Claude Code CLI, and Finisher's own loop on user API keys (Anthropic Messages, OpenAI Responses, one OpenAI-compatible adapter), all emitting the same events (D7, D28, D30);
- **sessions** that are persisted, interrupted on a crash, resumable from any device, each on **its own branch**, with **subagents** on child branches and **roles** and **modes** that map to sub-project 5 scopes;
- an **OS sandbox** around every agent process tree on macOS, Linux and Windows, which is the guarantee (arch §27.6), with provider permission rules as a second layer;
- **hooks** on every event of arch §10, **workspace artifacts** registered with provenance, the **full `fin`** (`apply`, `batch`, `tx`, `context`, `guide`, subagents, `mcp`), **budgets** for tokens, money and render minutes, and an **eval harness** that scores agents' edits;
- **training capture:** every agent turn, tool call and result, and every verdict on an agent's edits, recorded through the journal under the consent of D34 and joined to the commits it caused through `FIN_CALL` and txIds, so a later dataset builder can fine-tune an open-weight video-editing model.

### 1.2 Non-goals

- **The conversation UI, approval cards and the branch bar** (sub-project 9). §3.5 and §7.3 define the RPCs and streams they consume.
- **The critic's rubrics, the QA gate and review markers** (sub-project 10). Sub-project 7 provides the `critic` role, read-only sessions and child-branch fixes; sub-project 10 provides what the critic looks at and how it scores.
- **The review workflow itself** (sp5 5E). Agents request reviews; people decide them.
- **Plugin-provided providers, roles and hook handlers** (sub-project 8). The contracts here accept them; nothing loads them yet.
- **Uploading anything and the dataset builder** (D34 phasing). §12 only records, and keeps the builder's join possible.
- **Hosted model credits and metering** (`finisher-cloud`, arch §5 `ModelGateway`, `Metering`).
- **An egress allow-list proxy** (Owner question 5).

### 1.3 Success, for every batch

- Every capability is driven and tested through `fin` → `finisherd`, or through `finisherd` console commands (sp5 §4.6) for the human side, with **fake providers** (§16). Real providers run only in validation tasks and in the eval harness, never in CI.
- `cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e` pass on macOS, Windows 11 and the Linux VM; the sandbox escape suites (§8.10) pass on all three.
- No UI work. No host names, IPs, user names or key names in tracked files, fixtures and recorded transcripts included.

### 1.4 Batches and order

| Batch | Delivers | Depends on | Unblocks |
|---|---|---|---|
| **7A** Runtime core and the built-in agent | the session model and store, the adapter contract, the fake provider, the built-in loop with the three model APIs, the native `fin` tool (no shell), sessions on their own branches with minted agent credentials, mode scope sets, the model catalogue and money/token budgets, `SecretStore` and `ModelGateway`, agent kinds v1 with `FIN_CALL`, edit verdicts from merges, undos and abandoned branches, `finisherd agent …` console commands | 5A, D1–D7 | everything below |
| **7B** Subagents, roles, rules, approvals | session rollback and fork (§5.3), roles as data, `fin agent spawn|wait|ls|send|stop`, budget reserves for children, saved prompts, command-pattern rules enforced on the server through the exported registry, approvals answerable from any device, review requests and verdicts from 5E, context compaction for the built-in loop | 7A, 5E | 7D, 7E, 10 |
| **7C** OS sandbox | `fin-sandbox` (launcher and policy), Seatbelt, Landlock + seccomp, AppContainer with the ACL broker, per-session directories, provider config isolation, the agent listener, violation reporting, escape suites in CI, the worker sandbox hook for sp6 6G | 7A, 5A | 7D, 7E, 7F, sp6 6G |
| **7D** Claude Code adapter | detection and version gate, stream-JSON with the control protocol, permission prompts, interrupt, resume and fork, the Windows brokered shell tool, settings isolation | 7C (7B for approvals) | 7H |
| **7E** Codex adapter | the bundled app-server, the vendored T3 client checked against a schema generated from the pinned binary, sign-in over the protocol, approvals, resume and rollback | 7C (7B for approvals) | 7H |
| **7F** Hooks | every arch §10 event, command handlers in the sandbox, project trust, `hooks:modify` grants, audit and timeline entries | 7C, 5C (`JobDone`), 5E (merges) | 8, 10 |
| **7G** Workspace artifacts and the full `fin` | `fin asset add|update` from `workspace/` with provenance, the drafts bin, `fin workspace ls|gc`, `fin apply`, `fin batch` over every write, `fin tx begin|commit`, `fin context`, `fin guide`, `fin note`, `fin mcp`, `fin budget`; wiring of `fin look`/`listen`/`render` | 7A, 5C, sp6 6A (6C, 6D, 6E for listen, SVG/Lottie validation, render) | 7H, 11 |
| **7H** Eval harness | tasks, scorers, runners per provider and model, reports in `archive/evals/` | 7D, 7E, 7G | 11 |

**Order:** 7A → 7B → 7C → (7D ∥ 7E) → 7F → 7G → 7H. 7C may start right after 7A, in parallel with 7B, on its own stacked branch: it touches only process launch and the new crate. 7D and 7E touch disjoint adapter directories and can run in parallel. 7G's sub-project 6 parts land as those batches do; its `fin` parts that need nothing from 6 (`apply`, `batch`, `tx`, `context`, `guide`, `note`, `mcp`) can move ahead of 7F.

**What 7 needs from 5 and 6, precisely:**
- **5A:** principals, `control.db`, the audit log, the `agent` credential format and verifier, `FIN_CREDENTIAL`, scopes and `HUMAN_ONLY`, exit 7. 7A mints agent credentials through the 5A store (sp5 §4.1 "sub-project 7 mints").
- **5C:** the `JobDone` bus (hooks, 7F) and job ownership by principal (render-minute budgets).
- **5E:** `review.request`, `review.verdict` and protected branches (7B; landing in 7A works through a person's `fin merge`).
- **6A/6C/6D/6E:** `fin look`, `fin listen`, SVG sanitising and template checks, `fin render` (7G). **6G** consumes 7C's sandbox for workers (sp6 Owner question 6).

### 1.5 What exists today (the baseline)

- **`fin`** (`crates/fin`): one registry (`registry.rs`, `registry_edit.rs`, `registry_settings.rs`) generating the map (`map.rs`, < 1.5k tokens, asserted), `fin help` and `fin schema`; `fin batch` over four commands (`BATCHABLE`); `fin view --budget` (`view/budget.rs`, plan 4C Decision 46); stable exit codes 0/2/3/4/5/6 (`output.rs`); the endpoint from `FIN_SERVER` or `finisherd.json` (`endpoint.rs`). D6 adds actor headers from `FIN_SESSION`/`FIN_CALL`; 5A adds the handshake, `FIN_CREDENTIAL` and exit 7.
- **`finisherd`:** `childEnv()` (`env.ts`) with the arch §27.7 allow-list and `FIN_SERVER`/`FIN_SESSION`; no agent code.
- **`fin-core::events`:** the reserved v0 kinds `agent.session`, `agent.turn`, `agent.toolCall`, `agent.toolResult`, `agent.editVerdict` (`payloads/agent.rs`), the `Agent` source, `Body` (inline or blob), and the tables `AGENT_PROVIDERS` and `AGENT_MODELS` (`names.rs`).
- **Spikes:** S7 proved both CLI protocols (arch §27.5); S8 proved Landlock and AppContainer against 90 escapes each, and decided the Windows shell tool is brokered (TASKS.md, S8 follow-up). The installed CLIs on the Linux VM today are Codex CLI 0.167.0 and Claude Code 2.1.319.

---

## 2. Principles

1. **The sandbox is the guarantee; everything else is a second layer.** Provider approval policies can be fooled (arch §27.6). Scopes stop what an agent asks the server; the OS sandbox stops what its processes do to files; neither depends on the model's good behaviour.
2. **An agent is a principal, never the user.** Every agent session has its own `agent` credential (sp5 §3.2) whose scopes come from its mode. Nothing an agent runs can obtain the `local` or `console` key (§8.5).
3. **Agent edits land on branches and reach protected branches only through a person** (D14). A session writes its own branch subtree; `main` changes by review or by a person's merge.
4. **One event model for every provider.** CLI agents and API models produce the same nodes, journal entries and budgets; what a provider cannot report is marked as such, never invented (§12.6).
5. **What the product needs is stored as product state; what training needs is a copy under consent.** Conversations are kept for resume and display whatever the consent; the journal records its copy only as D34 allows (§12.1).
6. **Every link a dataset needs is made at the source.** Tool calls carry the `FIN_CALL` id that their commits carry; when the id cannot be set exactly, the attribution says so (§12.3).
7. **Fakes first.** Every adapter is built against a fake that speaks its protocol, including its failure modes, before a real provider runs (§16).
8. **Every error is `{code, message, details, fix?}`** with a stable `fin` exit code.

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

- `control.db` (sp5 §3.1) gains `sessions`, `runs`, `nodes`, `approvals` and `budgets` tables in a sub-project-7 migration. Node payloads up to 64 KiB are stored inline; larger ones (tool output, file contents, images) go to `<server-data>/agents/<sessionId>/blobs/<blake3>` and the node holds a reference.
- **This is product state, not telemetry:** it exists so a session can be resumed, shown on another device and audited by its owner. It is not gated by consent or by `collect: never`, never uploaded, and deleted with the session (`agent.session.delete`, owner-only) or by the session retention setting (default: 90 days after `ended`).
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
| `agent.session.rollback {sessionId, toRun}`, `.fork {sessionId, atRun}` | `agent.run` | §5.3 |
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
  rollback(toTurn: number): Effect<void, ProviderError>;   // capability-gated
  fork(atTurn: number): Effect<ProviderRef, ProviderError>; // capability-gated
  ref(): ProviderRef | null;                  // what resume needs
}
interface Capabilities { steer; rollback; fork; resume; reasoning; images; parallelTools;
                         costReported; exactCapture; nativeSubagents }
```

- **`ProviderEvent`** is a closed union: `turnStarted`, `messageDelta`, `message {role, blocks}`, `reasoningDelta`, `reasoning {text?, summary?, encrypted?}`, `plan`, `toolStarted {providerId, tool, input}`, `toolOutputDelta`, `toolCompleted {providerId, ok, output}`, `request {id, kind: approval|userInput, tool?, input?}`, `usage {input, output, cacheRead, cacheWrite, reasoning, costMicros?}`, `turnCompleted {stopReason}`, `turnFailed {error}`, `exited {code, signal}`, and `capture {request, response}` (the exact model traffic, built-in only). The runtime turns them into nodes; adapters never write nodes.
- **Binding** (arch §27.5): every event is bound to the process (or HTTP stream) that emitted it; events from a replaced process are dropped; a spawn failure or exit fails the pending turn with `PROVIDER_EXITED`.
- **`SessionSpec`** carries the model, the system prompt from the role (§6.2), the tool set, the mode, the working directory `workspace/<sessionId>/`, the session environment (§8.5), the sandbox policy (§8), the budget remaining, and the provider ref when resuming.

### 4.2 CLI agents and API models

| | Built-in (API) | Claude Code (CLI) | Codex (CLI) |
|---|---|---|---|
| Process | none; HTTP from `finisherd` | `claude -p` per session, stream-JSON on stdio | `codex app-server`, newline-delimited JSON-RPC on stdio |
| Auth | Finisher's `SecretStore` via `ModelGateway` (§10) | the CLI's own login, in a Finisher-only config dir (§8.6) | the CLI's own login or an API key passed over the protocol (§10) |
| Tools | Finisher's: `fin`, `workspace_*` (§4.4) | its own (Bash, Read, Edit, Grep, WebFetch…), restricted by flags and by the sandbox | its own (shell, apply_patch, …), likewise |
| Streaming | SSE from the API | `stream_event` lines with `--include-partial-messages` | `item/*` and `turn/*` notifications |
| Approvals | the runtime decides before running a tool | `can_use_tool` control requests via `--permission-prompt-tool stdio` | `*/requestApproval` → `accept|decline` |
| Usage and cost | exact, from the response | reported per result (`usage`, `total_cost_usd`) | token usage notifications; cost priced from the catalogue |
| Capture fidelity | exact requests and responses | the stream: messages, tool calls and results, thinking where emitted; no system prompt | the stream: items, reasoning summaries; no system prompt |
| Rollback, fork | Finisher-side | `--resume --fork-session`, resume at a message | `thread/rollback`, `thread/fork` where offered |

### 4.3 Tool protocol: `fin`, not MCP, with MCP as an adapter

- **The built-in agent gets one native tool, `fin {argv: string[], stdin?: string}`**, plus `workspace_read`, `workspace_write` and `workspace_list` confined to `workspace/<sessionId>/`. Finisher executes `fin` directly (no shell, no quoting, identical on Windows), with `FIN_CALL` set to the call's id. The system prompt carries the `fin` map (< 1.5k tokens) and the role's skills; `fin help <group>` and `fin schema` are there for the rest. One tool keeps the tool definitions small and cacheable; generated per-command tools are an eval variant (7H), not the default.
- **CLI agents use `fin` through their own shell** (D16): the binary is on the session `PATH`, the session env names the credential (§8.5). This is the path every agent already knows, and the one the eval harness measures.
- **`fin mcp`** (7G) is a stdio MCP server generated from the same registry (arch §9.12): one MCP tool per command group, arguments as the `fin schema` input, output as the same JSON. It is offered to CLI agents as configuration (`--mcp-config` for Claude Code, `mcp_servers` for Codex) and becomes the default per provider only if the eval harness shows it better (fewer calls, fewer errors at equal success). It also gives exact `FIN_CALL` attribution (§12.3), because `fin mcp` mints the call id per request.
- **Rejected:** MCP as the primary interface (contradicts D16; schemas for every command cost tokens on every turn; MCP adds a long-lived process per session); provider-native tool definitions written per provider (three copies of the registry).

### 4.4 The built-in agent

- **Loop:** prompt → model call (streamed) → tool calls (read-only ones may run in parallel, writes in order) → results appended → next call, until `end_turn`, the per-run cap on model calls (default 60), a budget stop or an interrupt.
- **Model APIs** (D30): Anthropic Messages (`@anthropic-ai/sdk`), OpenAI Responses (`openai`), and one OpenAI-compatible Chat Completions adapter (`openai` with a base URL) for OpenRouter, Ollama, LM Studio and similar. Finisher builds each request body itself and records it exactly (`capture` events); the SDKs are transport, streaming and retry.
- **Prompt caching:** Anthropic cache breakpoints on the system prompt and tools and on the last stable message; OpenAI caching is automatic. Cache tokens are reported separately and priced as the catalogue says.
- **Context:** tool results over 16 KiB are cut to a head and tail with a note naming the node (the full output stays in the node and, under training, in a blob). `fin view` is called with a `--budget` derived from the remaining window (at most 4k tokens or 5 % of the window). At 85 % of the window the run stops with `MODEL_CONTEXT_EXCEEDED` in 7A; 7B adds compaction (a summary turn that replaces older runs, recorded as a node and in the journal).
- **Retries:** 429 and 529/overloaded are retried with the server's `retry-after` or backoff (1 s × 2ⁿ, ≤ 60 s, 5 attempts); other 4xx fail the run with the mapped code (§15).

### 4.5 Claude Code (7D)

- **Launch** (arch §27.5, flags verified against 2.1.319): `claude -p --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --model <m> --settings <server-data session file> --setting-sources "" --strict-mcp-config [--mcp-config <fin mcp>] --disallowedTools Task --session-id <uuid>`; resume with `--resume <id>` (`--fork-session` for fork). `--setting-sources ""` means no settings files; if the CLI refuses an empty list, 7D uses the narrowest it accepts (S7 used `project`) and points it at a directory holding only Finisher's file.
- **Settings isolation (Decision 14):** the user's own Claude Code settings, hooks, MCP servers and memory files are not loaded. Finisher writes the session's settings file (read-only to the sandbox) and a `CLAUDE.md` with the role prompt into the session workspace. `CLAUDE_CONFIG_DIR` points to Finisher's own provider directory (§8.6).
- **Native subagents are disabled** (`Task`) so that every agent has a branch, a credential and a budget (§6.3).
- **Version gate:** a supported range per Finisher release, checked at detection; outside it, `PROVIDER_VERSION` with the range in `details`. A recorded-transcript contract suite per supported version (§16).
- **Windows:** Git Bash/MSYS cannot start in an AppContainer (arch §27.6), so the Bash tool is replaced by a **brokered shell**: an MCP tool `shell {command}` served by `fin-sandbox` outside the container, which runs PowerShell inside a fresh container for the session and returns its output (TASKS.md S8 follow-up). The built-in `Bash` is disallowed on Windows.
- **Licence:** never bundled; detected on `PATH` or at a configured location (arch §18).

### 4.6 Codex (7E)

- **Bundled** app-server (Apache-2.0; licence and NOTICE shipped; binary dependencies re-checked at each pin, arch §8.2). The JSON-RPC client is vendored from T3's `effect-codex-app-server` and pinned.
- **Drift gate:** `codex app-server generate-json-schema` of the pinned binary is committed; a test checks the vendored client's messages against it, and a user-selected newer `codex` is accepted only if its generated schema is compatible with the committed one (otherwise `PROVIDER_VERSION`).
- **Configuration per session** (`-c` overrides, never the user's `config.toml`): `CODEX_HOME` = Finisher's provider directory (§8.6); `sandbox_mode = "danger-full-access"` **inside** Finisher's sandbox when nested Seatbelt or Landlock is refused, otherwise `workspace-write` as the second layer (7E verifies which per OS); `approval_policy` routed to the runtime; `shell_environment_policy.inherit = "none"` with `set` = the session env of §8.5, so commands see `FIN_SESSION`, `FIN_CREDENTIAL` and `FIN_SERVER`.
- **Sign-in:** the user's ChatGPT account through the CLI's own login flow, or an API key sent over the protocol's login call (never in the environment, §10).

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

### 5.2 The credential

- `agent.session.start` mints an `agent` principal (sp5 §4.1) with the scope set of the mode (§6.1), `projects: [projectId]`, `branches: [root]` (descendants implied), `expiresAt` = session end. It writes the credential file to `<server-data>/agents/<sessionId>/credential.json` (0600; readable inside the session sandbox only, §8.5) and sets `FIN_CREDENTIAL` to its path.
- Ending or deleting the session revokes the credential; a resumed session keeps it (agent keys derive from the master and survive restarts, sp5 Decision 3).
- `Tx.author` for an agent's writes is `agent:<role>:<sessionId>` (free text, keyed-hashed by the journal).

### 5.3 Turn checkpoints, rollback, fork

- Each run records `headAtStart` (the root branch's seq) and the txIds its tool calls produced.
- **Rollback to run *r*** undoes, newest first, every transaction the session committed after *r* started (4B undo units, as the session's author), then rolls the provider back (`rollback` capability) or, for the built-in agent, drops the later nodes from the next request. A conflicting undo fails the rollback with `CONFLICT` and leaves both sides as they were.
- **Fork at run *r*** creates a new session whose root branch is created from the old root at `headAtStart` of *r*+1 and whose provider is forked at the same turn. The old session is untouched.

### 5.4 How edits land

1. The agent works on its branch; every write returns its delta (arch §9.4).
2. At the end of a brief, the agent calls `fin review request --title … [--summary …]` (5E; in 7A, `fin branch ls` shows the branch and the session ends with a summary node). The `BranchReady` hook runs (7F).
3. **A person** reviews the change list (5E: per-transaction accept or reject, then merge) or, before 5E, merges with `fin merge --from <branch>` as the `local` principal or from the console.
4. **Agents never merge into a protected branch** (sp5 §8.5: no `branch.protected`, no `review.decide`). In `auto` mode too (Owner question 4).
5. A parent agent may merge a child's branch into its **own** branch (7B), which is how subagent work composes before review.

### 5.5 Concurrency

- Sessions on separate branches run in parallel (D18), limited per server (default 4 live provider processes, 8 built-in sessions; settings) and per provider by its rate limits (§13.3).
- All writes go through the project's single writer (arch §6.5); a rebase conflict on `tx.commit` is answered to the agent, which re-plans (sp5 §8.4: not a review item).

---

## 6. Roles, modes, subagents, saved prompts

### 6.1 Modes and their scopes

| Mode | Scopes (sp5 §4.4 vocabulary, plus §3.5) | Sandbox writes | Typical use |
|---|---|---|---|
| `plan` | `server.read`, `project.read`, `library.read`, `jobs.read`, `diagnostics.read` (redacted); `jobs.submit` for `look` jobs only | `workspace/<session>/` only | read, analyse, propose; no ops |
| `draft` | `plan` + `project.write` (own branches, §5.1), `jobs.submit` (proxies, analysis, `look`, `listen`), `review.write`, `agent.run` for own children (7B) | `workspace/<session>/`, `workspace/shared/` | edit on its own branch |
| `auto` | `draft` + `render` and `critic` jobs within the render-minute budget | as `draft` | edit, render, self-review within budget |

- No mode grants a human-only scope, `branch.protected`, `review.decide`, `jobs.control`, `project.admin`, `library.admin`, `settings.admin`, `auth.admin` or `plugin.install` (sp5 §4.4 "agent"). A role may narrow its mode, never widen it.
- **`plan` may write its own workspace** (notes, scratch): `workspace/` is never canonical state (D22). "Read-only" in arch §8.3 means read-only to the project.
- `auto`'s critic needs sub-project 10; until then `auto` behaves as `draft` plus render.

### 6.2 Roles

- **Data, not code** (`apps/finisherd/src/agent/roles/*.json`, later contributed by plugins): `{name, title, prompt (skill references, `fin guide` topics), modeCeiling, models: {builtin, claudeCode, codex}, rules (allow/ask/deny, §7.2), budget, tools}`.
- **Bundled:** `editor` (the default main agent), `critic` (`plan` ceiling; reads renders and QA), `clip-finder`, `caption-stylist`, `colourist`, `mixer` (arch §8.3). Their prompts are skills borrowed from Uncut (D26), versioned with the server (arch §9.1).
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
4. **Hooks** (§9): policy code (`PreCommand`, `PreOp`, `PreMerge`).
5. **Provider permission rules** (Claude Code allow/deny, Codex approval policy): a convenience, never relied on.

### 7.2 Command rules on the server

- Arch §4 rule 6 enforces rules "in the command registry, whatever the transport", but an agent with a credential can call RPCs without `fin`. So rules are enforced **in `finisherd`'s authorisation wrapper**, against command names.
- **The registry is exported:** `fin schema --registry` writes every command's name, group, the RPC(s) it calls and the payload fields that identify it; the result is committed as `packages/contracts/registry.json` with a drift gate, like the generated types. The server maps each incoming RPC + payload back to its command name (`commandOf`), so a rule on `render` or `plugin.install` holds whether the call came from `fin`, `fin mcp`, a hand-written client or the built-in tool.
- **Rule syntax:** `{allow|ask|deny: "<command glob>", where?: {branch?, mode?, preset?, bytesOver?, costOver?}}`, layered `managed` > `project` > `user` > `local` > role (arch §8.3); the first `deny` wins, then `ask`, then `allow`; no match is `allow` within the scopes. Project rules are honoured only for trusted projects (§9.4).
- **Defaults:** `deny plugin.install` (arch §8.3), `ask render` over 10 minutes of output in `draft`, `ask asset.add` of a download without a recorded licence (7G), `ask workspace.gc`.

### 7.3 Approvals

- An `ask` (a rule, a budget extension, a provider permission the policy routes to people, §7.4) creates an **approval**: `{id, sessionId, kind: rule|budget|provider|plugin, subject (command and resolved arguments, server-written), requestedAt, expiresAt (default 30 min), decision?, by?, remember?}`. The card text is generated by the server from the subject, never worded by the agent (arch §8.3).
- **The request does not block an RPC.** `fin` gets `APPROVAL_REQUIRED` (exit 7) with `details.approval`; `fin approval wait <id> [--timeout <s>]` blocks up to 10 minutes and exits 0 on allow, 7 `APPROVAL_DENIED` on deny or expiry. The built-in loop waits on the approval itself (status `waitingApproval`) and gives the model the outcome as the tool result.
- Answered from any device (`agent.approval.answer`, scope `agent.approve`, human only), or the console (`finisherd agent approve <id> allow|deny`). `remember: session|project` adds a rule.
- Approvals are product state in `control.db` and journal entries (`agent.approval`, §12.2).

### 7.4 Provider permission requests

- Claude Code `can_use_tool` and Codex `requestApproval` are answered by the runtime: **allow** what the sandbox contains (file edits in the workspace, shell commands, reads), **deny** what the mode forbids (writes outside the workspace in `plan`), and route to an approval what a rule marks `ask` (by tool and pattern, for example `WebFetch` or `Bash(curl *)`).
- The decision is recorded as an `approval` node with `by: policy|user`.

### 7.5 Human-only operations

sp5 §4.5 makes consent changes, content export, purge, `project default`, confirmed takeovers, relocations, library administration, pairing and plugin installation human-only in 5A, ahead of the arch §21.1 phasing. Sub-project 7 adds what only it can:
- **making them hold:** the sandbox denies agents read access to server data (`local.key`, `console.key`, `agent.master`, `settings.json`, the journal, the audit log), so an agent cannot obtain a human credential (sp5 §4.5 residual);
- **new human-only operations:** answering approvals; starting, ending, deleting sessions on behalf of a person (`agent.run` is human, except a parent for its own children); provider sign-in and API keys; `agents.unsandboxed` overrides (Owner question 3); trusting a project's hooks; granting `hooks:modify`.

---

## 8. The OS sandbox (7C)

### 8.1 The policy

One pure type in `fin-sandbox::policy`, built by the runtime per session and per hook command, applied by the launcher per OS:

```rust
pub struct SandboxPolicy {
    pub write: Vec<PathBuf>,        // workspace/<session>, workspace/shared, the session dir's tmp/ and provider/
    pub read: ReadPolicy,           // Everything { except: Vec<PathBuf> } | Only(Vec<PathBuf>)
    pub protect: Vec<PathBuf>,      // .finisher/, assets/, finisher.project.json: no write, no link (arch §27.6)
    pub deny_read: Vec<PathBuf>,    // server data (except the session dir), sensitive home paths (below)
    pub unix_sockets: Vec<PathBuf>, // the only pathname sockets it may connect to (the agent listener)
    pub net: Net,                   // Open | None (hooks default to None)
    pub limits: Limits,             // processes, memory, wall clock (job object / rlimits)
}
```

- **Writable:** the session workspace, `workspace/shared/` (not in `plan`), a private temp directory and the provider's state directory inside `<server-data>/agents/<sessionId>/` (§8.5).
- **Read-only:** the project folder; registered libraries (read through `fin` anyway); the system and toolchains the CLIs need.
- **Denied for read:** server data except the session's own directory; `~/.ssh`, `~/.gnupg`, `~/.aws`, `~/.config/gcloud`, `~/.kube`, `~/.docker`, `~/.netrc`, `~/.config/gh`, browser profiles, password-manager data and the user's own `~/.claude` and `~/.codex` (§8.6). The list is data, extendable in settings.
- **Never writable, never linkable:** `.finisher/`, `assets/`, `finisher.project.json`, the session's own settings and credential files.

### 8.2 macOS: Seatbelt

- `/usr/bin/sandbox-exec -p <profile>` with parameters (`-D`), as S8 did and as Codex and Chromium do. The profile is generated from the policy: deny-default for writes, allow-list of write roots, `(deny file-write* file-link* (subpath protected))`, `(deny file-read* (subpath denied))` followed by the allow of the session directory (last match wins), `network-outbound` to unix sockets only by `path-literal` of the agent listener, IP networking open.
- **Keychain:** Claude Code keeps its login in the Keychain, so `mach-lookup` to the security server stays allowed; Finisher's own secrets are stored with an access list naming only Finisher (§10), so a sandboxed process asking for them is refused or prompts the user. 7C tests both.
- `sandbox-exec` is deprecated but present on macOS 14–26; its removal is a tracked risk (§22).

### 8.3 Linux: Landlock + seccomp

- **Landlock** (`landlock` crate) for files, as S8 confirmed (90/90, ABI 4): write roots enumerated around protected roots (Landlock only adds access); read granted on enumerated siblings around denied roots, so server data and the sensitive home paths are never under a granted tree; `REFER` (link and rename across directories) only within write roots; `TRUNCATE` handled (ABI ≥ 3).
- **seccomp** (`seccompiler`) for what Landlock cannot see: **`socket(AF_UNIX, …)` is refused** (`EACCES`). Landlock does not mediate `connect()` on pathname sockets (sp5 §14 item 2), so without this an agent can reach the D-Bus session bus (and through Secret Service, keyring secrets), `ssh-agent`, `/var/run/docker.sock` or an X11 socket. `socketpair()` stays allowed (libuv stdio). Also refused: `ptrace`, `process_vm_*`, `keyctl`, `bpf`, `perf_event_open`, mount and namespace calls, `personality`.
- **The agent listener:** because AF_UNIX is closed, `fin` inside the sandbox reaches `finisherd` over a loopback TCP listener that accepts only `agent` credentials (sp5's handshake proves both sides on any transport). `FIN_SERVER=tcp:127.0.0.1:<port>`; `fin`'s endpoint parser gains the `tcp:` form.
- **Floor:** Landlock ABI ≥ 3 (kernel ≥ 6.2) for CLI agents and hook commands. Below it, or without Landlock (old NAS kernels), see §8.9.
- **Docker:** Docker's default seccomp profile allows the Landlock syscalls (since 2022) and installing a stricter filter under `no_new_privs`; the image's validation row checks both on the reference NAS.
- **bubblewrap is not used:** it is LGPL-2.0-or-later and cannot ship (AGENTS.md), and it needs unprivileged user namespaces, which Ubuntu 24.04's AppArmor restricts (arch §27.8). This replaces the "bwrap fallback" of arch §8.4.

### 8.4 Windows: AppContainer

As S8 confirmed (arch §27.6): a normal AppContainer (not LPAC), **one container profile per project** (`finisher-<project-uuid>`), allow-only ACLs granted once through **one ACL broker in `fin-node`** with a write-ahead log and a startup sweep, protected roots outside write roots (or non-inheriting, with delete-child removed from the parent), a private TEMP and home, a private window station and desktop, a kill-on-close job object with process and memory limits, standard handles only, copies (never moves) into `assets/`.
- **`fin` → `finisherd`:** a per-project named pipe whose DACL grants the user and that container SID, created by `fin-node` and bridged into the RPC server (Node cannot set pipe DACLs; sp5 §4.2). Loopback TCP is blocked for AppContainers.
- **Shell:** brokered (§4.5).
- **Credential Manager** is denied inside the container (S8 follow-up), which protects Finisher's secrets there.

### 8.5 The session environment and directories

`<server-data>/agents/<sessionId>/`:
- `credential.json` (0600; read-only to the sandbox), `settings/` (provider settings Finisher writes; read-only), `tmp/` (`TMPDIR`/`TEMP`), `provider/` (scratch state the CLI writes per session), `blobs/` (product state, §3.3; denied to the sandbox).

The environment is `childEnv()` plus exactly: `FIN_SERVER` (the agent listener or pipe), `FIN_SESSION` (the session id), `FIN_CREDENTIAL` (the credential path), `FIN_CALL` (only for processes Finisher starts per tool call: the built-in `fin` tool, `fin mcp` requests, the brokered shell), `TMPDIR`, and the provider's directory variable (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`). `FIN_TRAINING`, `FIN_DIAGNOSTICS`, `SSH_AUTH_SOCK`, `DBUS_SESSION_BUS_ADDRESS`, `DOCKER_HOST`, `DISPLAY`, `WAYLAND_DISPLAY` and every `*_API_KEY` never pass (arch §27.7, D-plan Decision 14b).

### 8.6 Provider configuration isolation

S8 made `~/.claude` and `~/.codex` writable to the sandbox. That is a **persistence escape:** an injected agent could add a hook or an MCP server to the user's own Claude Code or Codex configuration, which then runs **unsandboxed** the next time the user starts that CLI in a terminal. Therefore:
- Finisher runs each CLI with its own provider directory under server data (`<server-data>/providers/claude`, `<server-data>/providers/codex`), set through `CLAUDE_CONFIG_DIR` and `CODEX_HOME`; the user signs in once for Finisher (onboarding, sub-project 9; until then `finisherd provider login <harness>`, which runs the CLI's login with that variable).
- Inside the sandbox that directory is **read-only except the paths the CLI must write** (session transcripts, caches), which 7D/7E enumerate per version; settings files in it are written by Finisher only.
- The user's own `~/.claude` and `~/.codex` are denied for read and write.
- Whether `CLAUDE_CONFIG_DIR` also separates the macOS Keychain entry is verified in 7D; if not, the Finisher login is a separate Keychain item named by Finisher, or the user's existing login is used read-only.

### 8.7 Network

Agents need egress: their own model API, downloads of media and fonts. In v1 egress is **open** for agent trees and **closed** for hook commands (unless a hook declares `net`). The sandbox's read denials (server data, secrets, the user's provider logins) bound what an injected agent can exfiltrate to the project's media and the agent's own workspace (Owner question 5).

### 8.8 Violations

- A denied operation is an `EACCES`/`EPERM`/`ACCESS_DENIED` to the process; the CLI sees a failed tool call.
- Where the OS reports it (Seatbelt's `deny` log via the unified log on macOS, which 7C reads for the sandbox's process tree; Landlock's audit records on kernels with Landlock audit support; seccomp `SECCOMP_RET_ERRNO` counted by a `USER_NOTIF` only in tests), `finisherd` writes an audit entry `sandbox.violation {session, op, root kind}` (sp5 reserved kind) and a `notice` node. Paths are stored as root kind and keyed hash only.
- Tampering that slips past is still caught: assets are content-addressed and verified on use; `.finisher/` segments are hash-chained (arch §8.4, sub-project 4 §3).

### 8.9 Hosts without a usable sandbox

- **The built-in agent always runs:** its tools are `fin` (a credentialed RPC client) and confined workspace file tools executed by `finisherd`; it has no shell. It needs no OS sandbox.
- **CLI agents and hook commands require the sandbox.** Without it (Landlock ABI < 3, no Landlock, `sandbox-exec` missing, AppContainer creation failing), starting one is `SANDBOX_UNAVAILABLE` with a `fix` naming the kernel or OS requirement.
- **In the Docker image**, where `finisherd` can be given `CAP_SETUID`, a **uid sandbox** is offered as a fallback: the agent tree runs as a dedicated uid that owns only the session directories and has read access to the project; protected roots belong to `finisherd`'s uid. This covers NAS kernels without Landlock.
- An owner-only override `agents.unsandboxed: allow` (audited, shown on every session) runs CLI agents with the env allow-list and scopes only (Owner question 3).

### 8.10 Escape suites

The S8 suites become tests (arch §22): the 90 core escapes, the Windows-specific paths (junctions, ADS, 8.3 names, `\\?\`, admin shares, WMI, `schtasks`), plus the new ones: connecting to a D-Bus session bus, an `ssh-agent` socket and a fake `docker.sock` (Linux); `security find-generic-password` for a Finisher item (macOS); reading `local.key`, `console.key`, `settings.json` and the journal; writing the user's `~/.claude/settings.json`; truncating a segment; hard-linking out of `assets/`. Each passes only with an observed denial. They run in CI on all three OSes and in the validation tasks as a non-elevated user.

---

## 9. Hooks (7F)

### 9.1 Events and replies

Arch §10's table holds. Wiring:

| Event | Fired by | May |
|---|---|---|
| `SessionStart`, `SessionEnd` | the runtime | add context; log |
| `UserPrompt` | `agent.session.prompt`, saved prompts | add context; block |
| `PreCommand`, `PostCommand` | the authorisation wrapper, for every command from an `agent` principal (and for every principal when a hook asks for `all`) | block with reason; add context |
| `PreOp`, `PostOp` | the `tx.commit` path, before apply and after commit | veto; annotate; trigger follow-ups |
| `BranchReady` | `review.request` from a session, or a session ending with commits | extra QA; gate the critic |
| `PreMerge`, `PostMerge` | `history.merge`, `review.merge` | enforce policy; notify |
| `PreRender`, `PostRender`, `JobDone` | the jobs bus (5C) | validate; publish; notify |
| `Approval`, `Notification` | approvals, session status changes | route to a phone or a chat service |

- **Replies:** `allow | block {reason} | context {text}`; `modify {payload}` only with a per-hook `hooks:modify` grant (human-only), with both payloads in the audit log (`hook.modify`) and the session timeline.
- **Cost:** a hook point with no registered hook costs one map lookup. `PreOp` handlers have a 2 s timeout (they sit on the commit path); others 10 s by default, 60 s at most. A timeout is a visible `HOOK_TIMEOUT` notice; whether it blocks is per hook (`onTimeout: allow|block`, default `allow` except `PreMerge`).
- **Order:** managed, user, project, local, plugin; within a layer, file order. The first `block` stops the chain; `context` replies concatenate.

### 9.2 Handlers

- **Commands:** JSON on stdin (`{event, v, session?, project, payload}`), one JSON reply on stdout; run through `fin-sandbox` with the cleared environment, `net: None` unless declared, the project read-only, a private temp directory, the timeout, and no credential unless the hook declares `fin` (then a `plan`-scoped agent credential of its own).
- **Plugin RPC methods:** sub-project 8, same reply shape and timeouts.

### 9.3 Configuration

`hooks` in `managed` settings, `<server-data>/settings.json` (user), `finisher.hooks.json` beside `finisher.project.json` in the project folder (project: it travels with the folder, so it is trust-gated, §9.4), and `<server-data>/projects/<id>/hooks.json` (local).

### 9.4 Project trust

- Project hooks and project command rules stay **disabled until the owner trusts the project** (arch §10): `control.db` `project_trust {projectId, hooksHash, trustedAt, by}`. A change to the project's hooks (hash) disables them again until re-trusted. Trusting is human-only (`finisherd project trust <id>`, later the UI).
- Hook blocks and modifications are audited (`hook.block`, `hook.modify`, sp5 reserved).

---

## 10. Secrets and `ModelGateway`

- **`SecretStore`** (TS interface; backends: the OS keychain through 5B's `keyring` binding, and an encrypted file `<server-data>/secrets.json` for headless servers, arch §16). 7A ships the file backend (0600, `XChaCha20-Poly1305` with a key in `<server-data>/auth/secrets.key`) and switches to the keychain when 5B lands. Entries: `{provider, label, kind: apiKey|baseUrl+apiKey, createdAt}`; values are never returned by any RPC.
- **`ModelGateway.grant({provider, sessionId, purpose})`** returns a short-lived in-memory handle used by the built-in adapter for one request; the key is read at call time and never leaves `finisherd` (arch §16). Every grant is counted per session; the first grant of a session and every denied grant are audited (`grant`, sp5 reserved).
- **CLI agents sign in on their own** (§8.6). When the user wants a CLI to use a key kept in Finisher:
  - **Codex:** the key goes over the app-server's login call on stdin.
  - **Claude Code:** `apiKeyHelper` in the session settings runs `fin credential model-key anthropic`, which obtains a grant over the agent listener. The key then exists inside that sandbox (the CLI must hold it), which is stated in the UI; it is never in the environment, a file or the journal.
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
- **`fin batch`** accepts every write command (today four), each line one transaction unless inside `tx`.
- **`fin tx begin|commit|abort`:** a server-side open transaction per session (one at a time, 10-minute idle expiry), committed atomically.
- **`fin context`:** project, branch, session, mode, role, budget left, open approvals, children, recent notes, in ≤ 800 tokens.
- **`fin guide <task>`:** the role's skills by topic, versioned with the server.
- **`fin note <text>`:** a note node in the session and a workspace file; content, not a marker.
- **`fin budget`:** remaining tokens, money, render minutes, wall clock.
- **`fin mcp`** (§4.3), **`fin agent …`** (7B), **`fin approval wait`** (7B), **`fin credential model-key`** (§10).
- **The map stays under 1.5k tokens** (its test); new groups appear in the map by name only.

---

## 12. Diagnostics and training capture

### 12.1 Two stores, one source

- The runtime writes **product state** (§3.3) for every node, and **journal entries** through `fin-node`'s `Journal` (D-plan Decisions 15–20) with `source: agent` (the runtime inside `finisherd`; sp5 §3.4 keeps `fin` for what `fin` itself sends).
- The journal applies D34: metadata under diagnostics; content (prompts, outputs, tool arguments and results) only under training, which only a person can turn on; nothing for a project with `collect: never` or unknown. `agent.turn`, `agent.toolCall` and `agent.toolResult` stay training-only kinds (their metadata part is written under diagnostics).

### 12.2 Kinds (version 1, replacing the v0 reservations)

| Kind | Meta | Content (training only) |
|---|---|---|
| `agent.session` | `sessionId`, `phase: started|resumed|interrupted|ended`, `harness: builtin|claudeCode|codex|fake|plugin`, `provider` (`AGENT_PROVIDERS`), `model` (`AGENT_MODELS`), `modelHash?`, `role` (`AGENT_ROLES`), `mode: plan|draft|auto`, `parent?`, `trigger: user|savedPrompt|subagent|hook|eval`, `reason?` (code), totals at `ended` (turns, tool calls, txs, tokens, `costMicros`, `wallS`) | — |
| `agent.turn` | `turnId`, `runId`, `seq`, `parentTurnId?`, provider, model, input/output/cache-read/cache-write/reasoning tokens, `latencyMs`, `ttftMs`, `stopReason`, `costMicros?`, `costBasis: reported|priced|subscription|unknown`, `capture: exact|stream`, `systemHash`, `toolsHash` | `messages` (the turn's new input messages), `output`, `system`, `tools` — each a `Body`, a blob when over 16 KiB; `system` and `tools` are always blobs, so identical prompts are stored once by content address |
| `agent.toolCall` | `callId` (the `FIN_CALL` ULID), `turnId`, `tool` (`AGENT_TOOLS`: `fin`, `shell`, `read`, `write`, `edit`, `search`, `webFetch`, `webSearch`, `mcp`, `workspace`, `other`), `command?` (`CLI_COMMANDS`, when the call runs `fin`), `argsHash` | `args` |
| `agent.toolResult` | `callId`, `ok`, `code?`, `exit?`, `micros`, `txIds`, `attribution: exact|inferred|ambiguous|none` (§12.3), `truncated` | `output` |
| `agent.approval` (new) | `approvalId`, `sessionId`, `kind: rule|budget|provider|plugin`, `decision: allow|deny|expired`, `by: user|policy`, `latencyMs` | `subject` |
| `agent.editVerdict` | `txIds` (≤ 64), `sessionId`, `verdict: accepted|rejected|undone|reverted|merged|abandoned`, `by: user|critic|agent`, `via: review|undo|revert|merge|archive|critic`, `afterS` (commit to verdict) | `reason` (a reviewer's comment, 5E) |

- These change the reserved v0 payloads in place (allowed for v0, D-plan Decision 3) and set version 1 when 7A's emitters land. `agent.approval` is a new kind.
- **New tables** in `fin_core::events::names`: `AGENT_ROLES`, `AGENT_TOOLS`; `AGENT_MODELS` grows to the catalogue (§4.7). `harness` is a closed enum.

### 12.3 `FIN_CALL` and attribution

- **Exact:** the call id is set as `FIN_CALL` for every process Finisher starts per tool call (the built-in `fin` tool, `fin mcp` requests, the brokered shell). `fin` sends it in its actor header (D6); 5A lets an `agent` principal set only `call`; every `log.commit` and `cli.command` of that run carries it, and `agent.toolResult.txIds` lists the commits. `attribution: exact`.
- **Inferred:** a CLI agent's own shell runs `fin` without per-call environment (neither Claude Code nor Codex lets a host set environment per tool call). When a request from an agent principal carries no `call`, the runtime looks at that session's open tool calls of kind `shell`: exactly one open → that call, `attribution: inferred`; several → no call on the commit, `ambiguous` on each candidate result, with the txIds listed on all of them.
- **Rejected:** rewriting the agent's shell command through the permission callback to prefix `FIN_CALL=…` (it changes what the model wrote, shows in its transcript, and corrupts the training record).
- `fin mcp` makes CLI attribution exact; the eval harness weighs that (§4.3).

### 12.4 Large content

- `Body::Blob` through `Journal::attach_blob` (D-plan Decision 20) for anything over 16 KiB; the 4 MiB blob cap holds, larger outputs are truncated with `truncated: true` (the product node keeps the full output).
- Images given to or produced by a model are blobs (`image/png`, `image/webp`); frames from `fin look` reach the journal only through these tool results.

### 12.5 Edit verdicts

A **verdict tracker** in `finisherd` observes commit outcomes (D5's classification, surfaced through the history RPC handlers) and review events (5E), and emits `agent.editVerdict` for transactions authored by an agent session:
- `merged` when a merge copies them into a branch outside the session's subtree (by the merging principal: `user`, or `agent` for a parent's merge of a child);
- `undone` / `reverted` when an undo or revert names them (`by` from the actor: a person, the agent itself, or a critic session);
- `accepted` / `rejected` per transaction from review decisions (5E, with the comment as content);
- `abandoned` when the session's root branch is archived, or the session is deleted, with unmerged transactions;
- `critic` verdicts when a critic session's child-branch fix undoes or replaces them (10).
- One verdict per transaction per kind of event; a later verdict does not erase an earlier one (the builder reads the sequence).

### 12.6 Capture fidelity per provider

- **Built-in:** `capture: exact`: the request bodies as sent (system, tools, messages) and the response as received.
- **CLI agents:** `capture: stream`: what the CLI's stream reports. Claude Code reports messages with thinking blocks where it emits them, tool uses and results, and usage; Codex reports items, reasoning summaries and usage. Neither reports its system prompt or tool definitions, so `system`/`tools` are absent and `systemHash` is the hash of what Finisher added (role prompt, settings). A recording proxy for exact CLI traffic is Owner question 2.

### 12.7 The dataset builder's join (kept possible, not built)

Example = state at `log.commit.baseRev` + instruction (the run's `userMessage` and the turns before the call, joined by `turnId` → `callId` → `ctx.actor.call` and `txIds`) + intent (`log.commit.intent`); label = the `agent.editVerdict` sequence and D-plan Decision 34's undo/merge/abandon signals. Provenance fields (`harness`, `provider`, `model`, `capture`) let the builder choose which outputs may be training **targets** (Owner question 1).

---

## 13. Cost and budgets

### 13.1 Budgets

- **Kinds:** input+output tokens (cache reads weighted by their price ratio), money (`costMicros`), render minutes (5C jobs owned by the session's principal), wall clock, model calls per run, tool calls per run.
- **Scopes:** per session (from the role, overridable at start), carved for children from the parent's remaining budget (a reserve that returns unused on the child's end), and per server per day for money (owner setting; off by default for subscription logins, whose usage is shown in tokens only).
- **Enforcement:** at 80 % a `notice` node and a `Notification` hook; at 100 % the runtime interrupts the current turn at the next safe point (after the running tool call), sets the session `interrupted` with `BUDGET_EXCEEDED`, and opens a `budget` approval to extend. Claude Code also gets `--max-budget-usd` as a backstop.
- **Accounting** is per `agent.turn` from usage events; a CLI that does not report cost is priced from the catalogue (`costBasis: priced`); a subscription login is `costBasis: subscription` with notional cost shown but not charged against money budgets unless the user asks.

### 13.2 Ledger

`control.db` `budgets {scope, kind, limit, used, reserved}` updated per turn in one statement; `fin budget` and `agent.session.get` read it.

### 13.3 Rate limits

Per provider and key: concurrent requests (default 4) and a token bucket from the provider's headers where present; 429 backoff for the built-in agent (§4.4); CLI agents handle their own and their waits show as `notice` nodes.

---

## 14. The eval harness (7H)

### 14.1 What it scores

Arch §9.13 and §22: CLI changes and agents' edits. A **task** (`evals/tasks/<id>/task.json`, tracked) is a brief, a starting project (a `fin batch` script over fixtures), the mode and role, and a **scorer spec**. Fixtures are synthetic (sp6's generated media) or rights-cleared clips in the local fixture cache (`FIN_MEDIA_FIXTURES`, sp6 §13.3); permissive only, never committed media (AGENTS.md). Results go to `archive/evals/<date>/` (git-ignored); only conclusions reach a spec.

### 14.2 Scorers (`crates/fin-eval`, linking `fin-core`)

- **State checks:** predicates over the final snapshot of the session branch (`item count by kind`, `caption coverage of a range`, `no CAPTION_OVERFLOW`, `duration within ±0.5 s of target`, `a clip trimmed to word w:240 ± 1 frame`), each pass/fail with a reason.
- **Reference comparison** (when the task has a reference edit):
  - **word keep F1:** label each transcript word of the sources kept or dropped by the timeline's coverage of it, compare with the reference (robust to ripple positions);
  - **cut F1:** output-timeline cut points matched within ±2 frames;
  - **structure distance:** tracks, item kinds, caption blocks, graphics slots.
- **QA** (sub-project 10): the deterministic gate's findings, once it exists.
- **Process:** success (all required checks), model calls, tool calls, `fin` errors by code, retries (a command repeated after an error), undos, tokens, cost, wall time, approvals asked.
- **Judge (optional):** the critic role's rubric score, reported beside the others, never a gate.

### 14.3 Runs and reports

- `pnpm eval --tasks <glob> --harness builtin,claudeCode,codex --model <m>… --n 5` drives sessions through the console credential, scores each final branch, and writes JSONL plus a summary (pass rate with a Wilson interval, medians of process metrics) per harness × model × task.
- **Comparing two builds** (a CLI change): same tasks, same models, `n ≥ 5`; better means a pass rate not lower beyond its interval and fewer calls or tokens.
- **In CI:** the harness itself runs against the scripted fake provider (§16), which tests runners and scorers, not models. Real-provider runs are manual, with the owner's keys.

---

## 15. Error codes

| Exit | Codes |
|---|---|
| 2 | `AGENT_ARGS` (bad `fin agent` usage), `TX_NOT_OPEN`, `TX_ALREADY_OPEN` |
| 3 | `SESSION_NOT_FOUND`, `ROLE_UNKNOWN`, `MODE_INVALID` (above the role's ceiling or the parent's mode), `HARNESS_UNKNOWN`, `MODEL_UNKNOWN`, `SAVED_PROMPT_UNKNOWN`, `APPLY_INVALID {index, error}`, `ASSET_TOO_LARGE`, `ASSET_TYPE_UNSUPPORTED`, `APPROVAL_NOT_FOUND` |
| 4 | `SESSION_BUSY`, `SESSION_ENDED`, `BRANCH_IN_USE`, `TX_EXPIRED`; `CONFLICT` for a failed rollback |
| 5 | `PROVIDER_UNAVAILABLE` (not installed), `PROVIDER_VERSION {range}`, `PROVIDER_AUTH` (not signed in, key missing or refused), `PROVIDER_RATE_LIMITED {retryAfterMs}`, `PROVIDER_OVERLOADED`, `PROVIDER_PROTOCOL` (a message the adapter cannot map: drift), `PROVIDER_EXITED {code}`, `MODEL_CONTEXT_EXCEEDED`, `SANDBOX_UNAVAILABLE {os, need}`, `SESSION_INTERRUPTED` (resumable), `WAIT_TIMEOUT`, `HOOK_TIMEOUT` (when blocking) |
| 7 | `APPROVAL_REQUIRED {approval}`, `APPROVAL_DENIED`, `RULE_DENIED {rule, layer}`, `BRANCH_NOT_ALLOWED {branch, root}`, `BUDGET_EXCEEDED {kind, used, limit}`, `HOOK_BLOCKED {hook, reason}`, `MODE_FORBIDS {mode, command}`, `DEPTH_EXCEEDED` |
| 6 | `INTERNAL` (an adapter bug, as elsewhere) |

Every 7-class `fix` names who can act (a person, with the console command or the approval id). Provider failures leave the session `interrupted` and resumable (arch §19).

---

## 16. Testing

### 16.1 Fake providers

- **`fake` harness (in-process, TS):** a scripted provider (`{on: turnStarted, emit: [...]}` scripts with waits, tool calls, deltas, usage, requests, crashes, late events from a replaced instance). It tests the runtime: the state machine, persistence, resume, budgets, approvals, attribution, verdicts and journal emission.
- **Fake model server (loopback HTTP, TS):** replays scripted SSE streams in the Anthropic Messages, OpenAI Responses and Chat Completions formats, including 429 with `retry-after`, 529, mid-stream disconnects, malformed events, tool calls split across deltas, cache usage fields. The built-in adapters run against it with real SDKs; tests never leave loopback (D-plan rule).
- **Fake CLIs (`fake-claude`, `fake-codex`, small Rust binaries in `crates/fin-agent-fakes`):** speak stream-JSON with the control protocol, and newline-delimited JSON-RPC with the app-server methods of arch §27.5, from a script file; they run under the real sandbox launcher, so 7C's policies are exercised by the same tests (`fin` calls from inside, writes outside the workspace).
- **Recorded transcripts:** per supported real CLI version, captured on a validation host by a controller task, scrubbed (no paths, user or host names; checked by the same grep as the D-plan's "never raw" test), committed as contract fixtures for the adapters' mapping. Only stream messages are recorded, never a provider's system prompt.

### 16.2 Layers

| Layer | Tests |
|---|---|
| `fin-core::events` | kinds v1 round trip; every content field classed; `AGENT_*` tables; catalogue ⊆ `AGENT_MODELS` |
| Runtime (vitest) | state machine properties (random event scripts never leave a session in an impossible state; resume after any crash point never re-runs a tool), budget ledger, approvals, rules × layers, attribution exact/inferred/ambiguous, verdict tracker against scripted commit outcomes |
| Adapters | fake model server and fake CLIs; recorded-transcript contract suites; the Codex schema drift gate |
| Sandbox | §8.10 escape suites on every OS; policy unit tests (enumeration around protected and denied roots, last-match profile order); ACL broker crash and concurrency tests (WAL replay, the S8 lost-grant race) |
| `fin` | `tcp:` endpoint, `FIN_CALL` and `FIN_CREDENTIAL`, new commands, the map budget |
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
| `chacha20poly1305` 0.11.0 | secrets file | Apache-2.0 OR MIT | use (7A, in `fin-node`) |

---

## 18. Crate and file map

| Place | 7A | Later |
|---|---|---|
| `fin-core::events` | agent kinds v1, `agent.approval`, `AGENT_ROLES`, `AGENT_TOOLS`, `AGENT_MODELS` growth | — |
| `apps/finisherd/src/agent/` | `session.ts`, `store.ts` (migration), `runtime.ts` (runs, events → nodes), `adapter.ts` (contract), `fake.ts`, `builtin/{loop.ts, tools.ts, anthropic.ts, openai.ts, compat.ts}`, `catalogue/{models.json, catalogue.ts}`, `budget.ts`, `credentials.ts` (mint, branch reach), `journal.ts` (emission, attribution), `verdicts.ts`, `rpc.ts`, `console.ts` | `roles/`, `rules.ts`, `approvals.ts`, `subagents.ts` (7B); `sandbox.ts` (7C); `claude/` (7D); `codex/` (7E); `hooks/` (7F); `workspace.ts` (7G) |
| `apps/finisherd/src/secrets/` | `store.ts`, `gateway.ts` | keychain backend (with 5B) |
| `@finisher/contracts` | `agent.ts` (RPCs, node and event schemas), scopes | `registry.json` (7B) |
| `fin-node` | `secrets.rs` (file encryption) | `acl_broker/` (7C), pipe bridge (7C) |
| `crates/fin-sandbox` (new) | — | `policy`, `macos`, `linux`, `windows`, launcher binary (7C) |
| `crates/fin` | `FIN_CALL`/`FIN_CREDENTIAL` passthrough checks, `--idem-key` from `FIN_CALL` | `tcp:` endpoint (7C), `agent`, `approval`, `workspace`, `apply`, `tx`, `context`, `guide`, `note`, `budget`, `mcp`, `credential`, `schema --registry` (7B, 7G) |
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
| 8 | Rollback undoes the session's transactions after the turn; fork branches at the turn's head | §5.3 |
| 9 | `plan` may write its own workspace | §6.1 |
| 10 | Provider-native subagents are disabled; subagents are Finisher sessions on child branches | §6.3 |
| 11 | Command rules are enforced on the server against an exported, drift-gated registry | §7.2 |
| 12 | Approvals never block an RPC: `APPROVAL_REQUIRED` + `fin approval wait` | §7.3 |
| 13 | Linux sandbox = Landlock (ABI ≥ 3) + seccomp refusing AF_UNIX sockets; `fin` reaches the server over a credentialed loopback listener; no bubblewrap | §8.3 |
| 14 | CLIs run with Finisher-only config dirs; the user's own CLI configs and settings are neither loaded nor writable | §4.5, §8.6 |
| 15 | The built-in agent needs no OS sandbox; CLI agents and hook commands require one, with a uid sandbox in Docker | §8.9 |
| 16 | Windows shell for CLI agents is brokered (S8 follow-up) | §4.5 |
| 17 | Agent kinds become v1 with harness, named roles, `plan|draft|auto`, tool kinds, attribution, capture fidelity, system/tools as deduplicated blobs; `agent.approval` added | §12.2 |
| 18 | Verdicts come from a tracker over commit outcomes and review events | §12.5 |
| 19 | Budgets interrupt at a safe point and open an extension approval | §13.1 |
| 20 | The eval harness scores branch state, reference similarity (word keep F1, cut F1) and process; judges never gate | §14 |

---

## 20. Owner questions

1. **Which model outputs may become training targets?** Anthropic's commercial terms forbid using its services "to build a competing product or service, including to train competing AI models", and its help centre lists using outputs as training targets for models as an example; OpenAI's terms carry a similar clause. An open-weight video-editing model may or may not count as "competing". Options: (a) record everything under the training opt-in, and have the dataset builder use closed-model outputs only as context and never as targets, taking targets from what people kept (edits that were merged, accepted or not undone, and people's own edits) and from open-weight model outputs; (b) record closed-model outputs only as metadata; (c) treat them as targets. **Recommendation: (a), with counsel before the first upload or training run.** Trade-off: (a) keeps the most signal while the labels (people's verdicts) are the part that is clearly the user's; (b) loses the instruction context the builder joins on; (c) risks the provider accounts the product depends on.
2. **How exact must CLI agents' LLM traffic be?** The CLIs' streams lack their system prompts and tool definitions (§12.6). Options: (a) record the stream (messages, tool calls and results, thinking and reasoning summaries where emitted); (b) route each CLI through a local recording proxy (`ANTHROPIC_BASE_URL`, a Codex model-provider base URL) that sees the exact requests. **Recommendation: (a) in v1**, with exact capture coming from the built-in agent. Trade-off: (b) captures everything but puts Finisher in the path of the user's subscription traffic and credentials, records the vendors' proprietary prompts, may break with any CLI update and may conflict with subscription terms; (a) loses the hidden prompt, which a fine-tuned model would not reuse anyway.
3. **What runs where no OS sandbox is available?** Landlock ABI ≥ 3 needs kernel 6.2+: Ubuntu 22.04's original kernel (5.15, inside D31's floor) and Debian 12 (6.1) fall short, and many NAS kernels have no Landlock at all. Options: (a) the built-in agent everywhere; CLI agents and hook commands only with a sandbox (or the uid sandbox in Docker), else `SANDBOX_UNAVAILABLE`, plus an audited owner override to run them unsandboxed; (b) run CLI agents anyway with a warning; (c) raise D31's Linux floor to kernel 6.2+. **Recommendation: (a).** Trade-off: some NAS and older-Linux users cannot use Codex or Claude Code without the override, against an injected agent being able to rewrite `.finisher/` and read server keys on exactly the always-on machines that hold the most media.
4. **May `auto` mode merge into `main` without a person?** Options: (a) never in v1: every agent edit reaches protected branches through review or a person's merge; (b) an owner-enabled policy per project lets `auto` merge when the QA gate and the critic pass. **Recommendation: (a).** Trade-off: "cook me five shorts" ends with five reviews to approve rather than five finished edits on `main`; (b) removes the click but makes a fooled critic the last line before the user's main timeline. Revisit with sub-project 10's data.
5. **Network egress for agents in v1.** An injected agent can upload project media anywhere: the sandbox keeps secrets and server data out of reach, not footage. Options: (a) open egress in v1, documented; (b) an egress proxy in `finisherd` with an allow-list (the provider's API, user-approved domains, downloads through `fin`), enforced by Seatbelt rules on macOS, Landlock TCP rules (ABI ≥ 4) on Linux and AppContainer capabilities on Windows. **Recommendation: (a) for v1, (b) after v1.** Trade-off: (b) closes the largest remaining injected-agent risk but costs a proxy that must understand each CLI's traffic (WebSockets, streaming, certificate pinning), Windows loopback exemptions that need elevation, and a raised Linux floor.

---

## 21. Contradictions and gaps found in earlier documents

1. **The reserved `agent.session` payload** (`role: main|subagent|critic|other`, `mode: interactive|headless|other`) does not match arch §8.3's named roles and `plan|draft|auto` modes, and has no harness: Codex and the built-in agent on an OpenAI key both say `provider: openai`. Resolved by §12.2.
2. **`agent.toolCall.command` is a `fin` command name**, but CLI agents mostly call their own tools (shell, edit, web fetch), which the reserved payload cannot express. Resolved by `tool` + optional `command` (§12.2).
3. **"System prompt and tool definitions by hash"** (arch §21.1) keeps no way back to the content, so training would lose them. Resolved by deduplicated blobs (§12.2).
4. **`FIN_CALL` "set by the agent harness for one `fin` run"** (D-plan Decision 23a) is impossible for Claude Code's and Codex's own shells. Resolved by exact-where-Finisher-executes plus inferred attribution (§12.3).
5. **Arch §8.4's "Landlock (bwrap fallback)":** bubblewrap is LGPL-2.0-or-later and cannot ship (AGENTS.md), and needs user namespaces that Ubuntu's AppArmor restricts (arch §27.8). Resolved by §8.3 and §8.9.
6. **Arch §27.6 calls Landlock "confirmed"**, but its escapes covered files only. Landlock does not mediate pathname-socket `connect()`, so D-Bus (Secret Service), `ssh-agent`, `docker.sock` and X11 were reachable. Resolved by seccomp and the agent listener (§8.3), and new escape cases (§8.10).
7. **Arch §27.6 makes `~/.claude` and `~/.codex` writable**, which lets an agent persist hooks that later run unsandboxed in the user's own terminal. Resolved by §8.6.
8. **D31 admits Ubuntu 22.04 (kernel 5.15)**, below the Landlock ABI that controls truncation. Owner question 3.
9. **Arch §8.4's env allow-list** names only `FIN_SERVER` and `FIN_SESSION`; `FIN_CALL` (D-plan), `FIN_CREDENTIAL` (sp5) and the provider directory variables are also needed, and `FIN_SERVER` cannot name a socket inside server data the sandbox denies. Resolved by §8.5.
10. **Arch §8.4 "a per-session 0600 credential file inside the session sandbox"** suggests `workspace/`, which is in the project folder and may be synced. Resolved by `<server-data>/agents/<sessionId>/` (§8.5).
11. **Arch §8.1 "Session = one agent on one branch"** vs `fin try` sub-branches, critic fix branches and subagent children. Resolved as a branch subtree (§5.1).
12. **Arch §4 rule 6 enforces rules "in the command registry, whatever the transport"**, but the registry lives in `fin`, and an agent can call RPCs directly. Resolved by the exported registry and server-side `commandOf` (§7.2).
13. **Arch §5 and §16 keep keys inside `finisherd`**, but Claude Code using an API key must hold it. Resolved by grants through `apiKeyHelper`, stated to the user (§10).
14. **Arch §8.2 lets a user select a newer `codex`**, while the vendored client is pinned. Resolved by the generated-schema compatibility check (§4.6).
15. **Arch §8.3 `auto` "runs the critic"**: the critic's rubrics and QA are sub-project 10, and render is sp6 6E. `auto` is partial until both land (§6.1).
16. **Arch §21.1 phasing puts human-only operations in sub-project 7**; sp5 moved them to 5A. What remains for 7 is making them hold (§7.5).
17. **`AGENT_MODELS` holds only Claude models**, so every Codex or OpenAI session would be `other`. Resolved by the catalogue drift test (§4.7).
18. **Arch §8.4's provider config directories and per-session home** conflict with Claude Code keeping its macOS login in the Keychain; whether a separate config dir separates the Keychain item is unverified (§8.6, 7D).

---

## 22. Risks

| Risk | Mitigation |
|---|---|
| CLI protocol drift (Claude Code ships often; stream-JSON and the control protocol are not versioned contracts) | version gate; recorded-transcript contract suites per version; `PROVIDER_PROTOCOL` rather than guessing; the built-in agent as fallback |
| Apple removes `sandbox-exec` | the profile generator is independent of the launcher; `sandbox_init` from the launcher as a second path; tracked per macOS beta |
| seccomp's AF_UNIX refusal breaks a CLI or a tool it runs | 7C validates real CLIs on the Linux VM; a narrow allow-list (`socketpair` only) and a test per CLI version; fallback: Landlock ABI 6 scoping plus the env clearing, with the gap recorded |
| Nested sandboxes (Codex's own Seatbelt/Landlock inside ours) refuse to apply | 7E measures it; Codex's sandbox off inside ours where nesting fails; ours is the guarantee either way |
| `CLAUDE_CONFIG_DIR` does not isolate the macOS Keychain login | 7D verifies; fall back to the user's existing login read-only, which the sandbox cannot write |
| Inferred attribution mislabels commits under parallel shell calls | `ambiguous` instead of a guess; `fin mcp` makes it exact; the builder can drop non-exact joins |
| Training capture grows the journal fast (tool outputs, images) | blobs deduplicated by content; 16 KiB inline cap; the journal's size cap (D34) evicts oldest first; product state is separate |
| Budgets overrun between checks (a long tool call, a big turn) | checks per turn and per tool call; `--max-budget-usd` on Claude Code; per-day money cap |
| Provider terms and training use | Owner question 1; provenance on every turn so the builder can filter |
| 7A is large (runtime, three APIs, budgets, secrets) | the plan's tasks are independent behind the adapter contract; the fake provider lets the runtime merge before the APIs |
