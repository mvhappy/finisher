# Sub-project 5E: Review workflow — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (the `drop` record and its meaning, format 3, merge limits and the review plan, protected branches and the policy gate, the workflow, the internal auto-merge path, validation), `sonnet` for plumbing (the verdict kind, contracts and the reviews store, conflict items and suggestions, `fin` and the console, e2e, docs).

**Goal:** agent branches go through a review that a person decides, one transaction at a time, and what the person decided binds every later merge. After this plan:
- a **`branchReview` item** holds a change list planned at a snapshot of both heads; a person accepts or rejects each origin transaction; **a merge copies only what was reviewed** (`include` and `sourceUpTo`, checked inside the merge's write transaction), so a commit that lands on the source after the review is never merged by it;
- **a rejection is a `drop` record in the op log** of the target branch: it syncs, survives moves and rebuilds, and keeps a later plain `fin merge` from bringing the transaction back, on this server or any other;
- **merge conflicts become `mergeConflict` items**; 8A's plugin suggestions become `pluginSuggestion` items;
- **protected branches** (default `main`) refuse writes from principals without `branch.protected`, checked at the chain's policy stage and again inside the store's write transaction; `local` keeps the scope (owner decision 5); the per-project policy is human-only;
- the **`auto` merge policy** exists as a server-internal path with all of its checks and tests, and **enabling it answers `AUTO_MERGE_UNAVAILABLE`** until sub-project 10 records QA-gate and critic verdicts;
- **`review.verdict` v1** is registered with sources `finisherd` and `agent`, so 7A's outcome tracker emits it as soon as this lands;
- `fin review request|ls|show|refresh|comment|decide|merge`, `fin drop`, `fin merge --include/--up-to`, and `finisherd review policy …`.

**Architecture:**
- **`fin-core`** gains `RecordKind::Drop` with `drops`/`undrops`, `history/drops.rs` (what is dropped for a lineage), the `DROPPED` reason, `include`/`sourceUpTo` in `plan_merge`, and `history/review.rs` (the change list with every candidate's status).
- **`fin-store`** gains format-3 segments for `drop` records, migration `m0004_drops` (a derived index), `Store::drop`, the merge limits, `Store::review_plan`, and the protected-branch check through 5A's `WriteGuard.deny_branches`.
- **`fin-node`** gains `engine.drop`, `engine.reviewPlan` and the new merge fields.
- **`fin-core::events`** fills `review.verdict` at version 1.
- **`@finisher/contracts`** gains `reviews.ts` and `review-rpcs.ts` (the RPCs spread into `FinRpcs`), `history.drop`, the `review` resolver, rules and codes.
- **`finisherd`** gains `control/migrations/reviews/0001.ts`, `reviews/{store.ts, plan.ts, workflow.ts, conflicts.ts, suggestions.ts, rederive.ts, policy.ts, gate.ts, auto-merge.ts, verdicts.ts, rpc.ts, hub.ts}`, `chain/protected.ts`, `console/commands/review.ts`; it changes `history.ts` and `handlers.ts` (call-site lines), `plugins/lifecycle.ts` (suggestions), `chain/resolve.ts`.
- **`fin`** gains `commands/review.rs` and `commands/drop.rs`, and `--include`/`--up-to` on `fin merge`.

**Tech Stack:** Node 24.22 (`node:sqlite`), Effect 4.0.6, vitest, fast-check 4 (already a dev dependency); Rust 2024 (`proptest` as already used by `fin-store`'s property tests). **No new dependency.**

**Spec:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` revision 6: §1.2 (live re-planning cut), §2 principles 7 and 9, §3.1 (`reviews` chain), §3.3 (`history.drop`, `review.merge`, `review.policy` audit kinds), §3.4 (`review.verdict`), §3.5, §4.4 stages 5–7 and §4.5 (`branch.protected`, `review.write`, `review.decide`), **§8** (all of it), §11 Decisions 28–32, §12 decisions 1 and 5, §13 (the `drop` and auto-merge risks), §14 items 6–8. Sub-project 4 spec §4.2 (field order), §4.4 (merges, `DEPENDS_ON_SKIPPED`). Agent runtime spec §1.7 items 5–6, §6.5, §12.5. The 7A-2 Task 11 note in `TASKS.md` on `sp7a2-t11` (the verdict fields and `kindVersion("review.verdict", "agent")`). The 8A Task 9b follow-up (`plugin_suggestions` → `pluginSuggestion` items).

**Branch:** `sp5e-review`, stacked on `sp4c-edit-breadth` (the integration branch, which holds 4B and 5A). It needs neither 5B nor 5C-2 nor 7A-2:
- **7A-2** (on `sp7a2-*`, not yet merged) adds `chain/hooks.ts` and the outcome tracker. 5E does not use the chain's `after` hook (it sees successes only, and 5E needs merge conflicts): `history.ts` calls 5E's observer directly (Decision 21). When 7A-2 merges, its tracker finds `review.verdict` at version 1 and starts emitting; `review.merge` is not a history RPC, so the tracker never double-counts it (the 7A note).
- **5B** (on `sp5b-remote`) touches the chain's confirmation stage; 5E's rules use 5A's `confirm` defaults and add nothing there.
- Shared files: `packages/contracts/src/rpc.ts` (one spread line), `scopes.ts` (a block of rules at the end), `handlers.ts` (one spread line), `chain/resolve.ts` (one resolver), `chain/policy.ts` (`DEFAULT_POLICIES` gains a rule).

**Out of scope:** the QA gate, the critic and their verdicts (sub-project 10; 5E reserves where they are recorded); delivering `changesRequested` to an agent as a turn (sub-project 7); review markers and the share page (10); A/B compare UI (9); live re-planning (cut); review state that syncs between servers (owner decision 1: items live in this server's `control.db`).

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code. No new dependency.
- **Primitive ops live only in `fin-core`.** A `drop` record has no ops and is no primitive op; it is planned by the history layer like `checkpoint` (spec §8.3).
- **TypeScript model types are generated, never hand-written:** the record kind, `DropReq`, the merge fields, `MergeReport.notIncluded`, `ReviewPlan` and the verdict payload are regenerated with `pnpm --filter @finisher/core-types generate` (drift gate `scripts/check-generated-types.mjs`). Review items are TS-owned Effect Schemas in `@finisher/contracts`.
- **File sizes:** non-test source ≤ 600 lines, tests ≤ 1,500. Near the limit: `packages/contracts/src/rpc.ts` 540, `apps/finisherd/src/projects.ts` 596, `crates/fin-store/src/tick.rs` 23.7 KB, `crates/fin/src/main.rs`: call-site lines only. `fin-core/src/history/merge.rs` is 19 KB: the limits go in a new `history/limits.rs`, the review plan in `history/review.rs`.
- **The record format changes only for `drop` records.** Every other kind stays byte-identical at format 2; no stored record changes shape; the op schema (`schemaVersion`) does not change; no upcaster (spec §8.3).
- Errors are `{code, message, details, fix?}`. New codes go into `crates/fin/src/output.rs`, `apps/finisherd/src/console/exit-codes.ts` and the contracts' code list (Decision 26).
- **Review text is content.** Titles, summaries, comments and reasons are never audited, never journalled except `review.verdict.reason` (training only, `Content<userText>`), and never in an error message.
- **The repo is public:** no host names, IPs, user names or key names in tracked files.

**Every commit is green.** A task that changes what the server answers changes `fin`, the console and the e2e harness in the same commit.

**Testing and process**
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`; tasks that touch `fin` or the server's answers also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh`.
- **Tracker:** each task ticks its line in `TASKS.md` under "Sub-project 5E", in the same commit.
- **Batches:**
  - A: Tasks 1 → 2 → 3 (the store layer, in order); Task 4 beside them;
  - B: Task 5, then Task 6;
  - C: Task 7, then Tasks 8 and 9;
  - D: Tasks 10, 11;
  - E: Tasks 12, 13.

## Decisions taken here (the spec fixes the shape; these fix the details)

### The `drop` record

1. **`RecordKind::Drop`** (`"drop"`) in `fin-core`, after `BranchSet` (the enum is `#[non_exhaustive]`). It reuses `from` (the source branch id) and adds two fields at the end of the kind fields, after `recovers` (sub-project 4 §4.2's order becomes `…, recovers, drops, undrops`): `drops` and `undrops`, each an optional list of origin-root txIds, at least one non-empty, each sorted and deduplicated at write, at most 4,096 ids in all; an id in both is `INVALID_OP`. `intent.ops == []`; `has_ops()` and `is_content()` are false; `Link::Drop {from, drops, undrops}`. Like `checkpoint`, a `drop` is no unit on any undo stack.
2. **Meaning** (`fin-core` `history/drops.rs`): an origin root is **dropped for T** when the newest `drop` record on T's lineage that names it lists it in `drops`. "Newest" is lineage order: an ancestor's records up to the fork seq come before the branch's own, and records on one branch go by seq. Branches forked from T after a record inherit it; a record on T after a fork does not reach the child (it inherits records before its fork point only, as every record). Computed from the records alone, so a writer, a follower that imported the records in another order and a rebuild agree.
3. **Honoured by:** merge candidate selection into T (a dropped origin is left out with reason **`DROPPED`**, and what depends on it is `DEPENDS_ON_SKIPPED` under the existing rules); `derived::compute`'s `merged` state (`left_to_merge` treats a dropped origin as not left); **not** cherry-pick (an explicit pick of a dropped origin is allowed and does not undrop); **not** undo stacks. **Deviation in wording:** spec §8.3 says a dropped origin is reported "in `skipped`"; `skipped` holds the conflicts that make a plain merge `CONFLICT`, so `DROPPED` goes into `excluded` beside `UNDOES_TARGET_RECORDS` (left out, not a conflict). The spec's as-built pass records it.
4. **Format** (spec §8.3): `drop` records are written at **record format 3, min reader 3**; `fin_core::record::record_format(kind)` answers 3 for `Drop`, 2 otherwise (`RECORD_FORMAT` and `MIN_READER` stay 2 as constants for every other kind). `fin_store::segment::version_for` and `version_for_branch` answer `(3, 3)` for a segment holding a `drop` record and keep their answers otherwise; `READER_VERSION` becomes 3. A 4C reader meeting such a segment opens the project read-only with `NEWER_FORMAT` (the existing rule). Project DB migration **`m0004_drops`** adds the derived index `drops (branch, origin, seq, dropped)`, maintained by every write and rebuilt by rebuild and import like the other derived tables; the DB's own version follows `m0003_version`'s rule.
5. **`Store::drop`** (`history_ops/drop.rs`): `DropReq {txId, author, into, from, drops?, undrops?, dryRun?, argsHash?}` → `CommittedTx`. Every named id must be an origin root of a record on `from`'s lineage after its common point with `into` (`INVALID_OP {reason: notOnSource, txId}` otherwise); the record is written on `into` through the one write path (the guard applies: Decision 16); idempotent on `txId` like every history write; `dryRun` plans and keeps nothing.

### Merges limited to what was reviewed

6. **`MergeReq` gains `include?: [TxId]`** (origin roots) **and `sourceUpTo?: u64`**, applied by `plan_merge` (`history/limits.rs`) to the candidates it plans inside the merge's write transaction: a candidate is copied only if its origin root is in `include` (when given) and, **for a candidate on the source branch itself**, its seq is ≤ `sourceUpTo` (a candidate on an ancestor branch below the source's fork is older than the fork and passes). The rest stay on the source and are reported in **`MergeReport.notIncluded`** (origin roots, oldest first). An included candidate that depends on one not included or dropped is `DEPENDS_ON_SKIPPED`, a conflict (the merge writes nothing unless `skipConflicting`). `include: []` merges nothing. `fin merge --include <txId>… --up-to <seq>`.
7. **The review plan** (`Store::review_plan`, `fin-core` `history/review.rs`), a read-only call that works on followers: `ReviewPlan {source: {branch, seq, hash}, target: {branch, seq, hash}, entries, conflicts, truncated}`; each entry `{origin, txId, branch, seq, kind, author, actor?, opsCount, committedAt, status: willMerge | alreadyPresent | dropped | excluded | dependsOnSkipped | conflict, reason?, blame?}`, one per candidate and one per source-branch record already present, oldest first, at most 10,000 (`truncated: true` past it). It is `plan_merge` with an explain sink, so the change list and the merge can never disagree on a status. `fin-node` `engine.reviewPlan(json)`.

### The reviews store

8. **Chain `reviews`, migration `0001`** (spec §3.1, §8.1): `reviews {id, projectId, kind, state, source?, target?, snapshot (JSON), title, summary?, requestedBy (JSON {principal, actor}), createdAt, updatedAt, closedAt?, closedBy?, mergeId?, rederived, subject? (JSON: a conflict list or a plugin id)}`; `review_decisions {itemId, txId, decision: accept|reject, decidedBy, reason?, at}` keyed by `(itemId, txId)`, so decisions survive refreshes (spec §8.2); `review_events {id, itemId, at, kind, by, details}`; `review_comments` (spec §8.1); `branch_policy {projectId, protected (JSON ids), requireReview, autoMerge (JSON)}`; and **`review_verdicts {itemId, kind: qa|critic, snapshot, pass, verdictId, at}`**, reserved for sub-project 10 (5E writes it only in tests). Indexed on `(projectId, state)` and `(projectId, source, target)`.
9. **Snapshot** `{sourceSeq, sourceHash, targetSeq, targetHash, plannedAt}`: `expect` must match the seqs and hashes (`plannedAt` is ignored), so a rewind to the same seq is caught. The change list is derived, cached in memory per `(itemId, snapshot)` (the newest 64), and re-planned on a miss, never stored. `review.get` reports `stale: true` when either head's seq or hash moved.
10. **One open `branchReview` per `(projectId, source, target)`**: requesting it again answers the open item with `existing: true` (title and summary unchanged). At most 256 open items per project (`REVIEW_LIMIT`, exit 4). Titles 1–200 characters, summaries ≤ 8 KiB, comments ≤ 16 KiB and ≤ 1,000 per item, reasons ≤ 2 KiB; no control characters but newlines.
11. **Plugin suggestions** (the 8A follow-up): `plugin.suggest` writes a `pluginSuggestion` item (`subject {pluginId}`, `title` the plugin id, `summary` the reason, `requestedBy` the suggester) instead of a `plugin_suggestions` row. At start, after every chain has migrated, rows still in `plugin_suggestions` are moved in one transaction (inserted if absent by id, then deleted); the table stays, unwritten, and a later `plugins` migration drops it (a `TASKS.md` follow-up: dropping it in this plan's migrations would race the move across chains). A person closes a suggestion with `review.close {state: dismissed | rejected}`; installing the plugin supersedes its open suggestions. `review.decide`, `review.merge` and `review.refresh` on a `pluginSuggestion` or `mergeConflict` item are `INVALID_REQUEST {reason: kind}`. *Needs owner confirmation* (Question 4).

### The workflow

12. **Request:** `review.request {projectId, source, into?: "main", title, summary?}` resolves both branches through the store, refuses the same branch or an archived source (`INVALID_REQUEST`), plans the change list (Decision 7) and stores the snapshot. Comments: `review.comment {itemId, body, at?, txId?}`.
13. **Decide:** `review.decide {itemId, expect, decisions?: [{txId, decision: accept | reject | pending, reason?}], requestChanges?: {reason?}}`; `expect` ≠ the item's snapshot is `REVIEW_STALE` with the fresh change list in `details`. Decisions name origins of the snapshot's entries (others `INVALID_REQUEST`). **A rejection writes a `drop` record at once**, one per call for every newly rejected origin (author: the decider's author; `from` the source, `into` the target); moving a rejected origin back to `accept` or `pending` writes an `undrops` record. `requestChanges` sets `changesRequested` and publishes it on the review hub, which sub-project 7 delivers as a turn; a new `review.refresh` or `review.request` by the source's author reopens it.
14. **Merge:** `review.merge {itemId, expect, acceptPending?, idemKey}` → `Store::merge {txId: idemKey, from, into, include, sourceUpTo: snapshot.sourceSeq}` called through the engine (not the `history.merge` RPC), where `include` = the accepted origins, plus the snapshot's pending ones with `acceptPending: true`. With `requireReview`, any pending entry refuses the merge (`REVIEW_UNDECIDED`) whatever `acceptPending` says. A conflict (`DEPENDS_ON_SKIPPED` included) is `REVIEW_HAS_CONFLICTS` with the list, and nothing is written. Afterwards: state `merged`, `mergeId` stored, and the source archived (a `branchSet` by the merger) when its author is an `agent:`/`mcp:` credential and `left_to_merge(source, target)` is empty. **A target that moved since the snapshot does not block the merge** (spec §8.2 compares `expect` with the item's snapshot, not the heads): the include set is fixed, conflicts refuse it, and the result says `targetMoved: true`. *Needs owner confirmation* (Question 2).
15. **Close:** `review.close {itemId, state: rejected | dismissed}`. Rejecting all writes one `drop` for **every snapshot entry not yet merged into the target, pending and accepted alike** (the spec's wording says "every pending entry"; an accepted mark on a rejected item would otherwise let a later plain `fin merge` bring the change in); dismissing writes nothing. *Needs owner confirmation* (Question 3). `REVIEW_CLOSED` for any write on a closed item.

### Protected branches

16. **The `protectedBranches` policy rule** (`chain/protected.ts`, after `agentBranchLimits` in `DEFAULT_POLICIES`): for every RPC with `writes` or `targets` (5A's rule fields), it reads the resolved targets; when one is protected for the project and the principal lacks **`branch.protected`**, the call is **`BRANCH_PROTECTED`** (exit 7, fix in words: a person reviews and merges it). For such a principal the outcome's guard carries `denyBranches` = the project's protected ids, so `fin-store` refuses inside the write transaction too (a rename or a re-resolution cannot slip, spec §8.5). `review.merge` and `history.drop` into a protected target need **`review.decide`** instead (their rules declare `protectedNeeds: "review.decide"`, a new `RuleOf` field). `local` keeps `branch.protected` and `review.decide` (owner decision 5), so `fin` still writes `main`; `agent` and `mcp` credentials do not get them by default (5A's mint defaults). `branch.set` of a protected branch (archive, rename) is a write like the others.
17. **`history.drop` scopes** (spec §8.6): base `project.read` plus an escalation from the resolved facts: `review.decide` when the target is protected (`RuleFacts.protectedTarget`, set by the `branch` resolver from the policy), `project.write` otherwise.
18. **The policy:** `review.policy {projectId, protected?, requireReview?, autoMerge?: {enabled}}` (`auth.admin`, human-only, confirmed; protected entries are branch references resolved to ids) and `review.policy.get` (`project.read`). Default `{protected: ["main"], requireReview: false, autoMerge: {enabled: false}}`. Audited `review.policy {old, new}` (a new audit kind; branch ids and booleans only). **Writes are serialised with the project's in-flight writes** (`reviews/gate.ts`): every write RPC of a project holds the gate shared from the policy stage to its answer, a policy write holds it exclusive, and waits at most 30 s (`POLICY_BUSY`, exit 5); so a merge either completes under the old policy or is checked under the new one.
19. **Auto merge** (spec §8.5): turning `autoMerge.enabled` on is **`AUTO_MERGE_UNAVAILABLE`** (exit 4) while the build constant `AUTO_MERGE_AVAILABLE` is `false`, which sub-project 10 flips when it records QA and critic verdicts. The path exists now (`reviews/auto-merge.ts` `tryAutoMerge(itemId, deps)`): it is called only inside `finisherd` with the server-internal actor `{kind: policy, policy: autoMerge, enabledBy}`, checks `policyOff | requireReview | verdictMissing | verdictFailed | snapshotMismatch` in that order, and on success runs the Decision 14 merge with `acceptPending: true` and `sourceUpTo` of the judged snapshot, under the gate, audited `review.merge {auto: true, item, snapshot, session?, verdicts, enabledBy}`. **No RPC reaches it:** its only callers are a `VerdictSource` port (none in release builds) and the tests, which inject a fake verdict source and an availability override through the constructor, not the environment.

### Verdicts

20. **`review.verdict` v1** (`fin-core`, replacing the reserved v0; sources `Finisherd` and `Agent`, so `kindVersion("review.verdict", "agent")` answers 1 for 7A's tracker): `itemKind: branchReview | mergeConflict | qaFinding | criticSummary | pluginSuggestion | none`; `verdict: accepted | rejected | merged | dismissed | changesRequested | undone | reverted | abandoned`; `via?: undo | revert | archive | merge | critic | autoMerge` (absent for a person's decision on an item); `by: user | agent | critic | policy`; `txIds: IdRef[]` (≤ 64; a decision on more is several entries); `accepted`, `rejected` (counts); `sessionId?: IdRef`; `openS` (seconds since the item opened, or since the transaction for 7's entries); training only: `reason: Content<userText>`. 5E emits one entry per `review.decide` (accepted and rejected batches), `requestChanges`, merge and close; `by` is `user` for `console`, `device` and `local`, `agent` for `agent` and `mcp`, `policy` for the auto-merge actor. Names come from new closed tables; `RPC_METHODS` gains the new RPCs.

### Conflicts and re-derivation

21. **`mergeConflict` items** (`reviews/conflicts.ts`): `history.ts` hands every `history.merge` outcome to `Reviews.observeMerge(projectId, from, into, report | CONFLICT error)` (call-site lines, inside the handler, so it works whether or not 7A-2's chain hooks exist). A `CONFLICT` opens or updates the item for `(source, target)` with the conflict list in `subject`; a later successful merge of the pair, or archiving the source, makes it `superseded`. `review.merge`'s own outcomes go through the same observer. Rebase conflicts on `tx.commit` stay with their writer (spec §8.5).
22. **Re-derivation** (`reviews/rederive.ts`, spec §8.1 and §3.1's damage rule): when a project is loaded on this server, and after a `control.reset`, every **active** branch (not archived, not merged) whose author is an `agent:` or `mcp:` credential, that has candidates into its fork parent and no open `branchReview`, gets one into that parent with `rederived: true` and a generated title. Idempotent; at most 64 per load (the rest on the next load). Comments and accept marks of the old server's items are lost (owner decision 1); rejections are not (they are `drop` records). *Needs owner confirmation* (Question 1).

### RPCs, `fin`, the console

23. **RPCs** (`packages/contracts/src/review-rpcs.ts`, spec §8.6): `review.request`, `review.comment`, `review.refresh` (`review.write`); `review.list {projectId, state?, kind?}`, `review.get {itemId}`, `review.watch {projectId}` (stream: item changes, snapshot first, never re-plans) (`project.read`); `review.decide`, `review.merge`, `review.close` (`review.decide`); `review.policy` (`auth.admin`) and `review.policy.get` (`project.read`). `history.drop` joins the history RPCs in `history.ts` of the contracts (Decision 17's scopes). A new resolver **`review`** maps `itemId` to the item's `projectId` (reach), `source` and `target` (the policy stage), so a caller outside the project gets `REVIEW_NOT_FOUND` exactly as for an unknown id. The resolver adds the item's `target` (a branch id) to the resolved payload, and the rules of `review.decide`, `review.merge` and `review.close` declare `writes: ["target"]` (they write `drop`, `merge` or `branchSet` records through the guard), so they are in `WRITE_RPCS` and agent branch limits and protected branches see their target like any write's.
24. **`fin`:** `fin review request [--branch <b>] [--into <b>] --title <t> [--summary <s>]`, `fin review ls [--state] [--kind]`, `fin review show <id> [--json]` (the change list), `fin review refresh <id>`, `fin review comment <id> <text> [--tx <txId>]`, `fin review decide <id> (--accept <txId>… | --reject <txId>… | --pending <txId>…) [--reason <r>] [--request-changes]`, `fin review merge <id> [--accept-pending]`, `fin review close <id> --rejected|--dismissed`; `fin drop <txId>… --from <b> [--into <b>]`, `fin drop --undo <txId>… …`; `fin merge --include <txId>… --up-to <seq>`. Each sends the item's snapshot as `expect` from a fresh `review.get` unless `--expect` is given, so a stale view is caught. `--idem-key`/`FIN_CALL` as for every write.
25. **Console:** `finisherd review policy show --project <id> [--json]` and `finisherd review policy set --project <id> [--protect <branch>]… [--unprotect <branch>]… [--require-review on|off] [--auto-merge on|off]` (TTY and typed code; `--auto-merge on` is refused with `AUTO_MERGE_UNAVAILABLE` before any code).
26. **Codes:** exit 3 `REVIEW_NOT_FOUND`, `REVIEW_UNDECIDED`; exit 4 `REVIEW_STALE`, `REVIEW_HAS_CONFLICTS`, `REVIEW_CLOSED`, `REVIEW_LIMIT`, `AUTO_MERGE_UNAVAILABLE`; exit 5 `POLICY_BUSY`; exit 7 `BRANCH_PROTECTED` (5A reserved it); merge reasons `DROPPED`, `notIncluded`. Audit kinds `history.drop {item?, count}`, `review.merge {item, protected, auto}` (for protected targets) and the new `review.policy {old, new}`.

## Owner decisions this plan implements

- **Spec §12 decision 1:** review items live in `control.db` of the server that holds the project; no `.finisher/review/` files; rejections are durable as `drop` records; open items are re-derived after a move (Decisions 8, 22).
- **Decision 5:** `local` keeps `branch.protected` and `review.decide`: `fin` still writes `main` without a review, and the e2e scenario uses `local` as the reviewer (Decision 16).
- **Decision 6:** human-only is advisory against raw-shell agents until 7C: an unsandboxed agent can use `local.key` (which holds `review.decide`) or `console.key` (which can change the policy). Protected branches bind agents that use their credentials; the README's security model says so.
- **Auto merge under a per-project owner policy** (agent §20 item 4): off by default, human-only, refused until sub-project 10 (Decision 19).

## Review Focus

1. **A dropped origin is never copied by any merge into the target or its later descendants,** whatever the order of drops, undrops, merges and imports, on a writer and a follower. → Task 2 `prop_dropped_never_copied`, `prop_writer_and_follower_agree`.
2. **A review merge never copies a record outside what was reviewed:** `include` and `sourceUpTo` inside the write transaction; a commit between planning and the write is not included. → Task 3 `commit_between_plan_and_write_not_included`, `prop_include_bound`.
3. **Format safety:** every non-`drop` record byte-identical; a 4C fixture opens with identical state; a 4C reader is `NEWER_FORMAT`, never wrong. → Task 2 goldens.
4. **Pending is never accept outside the snapshot;** `requireReview` refuses pending. → Task 7.
5. **Protected branches hold at both checks;** `local` keeps its reach; agents do not. → Task 6 `agent_commit_to_main_refused_at_stage_and_in_txn`, `rename_between_policy_and_write_caught`.
6. **No path to an auto merge from any RPC,** and every check of the path refuses as specified. → Task 9 `no_rpc_reaches_auto_merge`.
7. **`review.verdict` is what 7A's tracker expects.** → Task 4 `kind_version_for_agent_is_one`.
8. **Review text never leaks** into the audit, the journal (outside training-only `reason`) or errors. → Tasks 7, 11.

---

### Task 1: The `drop` record kind and what is dropped — tier: opus

**Files:**
- Create: `crates/fin-core/src/history/drops.rs`.
- Modify: `crates/fin-core/src/record/{mod.rs (`Drop`, `drops`, `undrops`, `record_format`), link.rs (`Link::Drop`)}`, `crates/fin-core/src/history/{candidates.rs (dropped origins → `DROPPED`), merged.rs (`left_to_merge`), mod.rs (`LogView` gains `drops_on(lineage)` with a default that reads records)}`, `crates/fin-core/src/rebase/conflict.rs` (`reasons::DROPPED`), `crates/fin-core/src/lib.rs`, the regenerated types, `TASKS.md`.
- Test: `crates/fin-core/tests/drops.rs`, `crates/fin-core/tests/record.rs` (extended).

**Interfaces:**
```rust
pub enum RecordKind { /* … */ BranchSet, Drop }
pub fn record_format(kind: RecordKind) -> (u32, u32);            // (3, 3) for Drop, (2, 2) otherwise
pub enum Link { /* … */ Drop { from: String, drops: Vec<String>, undrops: Vec<String> } }
pub mod drops {
  /// Origin roots dropped for the branch whose lineage is `lineage` (Decision 2).
  pub fn dropped_for(log: &dyn LogView, lineage: &Lineage) -> Result<BTreeSet<String>, CoreError>;
}
```
Decisions 1–3.

- [ ] **Step 1: Write failing tests:** `drop_record_golden` (the bytes of a `drop` record, field order `…, recovers, drops, undrops`), `other_kinds_byte_identical` (every 4B golden unchanged), `drop_needs_one_non_empty_list`, `id_in_both_lists_invalid`, `newest_record_wins`, `child_inherits_before_fork_only`, `undrop_restores_candidate`, `dropped_is_excluded_with_reason`, `dependent_of_dropped_is_depends_on_skipped`, `cherry_pick_ignores_drops`, `left_to_merge_skips_dropped` (a branch whose rest is merged or dropped is `merged`), `drop_is_no_undo_unit`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(core): drop records and dropped origins in merge planning`.

### Task 2: Format 3, the `drops` index and `Store::drop` — tier: opus — after Task 1

**Files:**
- Create: `crates/fin-store/src/history_ops/drop.rs`, `crates/fin-store/src/migrations/m0004_drops.rs`, `crates/fin-store/tests/drops.rs`, `crates/fin-store/tests/format_v3.rs`.
- Modify: `crates/fin-store/src/segment.rs` (`version_for`, `version_for_branch`, `READER_VERSION = 3`), `crates/fin-store/src/{derived.rs, rebuild/*, import/*}` (the index maintained and rebuilt), `crates/fin-store/src/history.rs` (`SqlLog` serves `drops_on` from the index), `crates/fin-store/src/wire.rs` (`DropReq`), `crates/fin-node/src/history.rs` (`drop`), `packages/fin-node` (typings), the regenerated types, `TASKS.md`.
- Test: the files above; a 4C-era fixture project (checked in under `crates/fin-store/tests/fixtures/`, made by the 4C build beside `fixture_4b.rs`'s) opens with identical state.

**Interfaces:**
```rust
#[derive(Deserialize)] #[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DropReq { pub tx_id: String, pub author: String, pub into: String, pub from: String,
  #[serde(default)] pub drops: Vec<String>, #[serde(default)] pub undrops: Vec<String>,
  #[serde(default)] pub dry_run: bool, pub args_hash: Option<String> }
impl Store { pub fn drop(&mut self, r: DropReq) -> Result<CommittedTx, StoreError>; }
```
Decisions 4, 5.

- [ ] **Step 1: Write failing tests:** `segment_with_drop_is_v3`, `segment_without_drop_keeps_version`, `v2_reader_gets_newer_format` (`decode_with_reader(…, 2)`), `fixture_4c_opens_identically`, `drop_validates_origins_on_source`, `drop_is_idempotent`, `drop_respects_guard` (an `allowBranches` without the target refuses), `index_rebuilt_equals_maintained`, `prop_dropped_never_copied` (random histories with drops, undrops, merges into the target and its descendants, exports and imports in any order: no merge ever copies a dropped origin), `prop_writer_and_follower_agree` (`derived::compute` and `dropped_for` equal on a writer and a follower that imported in another order), `merge_after_move_keeps_drop` (export, import into a fresh server data, a plain merge leaves the origin out).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(store): format-3 segments for drop records, the drops index and Store::drop`.

### Task 3: Merge limits and the review plan — tier: opus — after Task 2

**Files:**
- Create: `crates/fin-core/src/history/{limits.rs, review.rs}`, `crates/fin-store/src/history_ops/review_plan.rs`.
- Modify: `crates/fin-core/src/history/merge.rs` (call-site lines: the limits and the explain sink), `crates/fin-store/src/history_ops/merge.rs` (`MergeReq.include`, `source_up_to`), `crates/fin-store/src/wire.rs` (`MergeReport.not_included`, `ReviewPlan`), `crates/fin-node/src/history.rs` (`reviewPlan`), `packages/contracts/src/history.ts` (`HistoryMergePayload` gains `include`, `sourceUpTo`), `crates/fin/src/commands/history.rs` (`--include`, `--up-to`), the regenerated types, the RPC fixtures, `TASKS.md`.
- Test: `crates/fin-core/tests/merge_limits.rs`, `crates/fin-store/tests/merge.rs` (extended), `crates/fin/tests/cli_history.rs` (extended).

**Interfaces:**
```rust
pub struct MergeLimits<'a> { pub include: Option<&'a BTreeSet<String>>, pub source_up_to: Option<u64> }
pub fn plan_merge_limited(log: &dyn LogView, from: &str, into: &str, author: &str, merge_id: &str,
  skip_conflicting: bool, limits: MergeLimits, explain: Option<&mut dyn FnMut(ReviewEntry)>) -> Result<MergePlan, CoreError>;
impl Store { pub fn review_plan(&self, from: &str, into: &str) -> Result<ReviewPlan, StoreError>; }
```
Decisions 6, 7.

- [ ] **Step 1: Write failing tests:** `include_copies_only_named_origins`, `up_to_bounds_source_branch_only`, `not_included_reported`, `included_depending_on_excluded_conflicts`, `empty_include_merges_nothing`, `commit_between_plan_and_write_not_included` (a commit lands on the source after the review plan and before the merge's transaction: not copied, reported nowhere as merged), `prop_include_bound` (no merge copies an origin outside `include` or a source record above `sourceUpTo`), `plan_statuses_match_merge` (every `willMerge` entry is what the merge with that include copies), `review_plan_on_follower`, `review_plan_truncates`, `cli_merge_include_and_up_to`.
- [ ] **Step 2: Implement** both sides and re-capture the RPC fixtures in this commit.
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(history): merges limited by include and sourceUpTo; the review plan`.

### Task 4: `review.verdict` v1 — tier: sonnet — can start now

**Files:**
- Create: `crates/fin-core/src/events/payloads/review.rs`.
- Modify: `crates/fin-core/src/events/{kinds.rs (`review.verdict` v 1 [Finisherd, Agent]), names.rs (the tables of Decision 20; `RPC_METHODS` for every 5E RPC), payloads/{mod.rs, reserved.rs (drop `ReviewVerdict`)}}`, the regenerated schema and `generated.ts`, `TASKS.md` (the diagnostics follow-up "Sub-project 10: review verdicts" moves here).
- Test: `crates/fin-core/tests/events.rs` (goldens), `packages/fin-node/test/journal-sources.test.ts` (extended).

**Interfaces:**
```rust
pub struct ReviewVerdict { pub item_kind: ReviewItemKind, pub verdict: Verdict, pub via: Option<VerdictVia>, pub by: VerdictBy,
  pub tx_ids: Vec<IdRef>, pub accepted: u32, pub rejected: u32, pub session_id: Option<IdRef>, pub open_s: u64,
  pub reason: Option<Content<UserText>> }   // training only
```
Decision 20.

- [ ] **Step 1: Write failing tests:** `review_verdict_v1_golden`, `kind_version_for_agent_is_one` (`kindVersion("review.verdict", "agent")` and `"finisherd"` answer 1; `"ui"` answers none), `tx_ids_capped_at_64`, `reason_is_training_only` (stripped from the upload view), `outcome_verdict_shape_decodes` (7A's `OutcomeVerdict` `{verdict, txIds: [id], by, via, itemKind: "none", sessionId, openS}` validates).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(events): review.verdict v1 for finisherd and agent sources`.

### Task 5: Contracts, the reviews store and `history.drop` — tier: sonnet — after Tasks 2 and 4

**Files:**
- Create: `packages/contracts/src/{reviews.ts, review-rpcs.ts}`, `apps/finisherd/src/control/migrations/reviews/0001.ts`, `apps/finisherd/src/reviews/{store.ts, hub.ts}`, `crates/fin/src/commands/drop.rs`.
- Modify: `packages/contracts/src/{rpc.ts (spread `REVIEW_RPCS`), history.ts (`history.drop`), scopes.ts (rules; resolver `review`; `RuleFacts.protectedTarget`; `RuleOf.protectedNeeds`), audit.ts (`review.policy`; details of `history.drop`, `review.merge`), index.ts}`, `apps/finisherd/src/history.ts` (`drop` op), `apps/finisherd/src/handlers.ts` (`history.drop`; stub review handlers until Task 7), `apps/finisherd/src/chain/resolve.ts` (the `review` resolver), `crates/fin/src/{output.rs, commands/mod.rs, registry.rs}`, `apps/finisherd/src/console/exit-codes.ts`, `TASKS.md`.
- Test: `packages/contracts/src/scopes.test.ts`, `apps/finisherd/test/reviews-store.test.ts`, `crates/fin/tests/cli_drop.rs`.

**Interfaces:**
```ts
export class ReviewStore {
  constructor(store: ControlStore, clock?: () => number);
  open(item: NewItem): ReviewItem;                         // REVIEW_LIMIT; one open branchReview per pair
  get(id: string): ReviewItem | undefined;
  list(projectId: string, q: { state?: ReviewState; kind?: ReviewKind }): ReadonlyArray<ReviewItem>;
  decisions(id: string): ReadonlyMap<string, Decision>;
  setDecisions(id: string, ds: ReadonlyArray<Decision>, by: string): void;
  setState(id: string, state: ReviewState, by: string, extra?: { mergeId?: string; snapshot?: Snapshot }): void;
  comment(c: NewComment): Comment;
  policy(projectId: string): BranchPolicy;                // the default when absent
  setPolicy(projectId: string, p: BranchPolicy): { old: BranchPolicy };
}
```
Decisions 8–10, 17, 23, 26.

- [ ] **Step 1: Write failing tests:** `review_rpcs_have_rules`, `history_drop_scopes_follow_protection`, `review_resolver_reach` (an item of another project is `REVIEW_NOT_FOUND`), `reviews_chain_migrates_once`, `one_open_review_per_pair`, `limits_and_text_rules`, `decisions_keyed_by_origin_survive_snapshot_change`, `policy_default_protects_main`, `cli_drop_and_undo` (a drop record on the target; `fin merge` leaves the origin out; `fin drop --undo` brings it back), `drop_audited_without_text`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(reviews): contracts, the reviews store and history.drop`.

### Task 6: Protected branches and the policy — tier: opus — after Task 5

**Files:**
- Create: `apps/finisherd/src/chain/protected.ts`, `apps/finisherd/src/reviews/{policy.ts, gate.ts}`.
- Modify: `apps/finisherd/src/chain/policy.ts` (`DEFAULT_POLICIES`), `apps/finisherd/src/chain/chain.ts` (`BRANCH_PROTECTED` → rule `protectedBranches` in `DENIALS` exists; the gate held from the policy stage to the answer for writes), `apps/finisherd/src/handlers.ts` (`review.policy`, `review.policy.get`), `TASKS.md`.
- Test: `apps/finisherd/test/protected.test.ts`, `apps/finisherd/test/policy-gate.test.ts`, `crates/fin-store/tests/guard.rs` (extended: `deny_branches` with a rename between check and write).

**Interfaces:**
```ts
export const protectedBranches: PolicyRule;
export class PolicyGate {                                  // per project
  shared<A, E, R>(projectId: string, f: Effect.Effect<A, E, R>): Effect.Effect<A, E, R>;
  exclusive<A, E, R>(projectId: string, f: Effect.Effect<A, E, R>, timeoutMs?: number): Effect.Effect<A, E | FinError, R>; // POLICY_BUSY
}
```
Decisions 16–19.

- [ ] **Step 1: Write failing tests:** `agent_commit_to_main_refused_at_stage_and_in_txn` (stage 5 refuses; with the stage bypassed in a test, the guard refuses inside the transaction), `rename_between_policy_and_write_caught`, `local_writes_main` (owner decision 5), `every_write_rpc_checked` (the matrix over `WRITE_RPCS` and `targets`), `branch_set_archive_of_main_refused_for_agent`, `review_merge_and_drop_need_review_decide_on_protected`, `policy_write_is_human_only_and_confirmed` (`HUMAN_ONLY` for `agent` and `local`), `policy_audited_old_and_new`, `auto_merge_on_is_unavailable`, `policy_waits_for_in_flight_write`, `write_after_policy_change_sees_new_policy`, `policy_busy_after_30s`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(reviews): protected branches checked at the policy stage and in the write transaction`.

### Task 7: The review workflow — tier: opus — after Tasks 3 and 6

**Files:**
- Create: `apps/finisherd/src/reviews/{plan.ts, workflow.ts, verdicts.ts, rpc.ts}`.
- Modify: `apps/finisherd/src/handlers.ts` (spread `reviewHandlers`), `apps/finisherd/src/main.ts` (the reviews layer), `apps/finisherd/src/history.ts` (`committedOf` for drops), `TASKS.md`.
- Test: `apps/finisherd/test/reviews-workflow.test.ts`, `apps/finisherd/test/reviews-verdicts.test.ts`.

**Interfaces:**
```ts
export class Reviews {
  request(p: Principal, req: ReviewRequest): Effect.Effect<ReviewView, FinError>;
  refresh(p: Principal, itemId: string): Effect.Effect<ReviewView, FinError>;
  get(itemId: string): Effect.Effect<ReviewView, FinError>;              // change list, stale
  decide(p: Principal, req: ReviewDecide, guard: WriteGuard): Effect.Effect<ReviewView, FinError>;
  merge(p: Principal, req: ReviewMerge, guard: WriteGuard): Effect.Effect<ReviewMerged, FinError>;
  close(p: Principal, req: ReviewClose, guard: WriteGuard): Effect.Effect<ReviewView, FinError>;
  watch(projectId: string): Stream.Stream<ReviewWatchItem, FinError>;
  observeMerge(projectId: string, from: string, into: string, outcome: MergeOutcome): void;   // Task 8
}
```
Decisions 9, 12–15, 20.

- [ ] **Step 1: Write failing tests:** `request_plans_snapshot`, `request_again_returns_existing`, `stale_expect_is_review_stale_with_fresh_list`, `get_reports_stale_on_head_or_hash_move`, `reject_writes_drop_at_once` (durable before the item changes), `unreject_writes_undrop`, `decisions_survive_refresh`, `pending_never_accept_outside_snapshot` (a commit after the snapshot is not merged with `acceptPending`), `require_review_refuses_pending`, `conflicts_refuse_merge_and_write_nothing`, `merge_archives_finished_agent_branch`, `merge_keeps_unfinished_or_human_branch`, `target_moved_merges_with_flag`, `close_rejected_drops_every_unmerged_entry`, `close_dismissed_writes_nothing`, `closed_item_refuses_writes`, `changes_requested_published_and_reopened`, `merge_is_idempotent_on_idem_key`, `watch_snapshot_first_and_closed_on_revocation`, `verdict_entries_per_call` (by, counts, `openS`, ≤ 64 ids each, `reason` only when consent allows training), `review_text_never_audited_or_journalled`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(reviews): branch reviews with per-transaction decisions, durable rejections and reviewed-only merges`.

### Task 8: Conflict items, re-derivation and plugin suggestions — tier: sonnet — after Task 7

**Files:**
- Create: `apps/finisherd/src/reviews/{conflicts.ts, rederive.ts, suggestions.ts}`.
- Modify: `apps/finisherd/src/history.ts` (hand `history.merge` outcomes to `Reviews.observeMerge`; call-site lines), `apps/finisherd/src/projects.ts` (call re-derivation after a project loads; call-site lines), `apps/finisherd/src/plugins/lifecycle.ts` (`suggest` writes an item; installing supersedes), `apps/finisherd/src/main.ts` (the suggestion move after every chain migrated; re-derivation after `control.reset`), `TASKS.md` (the 8A follow-up ticked; a new follow-up to drop `plugin_suggestions` in a later `plugins` migration).
- Test: `apps/finisherd/test/reviews-conflicts.test.ts`, `apps/finisherd/test/reviews-rederive.test.ts`, `apps/finisherd/test/plugins-lifecycle.test.ts` (`suggest_records_plugin_suggestion` now expects an item).

Decisions 11, 21, 22.

- [ ] **Step 1: Write failing tests:** `conflict_opens_item`, `second_conflict_updates_item`, `successful_merge_supersedes`, `archived_source_supersedes`, `review_merge_conflict_goes_through_observer`, `tx_commit_rebase_conflict_opens_nothing`, `rederive_on_load_for_active_agent_branches` (not for archived, merged, human-authored or already reviewed branches; at most 64 per load), `rederive_after_control_reset`, `rederive_is_idempotent`, `suggestion_is_item`, `suggestion_rows_moved_once` (a crash between insert and delete moves nothing twice), `install_supersedes_suggestion`, `decide_on_suggestion_is_invalid_request`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(reviews): merge conflicts and plugin suggestions as review items; re-derived reviews`.

### Task 9: The internal auto-merge path — tier: opus — after Task 7

**Files:**
- Create: `apps/finisherd/src/reviews/auto-merge.ts`.
- Modify: `apps/finisherd/src/reviews/policy.ts` (`AUTO_MERGE_AVAILABLE = false`), `apps/finisherd/src/diagnostics.ts` (the `policy` actor's `by: policy`, `via: autoMerge`), `TASKS.md` (the sub-project 7 line "Auto merge needs sub-project 10" notes what exists).
- Test: `apps/finisherd/test/reviews-auto-merge.test.ts`.

**Interfaces:**
```ts
export interface VerdictSource { verdicts(itemId: string, snapshot: Snapshot): ReadonlyArray<{ kind: "qa" | "critic"; pass: boolean; verdictId: string }> }
export interface AutoMergeDeps { reviews: Reviews; store: ReviewStore; gate: PolicyGate; verdicts: VerdictSource; available?: boolean; audit: Audit }
export type AutoMergeRefusal = "unavailable" | "policyOff" | "requireReview" | "verdictMissing" | "verdictFailed" | "snapshotMismatch";
export function tryAutoMerge(itemId: string, session: string | undefined, deps: AutoMergeDeps): Effect.Effect<{ merged: true; mergeId: string } | { merged: false; reason: AutoMergeRefusal }>;
```
Decision 19.

- [ ] **Step 1: Write failing tests** (fake verdicts, `available: true` injected): `nothing_merges_when_unavailable` (the default), `policy_off_refuses`, `require_review_refuses`, `missing_verdict_refuses`, `failed_verdict_refuses`, `older_snapshot_verdict_refuses`, `policy_turned_off_in_flight_refuses_or_completes_first` (the gate), `merges_with_both_verdicts_passing` (bounded by the judged snapshot's `sourceUpTo`; audited `auto: true` with `enabledBy`; verdict `by: policy`, `via: autoMerge`), `item_stays_open_on_refusal`, `no_rpc_reaches_auto_merge` (every RPC tag with every principal kind and the most permissive payloads the schemas allow never calls `tryAutoMerge`, checked with a spy).
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates → PASS. **Commit** `feat(reviews): the internal auto-merge path, unavailable until sub-project 10`.

### Task 10: `fin review` and the console — tier: sonnet — after Tasks 7 and 8

**Files:**
- Create: `crates/fin/src/commands/review.rs`, `apps/finisherd/src/console/commands/review.ts`.
- Modify: `crates/fin/src/{commands/mod.rs, registry.rs, main.rs (call-site lines)}`, `apps/finisherd/src/console/main.ts` (`COMMANDS`, `HELP`), `TASKS.md`.
- Test: `crates/fin/tests/cli_review.rs`, `apps/finisherd/test/console-review.test.ts`.

Decisions 24, 25.

- [ ] **Step 1: Write failing tests:** every `fin review` subcommand's JSON and exit codes (`REVIEW_STALE` 4, `REVIEW_UNDECIDED` 3, `BRANCH_PROTECTED` 7), `expect_taken_from_fresh_get`, `fin_review_decide_as_local_works` (owner decision 5), `fin_review_merge_as_agent_is_scope_denied`; console `review policy show|set` text and `--json`, `CONSOLE_NEEDS_TTY`, the typed code, `--auto-merge on` refused before any code.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Run** the gates and `rtk pnpm e2e` → PASS. **Commit** `feat(fin,console): fin review, fin drop and the review policy console`.

### Task 11: e2e `review` scenario — tier: sonnet — after Task 10

**Files:**
- Create: `scripts/e2e/review.mjs`.
- Modify: `scripts/e2e.mjs` (`SCENARIOS.review`), `scripts/e2e.test.mjs`, `scripts/e2e/lib.mjs`.

- [ ] **Step 1: Write the scenario:**
  1. Start `finisherd`; `fin project create`; mint an agent credential with the mint defaults and `--branch own` (through `consoleRpc`).
  2. As the agent: `fin branch create`, four edits on it; an edit on `main` → `BRANCH_PROTECTED` (exit 7); `fin review request --title …`.
  3. As `local`: `fin review show` lists four entries; reject one, accept two, leave one pending; `fin review merge` without `--accept-pending` merges two; the rejected one is a `drop` record on `main`.
  4. The agent commits a fifth edit; `fin review merge --accept-pending` on the old snapshot is `REVIEW_STALE`; after `refresh`, merging with `--accept-pending` brings the pending one and the fifth, not the rejected one.
  5. A plain `fin merge --from <agent branch>` leaves the dropped origin out (`DROPPED` in `excluded`).
  6. A merge that conflicts opens a `mergeConflict` item; a later successful merge supersedes it.
  7. **Project move:** close the project, start a second server data, open the folder there: the drop still holds (a plain merge leaves it out), and the active agent branch gets a re-derived item.
  8. `finisherd review policy set --auto-merge on` (through `consoleRpc` with a code) → `AUTO_MERGE_UNAVAILABLE`; a `review.policy` write from the agent → `HUMAN_ONLY`.
  9. `review.verdict` entries in `events/` carry `by`, counts and no text (training is off); the audit has `history.drop` and `review.policy` rows without titles or comments.
- [ ] **Step 2: Run** `rtk pnpm e2e` (all scenarios) → PASS. **Commit** `test(e2e): review scenario with an agent credential, local reviewer, drops and a project move`.

### Task 12: Spec as built, architecture pointer, README and tracker — tier: sonnet — after Task 11

**Files:** `docs/specs/2026-11-07-sub-project-5-control-plane-design.md` (§8 rewritten **as built**, with a §8.8 mapping every Decision of this plan to As built or Deviation, §8.9 owner decisions and review fixes, §8.10 residuals; §3.1, §3.3 and §3.4 updated), `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` (§4.2's field order and the `drop` kind; §4.4's `include`/`sourceUpTo` and `DROPPED`; a pointer to sp5 §8.3), `docs/specs/2026-11-01-finisher-architecture-design.md` (a §27 item "Sub-project 5E as built"; §6.4 and §11.3 point at §8), `README.md` ("Reviews": requesting, deciding, merging, `fin drop`, protected branches and the console policy; "Security model": what protected branches bind before 7C), `TASKS.md`.

- [ ] **Step 1:** Write the as-built notes from the code; list every deviation (at least Decision 3's `excluded`, and Decision 15 if the owner confirms it).
- [ ] **Step 2: TASKS:** tick the 5E lines, the 8A follow-up and the 7A Task 11 note ("5E registers `review.verdict` v1"); add follow-ups: drop `plugin_suggestions` in a later `plugins` migration; sub-project 10 records `review_verdicts` and flips `AUTO_MERGE_AVAILABLE`; sub-project 7 delivers `changesRequested` as a turn.
- [ ] **Step 3: Run** the gates → PASS. **Commit** `docs: record sub-project 5E as built`.

### Task 13: Cross-platform validation (controller-run; needs hosts) — tier: opus (controller)

On macOS, Windows 11 and the Linux VM; host details in `archive/hosts.md`.

- [ ] **Step 1:** `cargo test --workspace`, `pnpm test`, `pnpm gates`, `pnpm e2e` on each OS.
- [ ] **Step 2: Mixed versions on a shared folder** (the 4A lease setup, SMB between two OSes): a 5E server writes a `drop` record; a 4C-era build opens the project read-only with `NEWER_FORMAT` and changes nothing; the 5E server keeps writing.
- [ ] **Step 3: Followers:** a follower on another OS imports the drop and merge records in another order and derives the same `merged` state and the same review plan.
- [ ] **Step 4: Measure** and record (TASKS and spec): `review_plan` and a limited merge on a branch of 1,000 records (p50/p99), `review.get` from the cache and after a miss, `dropped_for` on a deep lineage.
- [ ] **Step 5:** Fix what fails (one commit each, with a test); re-run Step 1.

---

## Questions for the owner

1. **Re-derived reviews (Decision 22):** after a project arrives on another server, or after `control.db` is reset, should every active agent-authored branch that still has something to merge into its parent get a review item automatically (marked as re-derived), even if its agent never asked for one there? Recommended: yes; it is the only way to "re-derive open items" without review files in the project. The alternative is to re-derive nothing and leave the agent or the person to request reviews again.
2. **Merging a stale review (Decision 14):** when `main` has moved since the person reviewed, may `review.merge` still go ahead? Only the reviewed transactions are copied, and a conflict refuses it. Recommended: yes, with `targetMoved: true` in the result. The alternative is to require a refresh first (`REVIEW_STALE`) whenever the target moved, always or only for protected targets.
3. **"Reject all" (Decision 15):** should closing a review as rejected drop every transaction it would still merge, accepted ones included? Recommended: yes, so that a later plain `fin merge` cannot bring in what the person rejected as a whole. The spec's wording drops only the pending ones.
4. **Plugin suggestions (Decision 11):** suggestions become review items that a person dismisses or rejects, and installing the plugin supersedes them. The old `plugin_suggestions` table is emptied at start and dropped in a later release, not by this plan. Recommended as written.
