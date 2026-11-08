# Sub-project 7B: Subagents, roles, rules and approvals — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (rules and their layers, approvals and one-time grants, subagent reach and budgets, rollback, compaction, the auto-merge trigger), `sonnet` for plumbing (the registry export, roles as data, saved prompts, `fin` commands, console, e2e, docs). Task 14 is controller-run.

**Goal:** agents compose and are governed. After 7B:
- **a running session can start child sessions** (`fin agent spawn|wait|ls|send|stop`) on child branches with their own credential, mode (at most the parent's), role and a **budget carved from the parent's**, up to depth 3 and 8 live descendants; the parent merges a child's branch into its own; provider-native subagents stay disabled (spec §6.3);
- **roles are complete data** (`apps/finisherd/src/agent/roles/*.json` with rules, tools, per-harness models and skills) and **saved prompts** expand into a start and a first prompt, for people only (§6.2, §6.4);
- **command rules** (`allow|ask|deny` on command globs, layered managed > project > user > local > role) are enforced **in `finisherd`'s chain** against an exported, drift-gated registry, whatever the transport (§7.2);
- **approvals** are durable, answerable from any device or the console, never block an RPC (`APPROVAL_PENDING`, exit 8; `fin approval wait` ≤ 90 s, repeatable), grant once and exactly (`(sessionId, command, argsHash)`), and `remember` adds a visible, revocable rule (§7.3); provider permission requests are routed through the same path (§7.4);
- **rollback** to a run undoes the session's later transactions (§5.3); the built-in loop **compacts** its context instead of stopping at 85 % (§4.4);
- with 5E: sessions **request reviews**, critics work on child branches, and the **`auto` merge policy's session side** calls 5E's internal merge path (which stays `AUTO_MERGE_UNAVAILABLE` until sub-project 10, §6.5).

**What 7A built that this plan starts from** (line numbers at the 7A-2 tip `sp7a2-t13`, `772b142`; 7A-2 is not merged into the integration branch at the time of writing):
- **Scopes and kinds:** `agent.read|run|approve|admin` (`packages/contracts/src/scopes.ts:37-40`; `agent.approve` and `agent.admin` in `HUMAN_ONLY`, `:47-60`, whose comment at `:56-57` already says 7B lets a parent drive its children). The session RPCs carry `kinds: SESSION_DRIVERS = ["local","console","device"]` (`:158`, rules `:291-313`), checked at stage 0a of `authorize()` (`apps/finisherd/src/chain/authorize.ts:40-50`, `SCOPE_DENIED {reason: principalKind}` from `chain/fixes.ts:44-53`). `agent.self`/`agent.self.try` are `SESSION_OWN = ["agent"]` (`:160`). `MODE_SCOPES` (`agent/credentials.ts:43-47`) gives no mode `agent.run` (deferred to 7B at `:36-38`).
- **Approvals:** the `approvals` table (`control/migrations/agent/0001.ts:58-63`: `id, sessionId, kind, status, subject, argsHash, requestedAt, expiresAt, decision, by, answeredAt, consumedAt`; indexes `approvals_session`, `approvals_open`), `sessions.approvals` (open ids, `:29`), `AgentStore.openApproval|answerApproval|expireOverdueApprovals|consumeApproval` (`agent/store.ts:318-370`, `APPROVAL_EXPIRED` past `expiresAt`), contracts `ApprovalKind = rule|budget|provider|plugin`, `RememberScope = session|project`, `AgentApprovalAnswerPayload {id, decision, remember?, add?, challenge}` (`packages/contracts/src/agent.ts:132-163, 290-298`). **`agent.approval.answer` refuses `remember`** (`agent/rpc.ts:143-145`: `INVALID_REQUEST {field: remember, reason: rulesIn7B}`); `Runtime.approve` (`agent/runtime.ts:342-376`) acts on `budget` approvals only; the only writer is the budget stop (`agent/runtime-engine.ts:524-543`); **provider `request` events are auto-denied** (`runtime-engine.ts:389-395`, `reason: approvalsUnavailable`).
- **Approval quotas** live in the chain, not the agent table: 5A's in-memory `ask` map (`chain/chain.ts:90-113`: `MAX_APPROVALS = 256`, `APPROVAL_TTL_MS = 10 min`, `MAX_APPROVALS_PER_PRINCIPAL = 16`, oldest dropped past it; `keepApproval()`), whose `ask` answers `APPROVAL_REQUIRED` (`chain/fixes.ts:84-91`), which `fin` maps to exit 7 (`crates/fin/src/output.rs:34`). Exit 8 exists (`PENDING = 8`, `PENDING_CODES = ["APPROVAL_PENDING"]`, `output.rs:18,69,444`; console mirror `console/exit-codes.ts:15,73`) but nothing emits it. There is no `fin approval` group.
- **`session_txs`** (`0001.ts:50-54`, `projectId` from `0002.ts`, `rootBranch` and `session_tx_outcomes {seq, txId, via, verdict, by, sessionId, at, emitted}` unique on `(txId, via)` from `0003.ts`), written by `agentChainHooks().after` (`agent/reach.ts:264-280`) through `AgentStore.recordTx` → `SessionTxs.record` (`agent/store-tables.ts:96`, also `of`, `setOutcome`, `bySession`, `byCall`). **The outcome tracker** (`agent/outcomes.ts`, `OutcomeTracker.observeHistory|observeBranch|observeSessionDeleted`) records `merged`/`undone`/`reverted`/`abandoned` (and `via: critic` for a critic session); `OutcomeVia` has `autoMerge` and `OutcomeBy` has `policy` (`agent/store-outcomes.ts:11-16`), unwired; it emits `review.verdict` only once `kindVersion` reports it for source `agent` (`outcomes.ts:183`), which 5E's Task 4 provides.
- **Roles:** `agent/roles/{editor,critic,fixer,clip-finder,caption-stylist,colourist,mixer}.json` with `{name, title, prompt, modeCeiling, defaultMode, models: {builtin}, budget}` (editor ceiling `auto`, the rest `draft`, critic default `plan`), loaded by `agent/roles.ts` (`Role`, `MODE_RANK`, `checkRole`, `loadRoles`, `BUNDLED_ROLES`, `budgetFor`); no `rules` or `tools` yet. `AGENT_ROLES` in `crates/fin-core/src/events/agent_names.rs:226-238`.
- **Sessions:** `sessions.parent` exists (`0001.ts:28`; `NewSession.parent?` `agent/store-rows.ts:27`, `Session.parent?` contracts `:77`; the journal's `trigger: subagent` at `agent/journal.ts:220`); no depth column; `AgentSessionStartPayload` (`contracts agent.ts:214-227`) has no `parent`. `OpenSession.root?` with `claimRoot` → `BRANCH_IN_USE` (`agent/credentials.ts:303-312`) is "for 7B's child branches"; `openSession` (`:333`) creates `agent.<role>-<n>` from `base`; `mintForSession(s, root, by)` (`:120`) mints through 5A's `Credentials.mintWithId` (`apps/finisherd/src/auth/credentials.ts:197`) with `branches: [root ?? s.base.branch]` and the mode's scopes intersected with the starter's. `BranchReach.allowed` reaches the subtree of `session.rootBranch` (`agent/reach.ts`). `Runtime` entry points (`agent/runtime.ts`): `start :147`, `prompt :188`, `promptIn :195`, `steer :248`, `interrupt :271`, `resume :290`, `end :303`, `delete :324`, `approve :342`, `watch :385`. **No per-server session limits are implemented** (spec §5.5's 4 provider processes and 8 built-in sessions).
- **Budgets:** `budgets(sessionId, kind, limit, used, reserved)` and `budget_days` (`0001.ts:64-68`); `Ledger.reserved` documented as "in 7B a child's carve-out" (`agent/ledger.ts:13-17`); per-call reservations only (`agent/budget.ts:138-180`, `raise :214`; `DbLedger` `store-tables.ts:186`).
- **Rules and the chain:** no rules code, no `fin schema --registry`, no `packages/contracts/registry.json`; `RULE_DENIED`, `MODE_FORBIDS` and `DEPTH_EXCEEDED` exist only as codes (`output.rs:59-62`). The chain is `withChain()` (`chain/chain.ts:169`; stages listed at `:1-13`: decode, authenticate, resolve, authorise, policy (`allow|deny|ask`) and console confirmation, handler, after); stage 5 is `runPolicies` over `PolicyRule`s (`chain/policy.ts:12,95`, its comment at `:2-4` reserving "sub-project 7 command rules"), wired in `apps/finisherd/src/handlers.ts:124` as `[reach.policy, agentBranchLimitsExcept(...)]`; `ChainHooks.authorize` (stage 4, `chain/hooks.ts`) is `agentChainHooks()` (`agent/reach.ts:254`), which already refuses `SESSION_REFUSED_RPCS` (`:228`). An argv-level `commandOf` exists for the journal (`agent/journal.ts:87`); `fin`'s registry is `crates/fin/src/registry.rs` (`COMMANDS :73`) with `registry_edit.rs`, `registry_plugins.rs`, `registry_settings.rs`; `fin schema commands` emits `argKinds` (`crates/fin/src/commands/schema.rs:18-23`).
- **Rollback and context:** `runs.headAtStart` is filled (`agent/runtime.ts:203,224`); `Capabilities.rollback` and `ProviderSession.rollback(toTurn)` are capability-gated (`agent/adapter.ts:13,110-111,142`); the built-in loop's `rollback` fails `PROVIDER_UNAVAILABLE` "rolls back in 7B" (`agent/builtin/loop.ts:393`); the fake supports it (`agent/fake.ts:192`). The context stop is in `runTurn` (`loop.ts:283-286`, `contextRatio: 0.85` at `:59`, `promptTokens` at `:143`), a run-only stop (`RUN_ONLY_STOPS`, `runtime-engine.ts:65`); history is rebuilt by `historyFromNodes` (`builtin/loop-history.ts:26`).
- **Console:** `finisherd agent start|ls|list|show|prompt|steer|interrupt|resume|end|watch|approve|delete|providers` (`agent/console.ts:17`) and `mint|credentials|revoke` (`console/commands/agent.ts:27-30`).

**Architecture:**
- **`crates/fin`:** `fin schema --registry` (the export), the `agent` group (`spawn`, `wait`, `ls`, `send`, `stop`) and the `approval` group (`wait`, `ls`), all in the registry with `argKinds`; the map stays under 1.5k tokens.
- **`@finisher/contracts`:** `registry.json` (generated, drift-gated), `rules.ts` (rule schema), the new RPCs in `agent.ts` (or `agent-rpcs.ts` if `agent.ts` would pass 600 lines), `NodeKind` gains `summary`.
- **`apps/finisherd/src/agent/`:** `command-of.ts` (RPC + payload → command name), `rules.ts` (layers, matching, the stage-5 `PolicyRule`), `rule-store.ts` (the `agent` migration 0004 tables), `approvals.ts` (durable asks, one-time grants, waits, quotas), `permissions.ts` (provider permission requests, §7.4), `subagents.ts` (spawn, wait, limits, cascade), `carve.ts` (child budgets), `rollback.ts`, `builtin/compact.ts`, `saved-prompts.ts` and `prompts/*.json`, `review-link.ts` and `auto-merge.ts` (7B-2, after 5E).
- **`fin-core::events`:** `agent.turn` gains the optional meta `purpose: compaction` (additive in v1); `agent.session.trigger` already has `subagent` and `savedPrompt`; `RPC_METHODS` gains the new RPCs.

**Tech Stack:** as 7A: Node 24, Effect 4.0.6, `node:sqlite` through 5A's `ControlStore`, vitest, fast-check; Rust 2024, clap, `schemars` 1. `globset`-style matching is written in TS (a 60-line glob over command names; no new dependency). No other new dependencies.

**Spec:** `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` §1.4 (7B row), §1.7 items 4–6, §3.5, §4.4 (compaction), §5.3, §5.4, §6 (all), §7 (all), §9.4 (project trust, for project rules), §11.2 (`fin agent`, `fin approval wait`), §12.2 (`agent.approval`), §12.5, §13.1 (carve-outs), §15, §16.2, §19 decisions 8, 10–12, 29, §20 owner decision 4. Cross-design: sp5 §4.4 (the chain), §8.1 and §8.5 (protected branches, `branch_policy.autoMerge`), §3.4 (`review.verdict`); the 5E plan (`home/plans-5b-5e`: `docs/superpowers/plans/2026-11-08-sub-project-5e-review-workflow.md`, Decisions 12, 18–20, Task 9 `tryAutoMerge`, Task 10 `fin review`); sp8 §3.8 (plugin command names), sp8 line on prompts and roles as plugin content.

**Batches and branches:**

| Batch | Tasks | Needs | Branch |
|---|---|---|---|
| **7B-1** Rules, approvals, roles, subagents, rollback, compaction, saved prompts | 1–10 | **7A-2 merged** (it is on `sp7a2-*` today); 5A | `sp7b1-*`, stacked on the integration branch after 7A-2 lands |
| **7B-2** Reviews and the `auto` merge trigger | 11, 12 | 7B-1; **5E merged** (`review.request`, `review.verdict` v1, `branch_policy`, `tryAutoMerge`) | `sp7b2-*` |
| Close | 13, 14 | all | — |

Task 1 (the registry export) and Task 3 (roles as data) touch nothing of the runtime's state and may start on the 7A-2 branch before it merges. 7B-1 does not wait for 7C: rules and approvals are server-side; the sandbox is a separate layer (spec §7.1).

**Out of scope:** the Claude Code and Codex mapping of permission requests and of `rollback` (7D, 7E: they call `permissions.ts` and implement `ProviderSession.rollback`), hooks and `BranchReady`/`PreMerge` handlers (7F: 7B emits the internal `branchReady` event and a no-op hook point), project trust for **hooks** (7F extends the trust record 7B creates for rules), `fin context`, `fin budget` and the rest of the full `fin` (7G), the QA gate, rubrics and critic scoring (sub-project 10), the UI's approval cards and session tree (9), plugin-contributed roles and saved prompts (8 later).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL dependency in shipped code (`pnpm gates`). **Role prompts and skills borrowed from Uncut (D26) need their licence recorded before they ship** (Question 1).
- **No cloud-only imports.**
- **Model and event types are generated, never hand-written:** the `agent.turn` change goes through `crates/fin-store/examples/export_schema.rs` and `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`); `registry.json` is generated by `fin schema --registry` and drift-gated the same way. Control-plane state (`Rule`, `SavedPrompt`, the RPC payloads) are Effect Schemas in `@finisher/contracts`, as 7A's are.
- **Primitive ops live only in `fin-core`:** rollback and compaction add none; rollback uses 4B's undo through `fin-node`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. `agent/rpc.ts` (12 KB), `agent/runtime.ts` (17 KB) and `agent/runtime-engine.ts` (27 KB) take call-site lines only; new behaviour goes in new files.
- **Agents never decide for people:** no agent credential gains `agent.approve`, `agent.admin`, rule writes, `branch.protected`, `review.decide` or anything human-only in any mode; a parent's `agent.run` reaches **its own children only**.
- Errors are `{code, message, details, fix?}`; new codes go into `crates/fin/src/output.rs`, `apps/finisherd/src/errors.ts` and `console/exit-codes.ts`.

**Testing and process**
- Real engine and real `control.db` in tests (temporary server data), the scripted `fake` harness for sessions, the fake model server for the built-in loop's compaction; clocks and ids injected.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks that touch `fin` or RPC wiring also run `rtk pnpm e2e`.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **The repo is public:** no host names, user names or key names in tracked files.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 7B: subagents, roles, rules, approvals", in the same commit.

## Decisions taken here (the spec fixes the shape; these fix the details)

### The registry and `commandOf`

1. **`fin schema --registry`** writes `{version, commands: [{name, group, rpcs: [{method, match?: {field: value}}], identity: [field…], write: bool, argKinds}]}` from `crates/fin/src/registry*.rs`, including plugin commands as `plugin:<id>/<cmd>` from `registry_plugins.rs` (sp8 §3.8). The output is committed as `packages/contracts/registry.json`; `scripts/check-registry.mjs` (a gate) regenerates and diffs it, like the generated types.
2. **`commandOf(method, payload)`** (`agent/command-of.ts`) maps an incoming RPC back to the command name through `registry.json`'s `rpcs[].match`; an RPC that several commands share is disambiguated by the `match` fields (for example `history.undo` with `to` is `undo --to`); an RPC no command names maps to `rpc:<method>`, so rules can still name it. A drift test asserts every RPC tag of `@finisher/contracts` is either named by a command or listed in an explicit `RPC_ONLY` set. The journal's argv-level `commandOf` (`agent/journal.ts:87`) moves next to it and keeps its behaviour.

### Rules

3. **Rule schema** (`packages/contracts/src/rules.ts`): `{id, effect: allow|ask|deny, command: <glob>, where?: {branch?, mode?, preset?, bytesOver?, costOver?, minutesOver?}, layer, source, createdBy?, createdAt}`. Globs match command names segment-wise (`*` within a segment, `**` across, `plugin:@acme/*`, `render`, `asset.add`); a `where.branch` is a branch reference resolved per project.
4. **Layers and order** (spec §7.2): `managed` (`<server-data>/managed-settings.json`, read-only to the server) > `project` (`finisher.rules.json` beside `finisher.project.json`, honoured only while the project is trusted, Decision 6) > `user` (`agents.rules` in `<server-data>/settings.json`) > `local` (`control.db` `agent_rules {projectId?}`: per project or server-wide, where `remember` writes) > role (`roles/*.json` `rules`). Evaluation collects every matching rule of every layer; **the first `deny` in layer order wins, then the first `ask`, then any `allow`; no match is `allow` within the scopes.** A role may add `deny` and `ask` but its `allow` never overrides another layer's `ask` or `deny` (a role narrows, never widens, spec §6.2).
5. **Where rules run:** a `PolicyRule` (`agent/rules.ts` `commandRules`) appended to the policy list in `handlers.ts:124`, **for `agent` principals only** (sp5 §4.4 gives people their own confirmation path). It sees the resolved request (stage 5), so `where` conditions use resolved values (the branch id, the bytes of an `asset.add`, a render's minutes). `deny` → `RULE_DENIED {rule, layer}` (exit 7); `ask` → Decision 9. **Defaults** (the role layer of every bundled role, spec §7.2): `deny plugin.install`, `ask render where minutesOver: 10, mode: draft`, `ask asset.add` of a download without a recorded licence (7G fills the condition; 7B ships the rule), `ask workspace.gc`.
6. **Project trust for rules** (spec §9.4): `control.db` `project_trust {projectId, rulesHash, hooksHash?, trustedAt, by}` (agent migration 0004); a project's `finisher.rules.json` is ignored (and `agent.session.get` says so) until a person trusts it with `finisherd project trust <id>` (human-only, typed challenge), and again after any change to the file's hash. 7F adds `hooksHash` to the same record.
7. **Rule administration is human-only:** `agent.rules.list` (`agent.read`), `agent.rules.add|remove` (`agent.admin`: owner, human-only; no new scope), console `finisherd agent rules ls|add|rm`. No agent credential can write any layer; the project file is the one layer an agent could edit (it lives in the project folder, read-only to agents under 7C), which is why it needs trust and a hash.

### Approvals

8. **Agent asks are durable; people's stay 5A's.** For an `agent` principal, an `ask` from stage 5 (a rule, Decision 5) does not enter 5A's in-memory map (`chain.ts:90-113`): `approvals.ts` writes an `approvals` row (`kind: rule`, `subject` = the resolved command and arguments written by the server, `argsHash` = the keyed hash of the resolved arguments as the card shows them, `expiresAt` = 30 min) and the call answers **`APPROVAL_PENDING {approval}` (exit 8)** instead of `APPROVAL_REQUIRED`. Other principals keep 5A's map and `APPROVAL_REQUIRED` (sp5's own confirmation flow). This is §1.7 item 4 implemented in the chain's `ask` branch (`chain.ts:211-215`) through a new `ChainDeps.agentAsk` port.
9. **One identical ask, one card.** A second request with the same `(sessionId, command, argsHash)` while one is open returns the open approval's id (no new row). **Quota:** at most 16 open approvals per session and 256 per server (5A's numbers); past either, the request is refused with **`APPROVAL_LIMIT {open}` (exit 4, new)** rather than dropping the oldest as 5A's map does, so an agent cannot expire a card a person is reading by flooding. The hourly sweep (`expireOverdueApprovals`) is unchanged.
10. **Grants are one-time and exact** (`(sessionId, command, argsHash)`, spec §7.3). The policy stage looks for an `allow` answer not yet consumed and **consumes it in `control.db` before the handler runs**, recording the call's idempotency key (`FIN_CALL` or the write's `--idem-key`); if the handler fails without committing, the grant is restored. *Deviation from the spec's "committed in the same transaction as the write":* the write commits in the project's store and the grant in `control.db`, two databases; consuming first means a crash between the two loses the grant (the agent asks again), never doubles a write. Recorded in the spec by Task 13.
11. **Waiting:** `agent.approval.wait {id, timeoutMs ≤ 90000}` (`agent.read`, kinds `agent` for the session's own approvals, and people) returns `allow` (exit 0), `deny` or `expired` (`APPROVAL_DENIED`, exit 7) or, on timeout, `APPROVAL_PENDING` (exit 8, repeatable). `fin approval wait <id> [--timeout <s>]` caps at 90 s; `fin approval ls` lists the session's own. The built-in loop waits on the approval itself (status `waitingApproval`) and hands the outcome to the model as the tool result.
12. **Answering:** `agent.approval.answer` (human-only, `agent.approve`, any device or the console) handles every kind: `budget` as today (`Runtime.approve`), `rule` and `provider` by recording the decision. **`remember: session|project` is accepted** for `rule` and `provider` kinds and adds a `local`-layer rule (`allow` or `deny` of exactly the approval's command, with `where` from its subject: the session id for `session`, the project for `project`), audited `agent.rule.add {from: approvalId}` and shown in `agent.rules.list` with its source; it is revocable like any rule and never a standing grant (spec §7.3). `remember` on a `budget` approval stays `INVALID_REQUEST`.
13. **Provider permission requests** (spec §7.4, `permissions.ts`): `decide(session, {tool, input}) → allow | deny | ask` replaces the auto-deny at `runtime-engine.ts:389-395`: **allow** what the sandbox contains (file tools inside the workspace, reads, shell commands; the 7C sandbox is the bound), **deny** what the mode forbids (writes outside `workspace/<session>/` in `plan`), **ask** what a rule marks by tool pattern (`ask "tool:WebFetch"`, `ask "tool:Bash(curl *)"`: tool rules use the `tool:` prefix in the same rule store). An `ask` opens a `provider` approval and the request waits (status `waitingApproval`); the answer goes back through `ProviderSession.respond`. Each decision is an `approval` node with `by: policy|user`. 7D/7E map their protocols onto it.

### Roles and saved prompts

14. **Role schema v1** (`agent/roles.ts`, Effect Schema): `{name, title, prompt: {text, skills: [topic…]}, modeCeiling, defaultMode, models: {builtin?, claudeCode?, codex?}, rules: Rule[] (role layer), tools: {allow: [tool…]} (the allow-set 7D builds `--disallowedTools` from, spec §4.5), budget, spawn?: {roles: [name…], maxChildren?}}`. All seven bundled roles get `rules` (the defaults of Decision 5), `tools`, and `spawn` (`editor` may spawn every other role; `clip-finder` may spawn `clip-finder`; `critic` may spawn `fixer`; others none). A role file that fails the schema fails the server's start (`ROLE_INVALID`), as `checkRole` does today.
15. **Saved prompts** (`agent/saved-prompts.ts`): `{name, title, role, mode, template, params: [{name, type: int|string|enum, values?, min?, max?, default?}], budget?}`. Bundled in `agent/prompts/*.json` (`make-shorts` first: `role: clip-finder`, `params: [{name: count, type: int, min: 1, max: 20, default: 5}]`) and owner-defined in `agents.savedPrompts` in `<server-data>/settings.json`; **not read from project folders in v1** (Question 3). Expansion substitutes typed parameters into `{{name}}` slots (no expressions, no code), validates every parameter (`AGENT_ARGS` with the field), and calls `Runtime.start` with `trigger: savedPrompt` and the expanded first prompt. **People only** (spec §6.4): `agent.savedPrompt.list` (`agent.read`), `agent.savedPrompt.run {projectId, name, params}` (`agent.run`, kinds `SESSION_DRIVERS`); console `finisherd agent run <name> [--project] [--param k=v]…`. `SAVED_PROMPT_UNKNOWN` (exit 3).

### Subagents

16. **Spawning is `agent.session.start` from an `agent` principal.** `kinds` on `agent.session.start|prompt|steer|interrupt|end` and the new `agent.session.wait` gain `"agent"`; a new stage-4 check in `agentChainHooks().authorize` admits an agent caller only for **its own children** (`sessions.parent = principal.id`) and, for `start`, makes the new session the caller's child (the payload carries no `parent`; an agent cannot attach a session elsewhere). `resume`, `delete`, `rollback` and every approval answer stay people's. `MODE_SCOPES.draft` and `.auto` gain `agent.run` (a credential minted before 7B keeps its scopes: an older session cannot spawn).
17. **A child** (spec §6.3): role must be in the parent role's `spawn.roles` (`ROLE_UNKNOWN` otherwise); mode ≤ the parent's mode and ≤ the role's ceiling (`MODE_INVALID`); harness the parent's unless given (7D/7E: a CLI parent may spawn a built-in child); its root branch is created from **the head of the parent's current default branch** (`session_state.defaultBranch`) through `openSession`, named `<parent root name>/<role>-<n>`; its credential reaches its own subtree only. **Depth ≤ 3 and ≤ 8 live descendants per root session** (`agents.subagents.maxDepth`, `.maxLive`), else `DEPTH_EXCEEDED {depth|live}` (exit 7). Agent migration 0004 adds `sessions.rootSession` and `sessions.depth` (backfilled `id`, 0).
18. **A parent's reach excludes its live children's subtrees.** A child's root is created under the parent's branch, so it lies in the parent's subtree; `BranchReach.allowed` therefore subtracts every subtree rooted at another **live** session's root (the existing one-live-session-per-root rule, made a reach rule). The parent reads a child's branch and merges it into its own (`fin merge --from <child>`, a read of the child's branch); once the child has ended, its branch is the parent's again (the parent may archive it). The outcome tracker keeps "merges inside the subtree are not outcomes" (`outcomes.ts`); the child's transactions are attributed to the child's session in `session_txs`.
19. **Live limits per server** (spec §5.5, not built in 7A): at most `agents.maxLiveProviders` (4) provider processes and `agents.maxLiveBuiltin` (8) built-in sessions in `running|waitingApproval` at once; a run that would exceed it waits in `starting`/`idle` with a `notice` node (`SESSION_QUEUED`), and starts when a slot frees, oldest first; children count like any session. No new error code: queuing never fails a call.
20. **Ending:** a child whose brief run ends with `end_turn` and no pending approval **ends itself** (`ended`, reason `briefDone`) and its final assistant message becomes its summary; the parent gets a `subagent` node `{child, status, branch, summary}`; nothing is injected into the parent's running turn (the parent asks with `fin agent wait`). Ending, interrupting or deleting a parent **interrupts** its live children (they stay resumable by a person); ending a root session with live descendants ends them after their current tool call. A child's interrupted state never blocks the parent's end.
21. **`fin agent`:** `spawn --role <r> --brief <text|@file> [--mode] [--budget <tokens|usd>] [--model]` → `{session, branch}` at once; `wait <id>… [--any] [--timeout <s> ≤ 90]` → each child's status, branch and summary, `WAIT_TIMEOUT` (exit 5, repeatable) when none is done; `ls` (the caller's children); `send <id> <text>` (prompt when idle, steer when running); `stop <id>` (end). The RPC behind `wait` is `agent.session.wait {ids, any, timeoutMs}`.

### Budgets for children

22. **Carve-outs** (spec §13.1, `agent/carve.ts`): a child's limits are reserved from the parent's remaining budget per kind (`budgets.reserved` of the parent += carve), in one `ControlStore.tx` with the child's creation; the child's own `budgets` rows hold the carve as its limits. On the child's end, the parent's `reserved` drops by the carve and its `used` rises by what the child used (so the parent's remaining is what it had minus what the child really spent). A child cannot raise its own limits; a person's `budget` approval on a child raises the child's limit and the parent's `reserved` together, or fails `BUDGET_EXCEEDED` on the parent. **Default carve when `--budget` is not given: the child role's budget, capped at half the parent's remaining per kind** (Question 2). Money for the server's day is charged by real spend only (unchanged).

### Rollback and compaction

23. **Rollback** (`agent.session.rollback {sessionId, toRun}`, human-only in 7B: kinds `SESSION_DRIVERS`; `agent/rollback.ts`): refused while the session runs (`SESSION_BUSY`) or has a live child (`SESSION_BUSY {reason: liveChildren}`). It takes the session's `session_txs` after run *r* started, per branch of the subtree, newest first, and undoes them as 4B undo units under the session's author, one engine call per branch, with the write guard; on a `CONFLICT` on any branch, the branches already undone are redone (4B redo), and the call answers `CONFLICT` with both sides as they were (spec §5.3). Then the provider rolls back (`ProviderSession.rollback(turn)` where the capability exists) or, for the built-in loop, the later runs' nodes are marked `rolledBack` and left out of the next request (`historyFromNodes`); the nodes stay for display and audit. Outcomes: the tracker records `undone` with `by: user`, `via: undo` for each (it already does for undos; rollback adds `details.rollback`). `fin` gets no rollback command (people only); the console gets `finisherd agent rollback <id> --to-run <seq>`.
24. **Compaction** (`builtin/compact.ts`): before a model call whose prompt would pass **75 %** of the window (`limits.compactRatio`, below the 85 % stop at `loop.ts:283-286`, which stays as the backstop), the loop makes one summary call with a fixed, versioned compaction prompt over the runs before the current one, then replaces those runs' messages in later requests with one user-role message holding the summary. The summary is a `summary` node (new `NodeKind`), its model call is an ordinary `agent.turn` with `purpose: compaction` (charged to the session like any call), and the next turn's trace-store delta starts over (`from: 0`, as 7A's delta rule already does after a turn it did not record), so a replay never splices a compacted history onto an uncompacted one. A compaction that itself would not fit, or fails, ends the run with `MODEL_CONTEXT_EXCEEDED` as today. The system prompt and tools stay byte-identical (the cache holds).

### Reviews and the `auto` merge trigger (7B-2, after 5E)

25. **Review requests from sessions:** `MODE_SCOPES.draft` and `.auto` gain `review.write`; `fin review request` (5E Task 10) from a session records the review id on the session (`session_state.reviews`), emits the internal `branchReady {sessionId, branch, headSeq, reviewId}` event (7F attaches `BranchReady` hooks to it), and the session's run ends with a summary node. A session that ends with unmerged commits and no review emits `branchReady` too (spec §9.1).
26. **Critics** (spec §6.2): starting a `critic` session with `base` = a branch under review creates its root as `<reviewed>/critic-<n>` (a child **branch** of the reviewed one, through `openSession {root}` with `claimRoot`, not a child session) so its reach excludes the reviewed branch; its fixes, and a spawned `fixer`'s, are recorded by the tracker with `by: critic`, `via: critic` (already built). A critic's verdict for sub-project 10 is out of scope; 7B records which head seq the critic session started from (`sessions.base.seq`) so the policy's "same head" check has it.
27. **The `auto` merge trigger** (`agent/auto-merge.ts`; spec §6.5): on `branchReady` of a session in `auto` mode whose role `branch_policy.autoMerge.roles` admits, the trigger checks the session side — **no open approval of the session, within budget, the critic session distinct from the authoring session and not one of its descendants (`rootSession` differs), the critic's base seq equal to the branch head** — and only then calls 5E's `tryAutoMerge(itemId, sessionId, deps)` (the 5E plan's Task 9; it checks the policy, `requireReview`, the QA and critic verdicts and the snapshot, merges as the `policy` actor and audits). Any failed check, or 5E's refusal, leaves the review item open for a person. 7F's `PreMerge` hook joins the session-side checks when 7F lands (a hook point that allows today). Until sub-project 10 provides a `VerdictSource` and flips `AUTO_MERGE_AVAILABLE`, the trigger runs and nothing merges; the tests inject both.

## Frozen interfaces

```ts
// apps/finisherd/src/agent/command-of.ts
export function commandOf(method: FinRpcTag, payload: unknown): string;            // "item.trim", "plugin:@acme/tools/x", "rpc:<method>"
// apps/finisherd/src/agent/rules.ts
export interface RuleSet { evaluate(input: { command: string; principal: Principal; session: Session; resolved: Resolved }): RuleOutcome }
export type RuleOutcome = { effect: "allow" } | { effect: "deny"; rule: Rule; layer: Layer } | { effect: "ask"; rule: Rule; layer: Layer };
export function commandRules(rules: RuleSource, approvals: Approvals): PolicyRule;  // appended in handlers.ts
// apps/finisherd/src/agent/approvals.ts
export interface Approvals {
  ask(s: Session, kind: "rule" | "provider", subject: Subject, argsHash: string): Effect<{ id: string }, FinError>;   // APPROVAL_LIMIT
  consumeGrant(sessionId: string, command: string, argsHash: string, idem: string): Effect<boolean>;
  restoreGrant(sessionId: string, idem: string): Effect<void>;
  wait(id: string, timeoutMs: number): Effect<"allow" | "deny" | "expired" | "pending", FinError>;
  answer(id: string, decision: "allow" | "deny", by: Principal, remember?: RememberScope): Effect<void, FinError>;
}
// apps/finisherd/src/agent/permissions.ts (7D/7E call it)
export function decideProviderRequest(s: Session, req: { tool: string; input: unknown }): Effect<"allow" | "deny" | { ask: string }, FinError>;
// apps/finisherd/src/agent/subagents.ts
export function spawnChild(parent: Session, req: SpawnRequest): Effect<{ session: string; branch: string }, FinError>;   // DEPTH_EXCEEDED, MODE_INVALID, ROLE_UNKNOWN
export function waitChildren(parent: Session, ids: readonly string[], any: boolean, timeoutMs: number): Effect<ReadonlyArray<ChildResult>, FinError>;  // WAIT_TIMEOUT
// apps/finisherd/src/agent/rollback.ts
export function rollback(s: Session, toRun: number, by: Principal): Effect<{ undone: readonly string[] }, FinError>;   // CONFLICT, SESSION_BUSY
```

## Review Focus

1. **No agent decides for a person:** no mode, role or child credential holds `agent.approve`, `agent.admin`, rule writes or human-only scopes; an agent's `agent.run` reaches only its own children. → Tasks 2, 5, 6 (`agent_cannot_answer_or_remember`, `agent_run_only_for_own_children`).
2. **Rules hold whatever the transport:** a rule on `render` holds through `fin`, a hand-written RPC client and the built-in tool; `registry.json` is drift-gated; plugin command names match. → Tasks 1, 4.
3. **Pending is not denied, and grants are exact and once:** exit 8 for pending, 7 for denied; a grant is consumed once, never by another session or other arguments, never doubles a write after a crash. → Task 5.
4. **Children are bounded:** depth, live count, mode, role, branch reach (parent and child), budget carve-outs that return what is unused. → Tasks 6, 7.
5. **Rollback is all or nothing** across the subtree's branches. → Task 8.
6. **Compaction never corrupts the training record:** the delta restarts; `bodyHash` replay holds. → Task 9.
7. **The auto-merge trigger can never merge by itself:** every check is session-side or 5E's; no RPC reaches it. → Task 12.

---

### Task 1: The exported registry and `commandOf` — tier: sonnet — 7B-1 (may start on the 7A-2 branch)

**Files:**
- Create: `crates/fin/src/commands/schema_registry.rs`, `packages/contracts/registry.json` (generated), `scripts/check-registry.mjs`, `apps/finisherd/src/agent/command-of.ts`.
- Modify: `crates/fin/src/commands/schema.rs` (`--registry`), `crates/fin/src/registry.rs` and `registry_*.rs` (each command's `rpcs` and `match`, data only), `apps/finisherd/src/agent/journal.ts` (its argv `commandOf` moves to `command-of.ts`), `scripts/gates/*` (register the gate), `TASKS.md`.
- Test: `crates/fin/tests/cli_schema_registry.rs`, `apps/finisherd/test/agent-command-of.test.ts`.

- [ ] **Step 1: Write failing tests:** `registry_lists_every_command` (count equals `COMMANDS`, plugin commands included as `plugin:<id>/<cmd>`); `registry_is_committed` (the gate fails on a changed registry); `command_of_round_trip` (for every command with an RPC, a sample payload maps back to its name; shared RPCs disambiguate by `match`); `every_rpc_is_named_or_rpc_only` (every `FinRpcTag` is a command's or in `RPC_ONLY`); `journal_command_names_unchanged` (the journal's existing samples).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(fin): export the command registry for server-side rules`.

### Task 2: Rules, layers, project trust and the policy stage — tier: opus — 7B-1, after Task 1

**Files:**
- Create: `packages/contracts/src/rules.ts`, `apps/finisherd/src/agent/{rules.ts, rule-store.ts, glob.ts}`, `apps/finisherd/src/control/migrations/agent/0004.ts` (`agent_rules`, `project_trust`, `sessions.rootSession`, `sessions.depth`), `apps/finisherd/src/agent/trust.ts`.
- Modify: `apps/finisherd/src/handlers.ts:124` (append `commandRules`), `apps/finisherd/src/agent/roles.ts` (role `rules` read through the schema of Task 3; until Task 3 lands, an empty role layer), `apps/finisherd/src/errors.ts`, `crates/fin/src/output.rs` (`RULE_DENIED` already exit 7), `packages/contracts/src/agent.ts` (`agent.rules.list|add|remove`), `agent/rpc.ts` and `agent/console.ts` (call-site lines), `TASKS.md`.
- Test: `apps/finisherd/test/agent-rules.test.ts`, `apps/finisherd/test/agent-trust.test.ts`.

- [ ] **Step 1: Write failing tests:** `glob_matches_segments` (`render`, `asset.*`, `**`, `plugin:@acme/*`, `tool:Bash(curl *)`); `layers_matrix` (deny in any layer beats ask and allow; ask beats allow; no match is allow; a role `allow` never overrides a user `ask`); `where_uses_resolved_values` (a `branch` condition on a branch reference; `minutesOver` on a render's resolved minutes); **`rule_holds_over_any_transport`** (a `deny render` refuses the same render sent by `fin`, by a raw RPC client with the agent credential and by the built-in tool); `rules_ignore_people` (the local principal's render passes a `deny render`); **`project_rules_need_trust`** (a `finisher.rules.json` deny has no effect until `finisherd project trust`, and stops again after the file changes; trusting is refused for `agent` and `local` principals and needs the typed challenge); `rule_admin_is_human_only` (`agent.rules.add` from an agent is `SCOPE_DENIED`).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): command rules in the chain, layered, with project trust`.

### Task 3: Roles as complete data — tier: sonnet — 7B-1 (may start on the 7A-2 branch)

**Files:**
- Modify: `apps/finisherd/src/agent/roles.ts` (schema v1, Decision 14), every `apps/finisherd/src/agent/roles/*.json`, `apps/finisherd/src/agent/planner.ts` (models per harness; tools), `TASKS.md`.
- Test: `apps/finisherd/test/agent-roles.test.ts`.

- [ ] **Step 1: Write failing tests:** every bundled role decodes; `role_invalid_fails_start` (`ROLE_INVALID` names the file and field); `role_narrows_never_widens` (a role's mode ceiling and rules can only narrow its mode's scopes; a role listing a human-only scope is refused); `bundled_roles_carry_default_rules` (Decision 5); `spawn_roles_are_known_names`; `AGENT_ROLES` holds every bundled name (the existing table test).
- [ ] **Step 2: Implement.** Prompt texts and skills: see Question 1 before any text borrowed from Uncut is committed; until answered, roles ship Finisher-written prompts.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): roles as complete data`.

### Task 4: Durable approvals for agent asks, quotas and exit 8 — tier: opus — 7B-1, after Task 2

**Files:**
- Create: `apps/finisherd/src/agent/approvals.ts`.
- Modify: `apps/finisherd/src/chain/chain.ts` (the `ask` branch, `:211-215`: an `agentAsk` port in `ChainDeps`, Decision 8), `apps/finisherd/src/chain/fixes.ts` (`approvalPending`), `apps/finisherd/src/agent/store.ts` (open-approval dedupe and counts; call-site lines), `packages/contracts/src/agent.ts` (`agent.approval.wait`, `APPROVAL_LIMIT`), `crates/fin/src/output.rs` (`APPROVAL_LIMIT` exit 4), `apps/finisherd/src/console/exit-codes.ts`, `TASKS.md`.
- Test: `apps/finisherd/test/agent-approvals.test.ts`.

- [ ] **Step 1: Write failing tests:** **`agent_ask_is_pending_exit_8`** (a rule `ask` for an agent principal answers `APPROVAL_PENDING {approval}` and `fin` exits 8; a person's ask still answers `APPROVAL_REQUIRED`, exit 7); `identical_ask_reuses_card`; **`quota_refuses_not_drops`** (the 17th open approval of a session is `APPROVAL_LIMIT`; the first 16 stay open); `server_quota_256`; `subject_is_server_written` (the card's text comes from the resolved request, never the agent's argv); `expiry_30_min` and `answer_after_expiry_is_expired` (7A's rule holds for rule approvals); `approval_node_and_journal` (`agent.approval` meta with `kind: rule`).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): durable approvals for agent asks, APPROVAL_PENDING and quotas`.

### Task 5: One-time grants, waits, answers, `remember` and provider requests — tier: opus — 7B-1, after Task 4

**Files:**
- Create: `apps/finisherd/src/agent/permissions.ts`, `crates/fin/src/commands/approval.rs` (`fin approval wait|ls`).
- Modify: `apps/finisherd/src/agent/approvals.ts`, `apps/finisherd/src/agent/rules.ts` (consume a grant before `allow` on an `ask` rule), `apps/finisherd/src/agent/rpc.ts:137-150` (`remember` accepted for `rule`/`provider`; the `rulesIn7B` refusal removed), `apps/finisherd/src/agent/runtime.ts` (`approve` dispatches by kind), `apps/finisherd/src/agent/runtime-engine.ts:389-395` (provider requests through `decideProviderRequest`), `apps/finisherd/src/agent/builtin/loop.ts` (a tool call that meets `APPROVAL_PENDING` waits as `waitingApproval`), `crates/fin/src/registry.rs` (the `approval` group), `TASKS.md`.
- Test: `apps/finisherd/test/agent-grants.test.ts`, `apps/finisherd/test/agent-permissions.test.ts`, `crates/fin/tests/cli_approval.rs`.

- [ ] **Step 1: Write failing tests:** **`grant_is_once_and_exact`** (an allowed ask lets exactly one identical request through; the second asks again; other arguments ask again; another session never inherits it); **`grant_survives_no_crash_double`** (kill `finisherd` between consume and commit: after restart the write is absent and the agent's retry asks again, never two commits); `failed_handler_restores_grant`; `wait_caps_at_90s` and `wait_timeout_is_pending_exit_8` (`fin approval wait` exits 8 on timeout, 0 on allow, 7 on deny or expiry, with an injected clock); **`remember_adds_a_rule`** (`remember: session` adds a `local` rule scoped to the session, `project` to the project; listed with its source; removable; audited `agent.rule.add {from}`); **`agent_cannot_answer_or_remember`** (`agent.approval.answer` from any agent principal is refused); `provider_requests_routed` (fake harness: a workspace write is allowed, a write outside the workspace in `plan` denied, a `tool:WebFetch` ask opens a `provider` approval and the answer reaches `respond`; each is an `approval` node with `by`); `builtin_tool_waits_on_pending` (the loop shows `waitingApproval`, then the tool result carries the outcome).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): one-time grants, approval waits and answers, remembered rules, provider requests`.

### Task 6: Subagents: spawn, reach, limits, wait and cascade — tier: opus — 7B-1, after Tasks 2 and 3

**Files:**
- Create: `apps/finisherd/src/agent/subagents.ts`, `apps/finisherd/src/agent/live-limits.ts`, `crates/fin/src/commands/agent.rs` (`fin agent spawn|wait|ls|send|stop`).
- Modify: `packages/contracts/src/scopes.ts:291-313` (`kinds` gain `"agent"` where Decision 16 says), `packages/contracts/src/agent.ts` (`agent.session.wait`; `subagent` node payload), `apps/finisherd/src/agent/reach.ts` (`agentChainHooks().authorize`: own children only; `BranchReach.allowed` subtracts live children's subtrees, Decision 18), `apps/finisherd/src/agent/credentials.ts:43-47` (`MODE_SCOPES.draft|auto` gain `agent.run`), `apps/finisherd/src/agent/runtime.ts` (call-site lines: `start` from an agent principal; `end` cascades), `apps/finisherd/src/agent/planner.ts` (child role and mode checks), `crates/fin/src/registry.rs` (the `agent` group, map line), `crates/fin/src/output.rs` (`DEPTH_EXCEEDED` already exit 7), `TASKS.md`.
- Test: `apps/finisherd/test/agent-subagents.test.ts`, `crates/fin/tests/cli_agent.rs`.

- [ ] **Step 1: Write failing tests:** `spawn_creates_child_on_child_branch` (root from the parent's default-branch head; `parent`, `rootSession`, `depth` set; credential reaches the child's subtree only; `fin agent spawn` returns at once); **`agent_run_only_for_own_children`** (an agent may prompt, steer, interrupt and end its children; another session's, a sibling's or its parent's are `SESSION_NOT_FOUND`; `resume`, `delete`, `rollback` from an agent are `SCOPE_DENIED`); **`parent_cannot_write_live_child_branch`** and `parent_merges_child_into_own` (`fin merge --from <child>` into the parent's branch works; no outcome is recorded for it); `child_mode_at_most_parent` and `child_role_in_spawn_list`; **`depth_and_live_limits`** (depth 4 and the 9th live descendant are `DEPTH_EXCEEDED`); `live_limits_queue` (the 9th built-in session's run waits with `SESSION_QUEUED` and starts when one ends); `child_ends_itself_with_summary` and the parent's `subagent` node; `wait_any_and_timeout` (`WAIT_TIMEOUT`, exit 5, repeatable); `parent_end_interrupts_children`; `pre_7b_credential_cannot_spawn`; the journal's `agent.session` for a child has `parent` and `trigger: subagent`; the map stays under 1.5k tokens.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): subagents on child branches with fin agent spawn, wait, ls, send and stop`.

### Task 7: Budget carve-outs for children — tier: opus — 7B-1, after Task 6

**Files:**
- Create: `apps/finisherd/src/agent/carve.ts`.
- Modify: `apps/finisherd/src/agent/{ledger.ts, budget.ts, store-tables.ts}` (`carve`, `settleCarve` on `LedgerStore` and `DbLedger`), `apps/finisherd/src/agent/subagents.ts`, `apps/finisherd/src/agent/runtime.ts` (`approve` of a child's budget approval raises both), crash recovery in `agent/store.ts` (a carve whose child is gone is settled at start), `TASKS.md`.
- Test: `apps/finisherd/test/agent-carve.test.ts`.

- [ ] **Step 1: Write failing tests:** `carve_reserves_on_parent` (one transaction with the child's creation); `carve_returns_unused` (the parent's remaining after the child's end equals before minus what the child used); `default_carve_is_half_remaining_capped_by_role` (Decision 22); `child_cannot_exceed_carve` (`BUDGET_EXCEEDED` on the child, not the parent); `raise_child_raises_parent_reserved_or_fails`; `carve_property` (fast-check over spawn, spend, end, crash sequences: `used + reserved ≤ limit` for every parent at every step; no carve outlives its child after recovery); per-call reservations of 7A still settle (existing tests).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): child budgets carved from the parent's remaining`.

### Task 8: Rollback to a run — tier: opus — 7B-1, after Task 6

**Files:**
- Create: `apps/finisherd/src/agent/rollback.ts`.
- Modify: `packages/contracts/src/agent.ts` (`agent.session.rollback`), `packages/contracts/src/scopes.ts` (rule: `agent.run`, kinds `SESSION_DRIVERS`), `apps/finisherd/src/agent/builtin/{loop.ts:393, loop-history.ts}` (the built-in rollback: later runs' nodes marked and skipped), `apps/finisherd/src/agent/store.ts` (`nodes.rolledBack`; migration 0004), `apps/finisherd/src/agent/console.ts` (`agent rollback <id> --to-run <seq>`), `apps/finisherd/src/agent/outcomes.ts` (`details.rollback`), `TASKS.md`.
- Test: `apps/finisherd/test/agent-rollback.test.ts`.

- [ ] **Step 1: Write failing tests** against a real engine: three runs with commits on the root and on a `fin try` branch; `rollback --to-run 1` undoes runs 2 and 3's transactions on both branches, newest first, as the session's author; **`conflict_leaves_both_sides`** (a person's later commit that conflicts with one undo: `CONFLICT`, every branch exactly as before, including the branch already undone); `refused_while_running_or_with_live_child`; `builtin_history_skips_rolled_back_runs` (the next request has no messages of runs 2–3; the nodes are still shown by `agent show`); `fake_provider_rollback_called` (capability true) and `cli_without_capability_keeps_provider_history` (capability false: transactions undone, provider history left, a `notice` says so, spec §4.2); outcomes `undone` by `user` with `details.rollback`; `agent_cannot_rollback`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): roll a session back to a run`.

### Task 9: Context compaction for the built-in loop — tier: opus — 7B-1

**Files:**
- Create: `apps/finisherd/src/agent/builtin/compact.ts`, `apps/finisherd/src/agent/builtin/prompts/compaction-v1.md`.
- Modify: `apps/finisherd/src/agent/builtin/{loop.ts:59, :283-286, loop-history.ts}`, `packages/contracts/src/agent-events.ts` (`NodeKind` gains `summary`), `crates/fin-core/src/events/payloads/agent.rs` (`agent.turn.purpose?: compaction`, additive), the generated schema and types, `apps/finisherd/src/agent/journal.ts` and `journal-content.ts` (the compaction turn; the next delta starts over), `TASKS.md`.
- Test: `apps/finisherd/test/agent-compaction.test.ts`, `crates/fin-core/tests/events.rs` (the new field's golden).

- [ ] **Step 1: Write failing tests** (fake model server with a small window): a run that would pass 75 % compacts first: one summary call with the compaction prompt, a `summary` node, the next request holds the system prompt and tools byte-identical and the summary instead of the older runs; the compaction turn is charged and journaled with `purpose: compaction`; **`replay_after_compaction`** (the trace store's deltas replay to every request's `bodyHash`, the turn after compaction starting from `from: 0`); a compaction that does not fit ends the run with `MODEL_CONTEXT_EXCEEDED`; resume after a crash mid-compaction rebuilds the same history (the summary node is the boundary).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): context compaction for the built-in loop`.

### Task 10: Saved prompts — tier: sonnet — 7B-1, after Task 3

**Files:**
- Create: `apps/finisherd/src/agent/saved-prompts.ts`, `apps/finisherd/src/agent/prompts/make-shorts.json`.
- Modify: `packages/contracts/src/agent.ts` and `scopes.ts` (`agent.savedPrompt.list|run`), `apps/finisherd/src/agent/{rpc.ts, console.ts}` (call-site lines; `finisherd agent run <name>`), `apps/finisherd/src/agent/settings.ts` (`agents.savedPrompts`), `crates/fin/src/output.rs` (`SAVED_PROMPT_UNKNOWN` exit 3), `TASKS.md`.
- Test: `apps/finisherd/test/agent-saved-prompts.test.ts`.

- [ ] **Step 1: Write failing tests:** `make_shorts_expands` (`count=5` → a `clip-finder` session with the expanded first prompt and `trigger: savedPrompt`); `params_validated` (`count=0`, `count=x`, an unknown param: `AGENT_ARGS` naming the field); `template_has_no_expressions` (a `{{` that is not a declared name is refused at load); `owner_prompts_from_settings`; `project_folder_prompts_ignored`; **`people_only`** (`agent.savedPrompt.run` from an agent principal is `SCOPE_DENIED`; `fin` has no command for it); `unknown_is_saved_prompt_unknown`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): saved prompts`.

### Task 11: Review requests from sessions and critics on child branches — tier: sonnet — 7B-2 (after 5E)

**Files:**
- Create: `apps/finisherd/src/agent/review-link.ts` (the `branchReady` event and its no-op hook point).
- Modify: `apps/finisherd/src/agent/credentials.ts` (`MODE_SCOPES.draft|auto` gain `review.write`), `apps/finisherd/src/agent/{store.ts, runtime.ts}` (`session_state.reviews`; a session that ends with unmerged commits emits `branchReady`), `apps/finisherd/src/agent/planner.ts` (a `critic` with `base` = a reviewed branch gets root `<reviewed>/critic-<n>` through `openSession {root}`), `TASKS.md`.
- Test: `apps/finisherd/test/agent-review-link.test.ts`.

- [ ] **Step 1: Write failing tests:** `fin review request` from a `draft` session opens a 5E item for its branch and emits `branchReady` with the head seq; a session ending with unmerged commits emits it too; **a critic cannot write the reviewed branch** (`BRANCH_NOT_ALLOWED`) but can write `<reviewed>/critic-<n>`; a critic's undo of an agent transaction on its child branch is recorded `by: critic`, `via: critic`; with 5E's `review.verdict` v1 registered, the tracker now emits (`kindVersion` answers 1) and a `review.merge` is not double-counted (5E's note); no mode gains `review.decide` or `branch.protected`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(agent): review requests from sessions and critics on child branches`.

### Task 12: The `auto` merge trigger — tier: opus — 7B-2, after Task 11

**Files:**
- Create: `apps/finisherd/src/agent/auto-merge.ts`.
- Modify: `apps/finisherd/src/agent/review-link.ts` (call the trigger on `branchReady`), `apps/finisherd/src/agent/outcomes.ts` (wire `via: autoMerge`, `by: policy` from 5E's merge result; `store-outcomes.ts:11-16` already has the values), `TASKS.md`.
- Test: `apps/finisherd/test/agent-auto-merge.test.ts`.

**Interfaces:** `autoMergeTrigger(deps: { reviews; tryAutoMerge: typeof import("../reviews/auto-merge.ts").tryAutoMerge; store; budget }): (ev: BranchReady) => Effect<AutoMergeOutcome>`; `AutoMergeOutcome = {merged: true, mergeId} | {merged: false, reason: "notAuto" | "roleNotAdmitted" | "approvalOpen" | "overBudget" | "criticNotDistinct" | "criticStale" | AutoMergeRefusal}`.

- [ ] **Step 1: Write failing tests** (5E's `VerdictSource` and availability injected through the constructor, never the environment): `nothing_merges_by_default` (no `VerdictSource`: the item stays open); `draft_session_never_triggers`; `role_not_admitted`; `open_approval_blocks`; `over_budget_blocks`; **`critic_must_be_distinct`** (the authoring session, its child or its root session's descendant as critic → no merge); `critic_on_older_head_blocks`; `merges_when_all_hold` (5E merges as `policy`; the tracker records `merged`, `by: policy`, `via: autoMerge`); **`no_rpc_reaches_the_trigger`** (every RPC tag with every principal kind never calls it, checked with a spy, as 5E's own test does for `tryAutoMerge`).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(agent): the auto merge policy's session-side trigger`.

### Task 13: e2e `subagents` scenario, spec as built, tracker — tier: sonnet — after Task 12 (7B-1 parts after Task 10)

**Files:** Create `scripts/e2e/subagents.mjs`. Modify `scripts/e2e.mjs` (register), `docs/specs/2026-11-07-sub-project-7-agent-runtime-design.md` (§5.3, §6, §7, §13.1 rewritten "as built"; the deviation of Decision 10; new codes `APPROVAL_LIMIT`, `SESSION_QUEUED` notice; status line), `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§4.4: the agent `ask` path; one line), `docs/specs/2026-11-01-finisher-architecture-design.md` (§27 pointer), `README.md` (rules, approvals and subagents from the console), `TASKS.md` (follow-ups for 7D/7E: map permission requests onto `permissions.ts`, implement `rollback`).

- [ ] **Step 1: Write the scenario** (scripted `fake` harness and the fake model server, from the console): a person adds `ask render` and starts an `editor` session; the agent's render answers exit 8; `finisherd agent approve <id> allow --remember session`; the agent's `fin approval wait` exits 0 and its render runs once; the second identical render passes by the remembered rule; the editor spawns two `clip-finder` children with `fin agent spawn`, waits for both with `fin agent wait --any` twice, merges one child's branch into its own; a third-level spawn chain stops at depth 3 with `DEPTH_EXCEEDED`; the parent's money remaining equals its start minus what both children spent; `finisherd agent rollback <id> --to-run 1` undoes the merge; a saved prompt `make-shorts --param count=2` starts a session; the built-in session with a small window compacts and completes. With 5E: a session's `fin review request` opens an item and `auto` merges nothing (no `VerdictSource`).
- [ ] **Step 2: Write** the as-built sections from the code (names, defaults, codes) and the follow-ups.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `test(e2e): subagents, rules and approvals` and `docs: record sub-project 7B as built`.

### Task 14: Cross-platform validation (controller-run; needs hosts and keys) — tier: opus (controller)

- [ ] **Step 1:** On macOS, Windows 11 (non-elevated, through the console session) and the Linux VM: `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` (the `subagents` scenario included).
- [ ] **Step 2:** With the owner's OpenRouter key (or a local server), one real built-in `editor` session that spawns a `clip-finder` child on the reel project, waits for it and merges its branch; one rule `ask` answered from a second device (a paired device once 5B exists, else the console); one compaction on a model with a small window; costs and carve-outs checked against the ledger. Record measured latencies and costs in `archive/`, conclusions in the spec.
- [ ] **Step 3:** Record results in TASKS.md; fix waves as needed; final whole-branch review. **Commit** `docs: 7B validation results`.

## Questions for the owner

1. **Role prompts borrowed from Uncut (spec §6.2, D26).** The spec says the roles' prompts are skills borrowed from Uncut. Under what licence may that text be committed to this MIT repository (is Uncut the owner's own work, or third-party text)? Until this is answered, Task 3 ships Finisher-written prompts. *Needs owner confirmation (legal).*
2. **A child's default budget (Decision 22).** When `fin agent spawn` names no budget, the child gets its role's budget capped at half the parent's remaining, per kind (recommended). Alternatives: the role's budget with no cap (a parent's children could together exhaust it quickly), or refuse a spawn without `--budget`. *Needs owner confirmation (cost).*
3. **Saved prompts from project folders (Decision 15).** v1 reads saved prompts from the bundle and the owner's settings only, not from a project's folder (a shared project could otherwise offer a prompt that starts an `auto` session with a large budget). Keep it so (recommended), or allow project prompts behind the same trust as project rules? *Needs owner confirmation.*
4. **Where `remember: project` writes (Decision 12).** Into the server's `local` layer for that project in `control.db` (recommended: never into the project folder, so a remembered `allow` does not travel to collaborators), or into the project's `finisher.rules.json` so it travels with the project? *Needs owner confirmation.*
5. **Grant consumption across two databases (Decision 10).** The spec says a grant is consumed "in the same transaction as the write"; the write is in the project store and the grant in `control.db`. The plan consumes first and restores on failure, so a crash loses a grant (the agent asks again) and never doubles a write. Accept this deviation (recommended), or require the grant in the project store's transaction (a new `fin-store` field)? *Needs owner confirmation.*
6. **Rollback is people-only (Decision 23).** The spec lists `agent.session.rollback` under `agent.run` without saying who; the plan lets only people roll a session back (an agent undoes its own work with `fin undo`). Confirm, or let a parent roll back its own children? *Needs owner confirmation.*
