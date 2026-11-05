# Sub-project 4B: History — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task names a **model tier** for its implementer: `opus` for semantics (formats, rebase, merge, undo, import), `sonnet` for plumbing (fixtures, bindings, RPC, CLI, docs).

**Goal:** Edits have history:
- A project has branches (`main`, then `br<n>`), each derived from the log so it syncs through the folder.
- Undo and redo append records and never truncate.
- Merge, cherry-pick and revert re-run intent through a stronger rebase engine.
- Checkpoints, `try` branches and archiving are available.
- Quarantined records come back as `recovered-*` branches.

Everything is driven and tested through `fin` → `finisherd`.

**Architecture:**
- **`fin-core`** gains:
  - record kinds with typed links;
  - exact per-track shift maps with ProseMirror-style mirrored mappings;
  - per-entity conflict footprints (one function per op module);
  - a rebase engine that maps positions op by op and re-runs each op at the head with full policy;
  - a branch index derived from records;
  - undo-stack derivation;
  - **pure history planners** (`undo`, `redo`, `undo --to`, `merge`, `cherry-pick`, `revert`) over a read-only `LogView` trait. That makes the property tests in-memory and fast, and the logic stays WASM-safe.
- **`fin-store`** gains:
  - segment format v2, written only when a segment holds a 4B kind;
  - per-kind reader versions;
  - DB migration 0002 (derived tables `branches` and `record_links`);
  - one shared write path, `Store::write`, used by commit and every history operation (dry runs also work on a follower);
  - lineage-aware reads;
  - branch-aware export (parents first) and import;
  - rewinds and diverged tails that cascade to descendant branches;
  - branch-aware verify, rebuild and compaction;
  - a `LogView` over SQLite;
  - recovered branches with remapped ids.
- **`fin-node`, `@finisher/contracts`, `finisherd`** get branch and history RPCs. Each committed record is published to its branch's hub.
- **`fin`** gets `branch`, `merge`, `cherry-pick`, `revert`, `undo`, `redo`, `checkpoint`, `try`, an extended `project recover`, and a global `--branch`.

**Tech Stack:**
- Rust 2024: rusqlite 0.37 bundled, `blake3`, `ulid`, `proptest` (dev). No new crates.
- Node 26, Effect 4, vitest.

**Spec:** `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`, §4. The plan also depends on §2, §3.2–3.4, §3.6, §3.7, §3.9, §3.10, §6 and §8. Parent: `docs/specs/2026-11-01-finisher-architecture-design.md` §6.3–6.6 and §9.8.

**Branch:** `sp4b-history`, stacked on `sp4a-durable-storage`. 4A Task 19 (cross-host) still gates 4A's merge. Task 23 here re-runs the relevant rows with 4B on top.

## Global Constraints

**Code**
- MIT. No GPL/LGPL/AGPL/SSPL/EUPL in shipped code (`pnpm gates` runs cargo-deny and the JS licence gate).
- **`fin-core` is pure:** no I/O, clocks or randomness. It must build for `wasm32-unknown-unknown` (part of `pnpm gates`).
- **Primitive ops live only in `fin-core`, one module per op.** Each op's footprint and positions live in its own module under `crates/fin-core/src/ops/`.
- **TypeScript model types are generated, never hand-written.**
  - Every new wire type derives `JsonSchema` and is added, in the task that creates it, to `crates/fin-store/examples/export_schema.rs`.
  - Then run `pnpm --filter @finisher/core-types generate`. The drift gate is `scripts/check-generated-types.mjs`.
- **File sizes:** non-test source files ≤ 600 lines, tests ≤ 1,500.
  - Already near the limit: `import/mod.rs` 600, `export/mod.rs` 590, `tick.rs` 587, `import/apply.rs` 586, `db.rs` 557, `status.rs` 530.
  - Task 1 makes room. Later tasks add new modules rather than growing those files.
- Errors are `{ code, message, details, fix? }`. Every new code goes into `crates/fin/src/output.rs` with the exit code this plan gives.

**Formats and migrations**
- **Migrations are frozen once shipped.**
  - `m0001_records.rs` and its pinned output (`migration_0001_output_is_pinned`, the golden hash) never change.
  - The one allowed edit to `tests/migrations.rs` is in how `migration_0001_matches_encode_record` *constructs* its `Record`: it switches from a struct literal to `Record::new_edit(..)`, because new fields would break the literal. Its expectations stay as they are.
  - m0002 writes literal values (`"2"`), as m0001 writes `"1"`, and never uses current constants.
  - m0002 never runs `ALTER TABLE`. It only creates tables, so the fresh schema (`db::SCHEMA`) and the migrated one stay byte-identical (`fresh_and_migrated_schemas_match`).
- **Stored bytes are never re-encoded.** An `edit` record written by 4B is byte-identical in shape to 4A's: new record fields are `skip_serializing_if = "Option::is_none"`.
- **Segment files** (Decision 4):
  - A segment or pack whose records are all `edit` is written as `formatVersion: 1, minReaderVersion: 1`. One that holds any 4B kind is written as `2/2`. The version is derived from content, so a re-export is byte-identical.
  - Readers accept both versions.
  - Snapshot files, compaction manifests and `finisher.project.json` stay at format 1. Each file kind is checked against its own reader version.

**Testing and process**
- **Property tests run inside `cargo test`.** All 4B properties together take under 30 s in a debug build on the Linux VM.
  - Each `proptest!` block sets `ProptestConfig { cases, .. }` explicitly and honours a `PROPTEST_CASES` override.
  - The budget is measured first in Task 6, then in Task 13, then in full in Task 20.
- **Before each commit, every task runs** `rtk cargo test --workspace`, `rtk pnpm test` and `rtk pnpm gates`. Tasks that touch the TS side or `fin` also run `rtk pnpm e2e` once at the end.
- Commits use Conventional Commits, author `mvhappy <333925574+mvhappy@users.noreply.github.com>`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Prefix shell commands with `rtk`, after `source .superpowers/env.sh` (Node 26).
- **The repo is public:** no host names, IPs, user names or key names in tracked files, fixtures included (Task 2 scrubs and checks them). Cross-host details stay in `archive/hosts.md` (git-ignored).
- **Tracker:** each task ticks its line in `TASKS.md` under a new section "Sub-project 4B: history", in the same commit.

## Decisions taken here (the spec leaves them open; Task 22 records them in the spec)

### Format and storage

1. **Kind fields are flat, optional record fields.**
   - They sit right after `kind`, in this order: `undoes, redoes, mergeId, from, origin, originHash, reverts, name, branchId, forkSeq, forkHash, archived`. Each is omitted when absent.
   - `RecordKind` serialises camelCase (`cherryPick`, `branchSet`; `edit` is unchanged).
   - A typed `Link` is read with `Record::link()`. A field set that does not match the kind is `INVALID_FORMAT`.
   - *Why:* edit records stay byte-identical to 4A.
2. **`undoes` is a list of unit ids,** newest first. `fin undo` writes one; `undo --to` writes many. *Why:* one `undo --to` record cancels several units, and one redo reverses it.
3. **A `branch` record also carries `forkSeq` and `forkHash`.**
   - `forkHash` is absent when `forkSeq` is 0.
   - The fork point is the requested seq (default: the parent's head before the branch record), and the record is appended to the parent at head + 1.
   - *Why:* `--from <b>@<seq>` can fork below the head, and the index must be derivable from the log alone.
4. **Segment version follows content** (owner decision).
   - Segments and packs holding only `edit` records are v1; any 4B kind makes them v2. Readers accept both.
   - A 4A follower of a 4B folder keeps following until a 4B kind is used. It ignores `br<n>` directories, which 4A already treats as "4B's".
5. **DB schema v2 (m0002)** creates two derived tables:
   - `branches`;
   - `record_links`: non-edit records, with kind, origin, `origin_hash`, from, mergeId, targets and name.

   It sets `meta.formatVersion` and `meta.minReaderVersion` to `"2"`. Both tables can be rebuilt from `txs`.
   - "Recovered" is not a table. A quarantine row counts as recovered while some `cherryPick` record in `txs` has `originHash` = its txHash (Decision 27). That makes it derivable on import and rebuild.
6. **Derived tables follow the log.**
   - `branches` and `record_links` are updated incrementally on commit and import.
   - Any row deletion (a rewind, a diverged tail, a cascade) runs `derived::recompute(txn)` in the same transaction. That deletes `record_links` rows whose txId is gone and recomputes the index, including `merged`.
   - A new verify check, `branch_index`, compares the tables with the log.
7. **Rewinds and diverged tails cascade to descendants.** This deviates from 4A rule (d), which refused.
   - **When:** the lost rows on a branch include the `branch` record of a child (equivalently, the child's fork point is not on the winning line).
   - **What:** the child, and recursively its descendants, are rewound whole. Exported rows go to quarantine as `SUPERSEDED` and unexported ones as `DIVERGED_TAIL`, each with the 4A origin rule. Their snapshots and `segments` rows are deleted, their index rows removed, their generations bumped, and a `Rewound` event is queued per branch.
   - A child whose branch record survives is untouched.
   - **Why:** followers must converge whatever the arrival order. Refusing would bring back the permanent `CHAIN_FORK` that 4A step 6a removed.
   - `meta` `branchBase.*` is never written.
8. **A set-aside branch record takes its branch's folder segments with it.**
   - Import sees a segment under `segments/br<n>/` whose `branch` record (the one its header names, `branchRecord`, Decision 33) is a current quarantine row (`STALE_EPOCH`, `SUPERSEDED`, `DIVERGED_TAIL`, `MALFORMED`, `ID_COLLISION`) or an inert row. It quarantines every record of that segment with the same reason (origin `folder`, detail `branch record <hash12> set aside`; an inert record: `ID_COLLISION`, or `MALFORMED` when the index refused it), recursively for grandchildren.
   - A directory whose branch record is unknown waits instead: `ImportReport.unknown_branches`, status `unknownBranches`, never read-only.
   - Recover groups include these records (Decision 27). Nothing is dropped, and nothing is silent.
9. **Lineage-aware reads.**
   - `ops_since`, `txs_since`, `state_at`, `hash_at` and `refresh` on a branch below its fork read the ancestors.
   - Each ancestor read calls `diffs::fill` on that ancestor when its diffs are pending.
   - `refresh` falls back to the branch's `fork_hash` as the tip when the head is `fork_seq` (`lib.rs` `refresh` today uses `db::hash_at(branch, rev)`, which is `None` there).
10. **Export runs parents before children,** in index order (depth, then numeric id). `rows::pending_branches` orders by name today, and `"br1" < "main"`. A follower therefore never sees a child segment before its parent's branch record from the same writer.

### Branches and names

11. **Names.**
    - Branch display names match `[A-Za-z0-9._-]{1,64}`, must not start with `-` or `.`, and must not be `main` or look like an id (`br\d+`).
    - They are unique, case-insensitively, among **non-archived** branches. `merged` branches keep their names.
    - Checkpoint names follow the same rules, must not be all digits, and are unique per branch.
    - A bad name is `INVALID_NAME` (exit 3, reused); a taken name is the new `NAME_TAKEN` (exit 3).
    - Duplicates that arrive through import: the numerically lower id keeps the name (`br9` beats `br10`), and the others list as `<name>#<id>`.
    - An archived branch is addressed by id or `<name>#<id>`. Restoring one whose name is taken needs a new name: `fin branch archive <b> --restore --as <new>`. Otherwise it is `NAME_TAKEN` with that `fix`.
12. **Branch references** in every API are an id, an active display name or `<name>#<id>`. The store resolves them, and stored intents and records hold the id.
13. **`main` cannot be renamed or archived** (`INVALID_OP`). **An archived branch refuses writes** with `BRANCH_ARCHIVED` (exit 3, `fix`: `fin branch archive <b> --restore`).
14. **Branch creation stores the parent's state at the fork** as `snapshots(child, forkSeq)`. The child's head is `forkSeq`, and its first record's `parentHash` is `forkHash`.
15. **Import learns branches from the branch records it applies, in the same pass** (a worklist, parents first).

### Rebase

16. **The rebase engine is a mapping with mirrors** (ProseMirror `Mapping` semantics).
    - **Maps:** each record's `Diff.shifts` is its map. The inverse of `Shift {from, by}` is `{from: from + by, by: -by}`.
    - **Recover rule:** a position in a removed range `[s, e]` collapses to `s` and is recorded as deleted with `offset = pos − s`, *except* at the edge on the assoc side: `e` for `After`, `s` for `Before`. So with `After`, a position at the start edge of our own earlier insert is recorded with offset 0 and resumes at `A'.from + 0`. That keeps "insert before our own earlier insert" in order.
    - **Mirrors:** recovery goes through a mirror only when the two shifts are on the same track and have equal `|by|`. Mirrored maps pair shifts in reverse order, so each side is one op's map (one inverse `StepMap` per earlier op of ours).
    - **Without a mirror** (ProseMirror `deletedAfter` / `deletedBefore` / `deletedAcross`): only a position strictly inside `(s, e)` is `Unmapped::Deleted`. One at the deleted edge (`s` with `After`, `e` with `Before`) is a boundary, not an entity reference (§4.3), and carries on from `s`, as in 4A: an insert at the end of a clip whose neighbour was deleted still maps.
    - **Mapping order:** ops are mapped one at a time over `[inverse(our earlier ops), intervening steps, our rebased earlier ops]` and re-run at the head with full policy. The 4A "blind after our own trim/delete" conflict is gone.
17. **Conflict reasons are codes:** `REMOVED`, `SAME_EDGE`, `SAME_PARAM` (4C), `DEPENDS_ON_SKIPPED`, `ALREADY_PRESENT`, `ID_COLLISION`, `UNDOES_TARGET_RECORDS`, or the policy code a re-run raised.
    - `CONFLICT` (exit 4) keeps the 4A detail fields, taken from the first conflict, and adds `details.conflicts: [{txId, opIndex, reason, againstTx, fix}]`.

### History operations

18. **Merged and cherry-picked copies keep the source's entity ids,** re-run with the source bindings as `BoundIds` and full policy. A `DUPLICATE_ID` on re-run is:
    - `ALREADY_PRESENT` when the existing entity was bound by a record with the same origin chain;
    - `ID_COLLISION` otherwise.

    `clip.restore` gains `$placeholder` support for its clip id (spec §4.3). Recovery (Decision 27) uses it.
19. **Merge candidates** are the content records on the source's lineage after the common point C of the two lineages. Excluded:
    - records whose txId is already the origin of a `merge` or `cherryPick` record on the target's lineage after C;
    - `cherryPick` records whose origin is on the target's lineage;
    - **source `undo`, `redo` and `revert` records whose unit is a merge group whose records originate on the target.** These are reported in `MergeReport.excluded` with `UNDOES_TARGET_RECORDS`: re-running them would undo the target's own edits.
20. **Merge records.**
    - One `merge` record per candidate, sharing the `mergeId`, which is the caller's `txId`. Record txIds are `<mergeId>-<n>`.
    - The copies' `intent.author` and record author are **the merging author** (spec §4.5: units of the merging author on the target). The source author survives only through `origin`.
    - Without `--skip-conflicting`, planning continues past a conflict as if that record were skipped, so the full list comes back.
21. **`merged` state:**
    - A branch B is `merged` when some branch T holds a `merge` record with `from = B`, and every content record of B's own (seq > `forkSeq`) is the origin of a `merge` or `cherryPick` on T's lineage.
    - It is recomputed by `derived::recompute` and after each merge or commit, on the holder and on followers alike.
    - A new edit on B makes B `active` again.
22. **Undo stacks.**
    - **Scope:** the branch's own records (seq > `forkSeq`) for one author.
    - **Dropping:** a unit is dropped (with everything below it on the undo stack) only when one of its records is the origin of a **`merge` record with `from = this branch` on a branch that is not a descendant of this branch**. Cherry-picks never drop (arch §6.4: "when the branch is merged").
    - **`undo --to`:** with a checkpoint at or before the fork, it undoes all of the author's own records on the branch.
23. **Redo is a unit** (ProseMirror-like).
    - A `redo` record is pushed on the undo stack as a unit of its own: it stands for the unit it recreated.
    - The next `undo` takes the redo record's stored `inverse` at the redo record's frame and writes `undoes: [<redo txId>]`.
    - *Why:* it is exact without mapping through the undo/redo pair. This deviates from the spec's wording "push the original unit back"; the effect is the same.
24. **Mirrors** pair:
    - an `undo` with the units it undoes;
    - a `redo` with its `undo`;
    - a merge or cherry-pick copy with its inverted original.

    Mirrored pairs are excluded from entity-conflict checks.

    For a stale commit the pairs inside the interval are wired in Task 6 (`rebase::steps_from`, from `CommittedTx.link`): an undo whose units are each one record in the interval is cut into one part per unit, each mirroring its unit; a redo mirrors its undo when that undo is one unmirrored step there. Merge units and `revert` (Tasks 13–14) still stand alone until those tasks extend `steps_from`.
25. **Author.** Stacks are keyed by `Tx.author`. `fin` sends `$FIN_AUTHOR` when set and not empty, else `"fin"`. Every history RPC carries `author`.
26. **History RPCs take `txId` and `dryRun`.**
    - `txId` is the idempotency key: the store dedupes on the first record's txId (`<key>-1` for a merge).
    - `fin --idem-key K` looks up `tx.get(K)`, or `tx.get(K-1)` for a merge, before sending, and sets `argsHash` (BLAKE3 of `[{cmd, args}]`, as for edits) on the first record's intent. The same key with another hash is `IDEM_KEY_REUSED`.
    - **`dryRun` works on a follower** (owner decision): it plans against the local DB in a transaction that is rolled back, with no read-only, lease or fence check. The transaction is IMMEDIATE, as for real writes: planning writes (ids, filled diffs, index rows), and a DEFERRED one that read first fails at once with `SQLITE_BUSY` when a leader beside the follower commits in between, where IMMEDIATE waits for the busy timeout.
    - Checkpoint, branch and branchSet records have `intent.ops == []`. Client `tx.commit` still refuses empty ops.
    - `CommittedTx` gains `link: Option<Link>`, omitted for edits.
27. **Recovered branches.**
    - **Groups:** current quarantine rows are grouped into `parentHash` chains. Each group forks at its last common hash with the row's branch line.
      - A group holding `branch` records gets child groups: the records of the set-aside child directories (Decision 8). Its `children` list their ids.
      - A group with no common history is listed but not created.
    - **Re-commit:** records go onto the new branch as `cherryPick { origin, from, originHash }` with txIds `<recoverId>-<n>`.
    - **Fresh ids:** each record is re-run with `$placeholders` for every id its original bound, allocated from `Counters`. A remap `old → new` is applied to the literal ids of later records in the group, their inverse-derived ops included.
      - A literal id that is neither remapped nor present at the fork state, but exists in the DB, is `ID_COLLISION`: listed, and the group is not created.
    - **Children:** each child group is created after its parent group, forked from the recovered parent at the copy of its fork record, or from the original parent when the fork is on the surviving line.
    - **Name:** `recovered-<server8>-<YYYYMMDD-HHMMSS>`. `<server8>` is the first 8 characters of the writer's server id (ours for `local`/`salvage` rows; the segment header's `writerId` for `folder`/`foreign` rows when the file is present), else `unknown`. The time is the group's first `committedAt`, in UTC. A name already taken gets `-2`, `-3`, ….
    - **Commands:** `fin project recover` lists groups; `--create <group|all>` makes branches; `--import <dir>` reads `.fseg` files moved out of the folder into quarantine (reason `IMPORTED`, origin `foreign`).
28. **Id counters are raised past quarantined ids too.** `Counters::raise_to_quarantine` scans the bindings and resolved-op ids of quarantine rows added since `meta` `quarantineIdsScannedThrough`, on every pass that added rows. This server therefore never hands out an id a set-aside record used (B5). Remapping in Decision 27 covers ids another server handed out first.
29. **`CHAIN_FORK` keeps the 4A manual procedure, plus `fin project recover --import <dir>`** (owner decision). In-band fork resolution (a new record kind honoured by chain resolution) is a TASKS follow-up.

### Interfaces and CLI

30. **New codes and exits:** `NAME_TAKEN` 3, `BRANCH_ARCHIVED` 3, `NOTHING_TO_UNDO` 3, `NOTHING_TO_REDO` 3.
    - An unknown tx, merge or checkpoint is `NOT_FOUND` with `details.kind` ∈ `tx | merge | checkpoint`.
    - `INVALID_NAME`, `INVALID_OP`, `BRANCH_NOT_FOUND`, `CONFLICT` and `IDEM_KEY_REUSED` are reused.
31. **RPC tags:** `branch.create`, `branch.list`, `branch.set`, `history.undo`, `history.redo`, `history.checkpoint`, `history.merge`, `history.cherryPick`, `history.revert`.
    - `project.recover` gains `txId`, `author`, `create`, `importDir` and `dryRun`, and answers `{groups, created, committed, imported, manual}`.
    - There is no new event type: branch changes reach clients through `status` events (`branches[]` gains `name`, `parent`, `forkSeq`, `state`).
32. **CLI.**
    - The global `--branch <b>` (default: the `fin-config.json` default, else `main`) replaces `fin project acknowledge --branch`.
    - `fin branch rename <old> <new>` is added.
    - `fin try` records each try branch in `<server-data>/fin-config.json` `tries: {<branchId>: <parentId>}`. `fin try keep` and `fin try drop` act only on the current default branch when it is listed there; otherwise they fail with `USAGE` (`fix: fin try`).
    - `try` names default to `try-<digits of the new branch id>`.

### Added during implementation

33. **Branch identity across servers** (Task 9b, from the review of Task 9; coordinator decision).
    - **`branch` records carry `epoch`**, the fence epoch at commit: a new optional record field (right after `forkHash`), part of the bytes and the hash, required on every `branch` record (`Link::Branch.epoch`; 4A never wrote one). A dry run writes 0 (it keeps nothing).
    - **Id collisions** (two servers across a takeover made the same `br<n>`, on the same parent or on different ones) are resolved by `(epoch desc, txHash asc)`: arrival-order independent, and it survives packing (record bytes, not segment labels). `BranchIndex::absorb` returns `Absorbed::{Made, Set, Replaced, Lost, Nothing}`; the index row keeps the holder's `record` (txHash) and `epoch` (new `branches` columns `record`, `epoch`; m0002 is not shipped, so its table text changed).
    - **The losing record is an inert row**, applied on its parent's chain (it has no ops; the index ignores it), so the parent keeps following. Only the loser's branch content goes: when it was applied, its subtree is rewound whole as `ID_COLLISION` (`cascade::subtree`, `Cause::voided("idCollision", …)`); its folder segments are classified `ID_COLLISION`. **Unfit `branch`/`branchSet` records** (a name the rules refuse, an unknown branch, a `forkHash` that is not the parent's record) are inert rows too. Inert records are marked in `record_links.ignored` (new column) and status warns `BRANCH_RECORDS_IGNORED`. `ID_COLLISION` quarantine rows are kept once set and current while the folder's verdicts set their record aside, like `SUPERSEDED` (re-review of 9b: the record that holds the id can itself be set aside later, and then the loser's branch comes back, its fork state restored after the recompute and resolved in the same pass); an unexported row of such a branch, which exists nowhere else, is a `DIVERGED_TAIL`.
    - **Every non-`main` segment header names its `branch` record**: `branchRecord: <txHash>` (packs keep it; compaction packs only segments naming the same record, and its view of a branch is the segments naming the index's record). Such segments are always format 2; decode refuses a branch segment without it, a `main` segment with it, and a format-1 segment with it. A branch directory's segments are classified by the record they name: the index's record → resolved with the branch; set aside or inert → quarantined with that row's reason; unknown → wait. This replaces Task 9's epoch-label rule (segment labels are upper bounds and packs relabel).
    - **A 4A reader** (reader version 1) lists `segments/br<n>/` files as `Newer` and goes `NEWER_FORMAT` read-only; it already does once the parent's segment holding the `branch` record (a 4B kind, format 2) arrives, so nothing changes for it.
    - **Entity ids collide the same way** (two servers across a takeover hand out the same `c<n>` on different branches): Tasks 13–14 (merge, cherry-pick) must treat a re-run's `DUPLICATE_ID` as `ID_COLLISION` unless the origin chain matches (Decision 18), and Task 16's remap covers recovered records.

## Questions for the owner

None open. The fork-resolution question was answered: manual move-out plus `--import`, with the in-band kind as a follow-up (Decision 29).

## Review Focus

1. **Byte stability across the upgrade.** 4A records and v1 segments are never rewritten, m0001's pins pass, and an edit-only segment stays v1. A v1 segment written by 4A before an upgrade crash is adopted. → Task 3: `edit_record_bytes_unchanged_from_4a`, `v1_segment_written_before_upgrade_is_adopted`, `edit_only_segment_is_v1`.
2. **Mirror edges.** Inserting before our own earlier insert keeps the order, and shifts of unequal size never mirror. → Task 4: `insert_before_own_earlier_insert`, `mirror_requires_equal_size`.
3. **Branch arrival order and set-aside parents.**
   - Child segments arriving first wait.
   - A child whose branch record loses is quarantined whole, on the holder and on every follower, in any arrival order.
   - → Tasks 9 and 10: `child_segments_before_branch_record_wait`, `rewind_cascades_to_child`, `cascade_converges_in_every_arrival_order`, `set_aside_branch_record_quarantines_child_segments`.
4. **Ids across takeovers.** A recovered tail never reuses an id the winner allocated. → Task 16: `recovered_tail_gets_fresh_ids_after_takeover`.
5. **A merge is all-or-nothing in the DB, and safe as a prefix for followers.** → Task 13: `merge_is_all_or_nothing_on_crash`, `follower_applies_partial_merge_prefix`.
6. **Undo, redo, undo again** stays exact, also across another author's edits. → Task 12: `undo_redo_undo_again`, `undo_rebases_over_other_authors_edit`.
7. **A rebuild from the folder alone** restores every branch, the index, merged state, checkpoints and the recovered marks. → Task 11: `rebuild_restores_branches_and_index`.

---

### Task 1: Make room (pure refactor) — tier: sonnet

**Files:**
- `crates/fin-store/src/db.rs` → `db/mod.rs` (connection, schema, meta, snapshots) + `db/records.rs` (`new_record`, `insert_tx`, `insert_imported`, `record_rows`, `parse_record`, `decode`, `find_tx`, `txs_since`, `replay_record`, `replay_row`, `kind_text`).
- `crates/fin-store/src/import/mod.rs` → move the per-branch loop of `Store::import` (from `for branch in db::branches(&txn)?` through the head adoption) into `import/pass.rs` as `fn resolve_branches(..) -> Result<PassOut, StoreError>`.
- `crates/fin-store/src/export/mod.rs` → move `prepare` and `adoptable` into `export/prepare.rs`.
- `crates/fin-store/src/import/apply.rs` → move `Ctx` and its helpers into `import/ctx.rs`.
- `crates/fin-store/src/status.rs` → move the warning builders (`quarantine_summary`, `rewind_warnings` and the other warnings) into `status/warnings.rs` (`status.rs` → `status/mod.rs`).

**Interfaces:** none new. All items keep their names and visibility (`pub(crate) use records::*;` and so on), so call sites don't change.

- [x] **Step 1:** Move the code with no logic changes. Doc comments move with their items.
- [x] **Step 2:** `rtk wc -l` the touched files: each must be ≤ 450.
- [x] **Step 3: Run** `rtk cargo test --workspace`, `rtk pnpm test`, `rtk pnpm gates`. Expected: PASS with the same test count (record it in the commit body).
- [x] **Step 4: Commit** `refactor(fin-store): split db, import, export, apply and status modules ahead of 4B`.

### Task 2: Scrubbed 4A fixture, generated from 4A code — tier: sonnet

**Files:**
- Create: `crates/fin-store/tests/fixtures/4a/folder/finisher.project.json`, `crates/fin-store/tests/fixtures/4a/folder/.finisher/segments/main/*.fseg`, `crates/fin-store/tests/fixtures/4a/project.fdb`, `crates/fin-store/tests/fixtures/4a/expected.json` (`{rev, project}`), `crates/fin-store/tests/fixtures/4a/README.md` (how it was made: the 4A commit hash, the scrub steps)
- The generator is **not** committed to 4B. It lives in a 4A worktree under `archive/` (git-ignored), so no later `cargo test` compiles it.

- [x] **Step 1: Generate from 4A code.**
  - `rtk git worktree add archive/wt-4a ef6f3ca`.
  - In it, write `crates/fin-store/examples/make_4a_fixture.rs` against that API:
    1. create a project in a temp dir;
    2. commit `asset.add` plus 5 inserts, trims and deletes;
    3. `tick` until exported;
    4. `close`;
    5. `PRAGMA wal_checkpoint(TRUNCATE)`.
  - Run it there with `rtk cargo run -p fin-store --example make_4a_fixture -- <out>`.
- [x] **Step 2: Scrub.**
  - Copy only `finisher.project.json`, the `.fseg` files and `project.fdb`. Never copy `lease.json`, `lease.claim-*`, `lease.probe-*`, `server-id`, `server-id.origin`, backups or locks.
  - On the DB copy: `DELETE FROM meta WHERE key IN ('projectDir','serverIds','createKey'); DELETE FROM lease_state; VACUUM;`.
  - Write `expected.json` from `snapshot("main")`.
- [x] **Step 3: Check for leaks.**
  - `rtk proxy bash -c 'grep -rIl -e "$(hostname)" -e "$USER" crates/fin-store/tests/fixtures/4a || echo clean'`, plus `strings project.fdb | grep -e "$(hostname)" -e "$USER"`. Expected: clean.
  - Segment headers hold only the random `writerId` ULID.
- [x] **Step 4: Add the test helper** `fixture_4a(tmp) -> (server_data, folder)` to `crates/fin-store/tests/integration_support/mod.rs`:
  1. copy the folder;
  2. place `project.fdb` at `<server_data>/projects/<id>/project.fdb`;
  3. open with `OpenOptions { relocate: true, .. }`.

  Add a smoke test, `fixture_4a_opens_with_relocate`, with the current (4A) code: the state equals `expected.json`.
- [x] **Step 5: Run** `rtk cargo test -p fin-store`, `rtk pnpm gates`. Remove the worktree with `rtk git worktree remove archive/wt-4a`.
- [x] **Step 6: Commit** `test(fin-store): scrubbed 4A fixture for 4B upgrade tests`.

### Task 3: Record kinds, content-versioned segments, per-kind reader versions, migration 0002 — tier: opus

**Files:**
- Create: `crates/fin-core/src/record/link.rs`. Rename `crates/fin-core/src/record.rs` → `record/mod.rs`
- Create: `crates/fin-store/src/migrations/m0002_history.rs`
- Modify: `crates/fin-core/src/upgrade.rs`, `crates/fin-core/src/tx.rs` (`CommittedTx.link`), `crates/fin-core/src/lib.rs`
- Modify: `crates/fin-store/src/segment.rs` (versions; `decode_frame` takes the reader version), `crates/fin-store/src/snapfile.rs` (passes its own reader version), `crates/fin-store/src/export/prepare.rs` (version from content), `crates/fin-store/src/compact/pack.rs` (pack version = the highest of its records' needs), `crates/fin-store/src/migrations/mod.rs` (`DB_VERSION = 2`, `MIN_READER_VERSION = 2`, step), `crates/fin-store/src/db/mod.rs` (`SCHEMA`, `init` inserts `main`), `crates/fin-store/src/db/records.rs`
- Modify: `crates/fin-store/examples/export_schema.rs` (`Link`)
- Test: `crates/fin-core/tests/record.rs`, `crates/fin-store/tests/migrations.rs` (the constructor edit only), `crates/fin-store/tests/codec.rs`, new `crates/fin-store/tests/format_v2.rs`

> Implementation notes (review of 1b69eeb): `tests/migrations.rs` changed beyond the constructor edit: its expectations moved with the version bump (meta `formatVersion`/`minReaderVersion` "1" -> "2", backup name `pre-migrate-0-1-` -> `0-2-`). `v1_segment_written_before_upgrade_is_adopted` tests adoption of the file, not content versioning. A format 1 segment holding a 4B kind is `Corrupt` (a v1 label implies edit-only). `record_links` rows per kind are covered by `crates/fin-store/src/db/link_tests.rs`.

**Interfaces:**
- **`fin_core::record`:**
  - `pub const RECORD_FORMAT: u32 = 2; pub const MIN_READER: u32 = 2;`
  - `RecordKind`:
    ```rust
    #[serde(rename_all = "camelCase")] #[non_exhaustive]
    pub enum RecordKind { Edit, Undo, Redo, Merge, CherryPick, Revert, Checkpoint, Branch, BranchSet }
    impl RecordKind {
        pub fn is_content(self) -> bool; // edit, undo, redo, cherryPick, revert
        pub fn has_ops(self) -> bool;    // false: checkpoint, branch, branchSet
        pub fn is_4a(self) -> bool;      // edit only
    }
    ```
  - `Record` gains, right after `kind` and each `#[serde(default, skip_serializing_if = "Option::is_none")]`: `undoes: Option<Vec<String>>`, `redoes: Option<String>`, `merge_id: Option<String>`, `from: Option<String>`, `origin: Option<String>`, `origin_hash: Option<String>`, `reverts: Option<String>`, `name: Option<String>`, `branch_id: Option<String>`, `fork_seq: Option<u64>`, `fork_hash: Option<String>`, `archived: Option<bool>`.
  - Constructor, used by `db::new_record` and by the m0001 comparison test:
    ```rust
    impl Record {
        pub fn new_edit(tx_id: String, seq: u64, schema_version: u32, parent_hash: Option<String>,
            author: String, committed_at: i64, intent: Tx, resolved: Vec<Op>, inverse: Vec<Op>,
            bindings: BTreeMap<String, String>) -> Record;
    }
    ```
  - `Link`:
    ```rust
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    #[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
    pub enum Link {
        Edit,
        Undo { undoes: Vec<String> },
        Redo { redoes: String },
        Merge { merge_id: String, from: String, origin: String },
        CherryPick { origin: String, from: String, origin_hash: Option<String> },
        Revert { reverts: String },
        Checkpoint { name: String },
        Branch { branch_id: String, name: String, fork_seq: u64, fork_hash: Option<String> },
        BranchSet { branch_id: String, name: Option<String>, archived: Option<bool> },
    }
    impl Link { pub fn kind(&self) -> RecordKind; }
    impl Record {
        pub fn link(&self) -> Result<Link, CoreError>; // INVALID_FORMAT on a mismatch
        pub fn set_link(&mut self, link: Link);
    }
    ```
  - `upcast_record` validates `link()`.
- **`fin_core::tx::CommittedTx`** gains `#[serde(default, skip_serializing_if = "Option::is_none")] pub link: Option<Link>`.
- **`fin_store::segment`:**
  - `FORMAT_VERSION = 2`, `MIN_READER_VERSION = 2`, `READER_VERSION = 2`.
  - `pub fn version_for(records: &[&[u8]]) -> (u32, u32)` gives `(1, 1)` when every record's kind is `edit`, else `(2, 2)`. It reads `kind` from each record's bytes.
  - `pub(crate) fn decode_frame(bytes: &[u8], magic: &str, format_version: u32, reader_version: u32)`. `snapfile.rs` passes `snapfile::READER_VERSION = 1`.
- **`db::records::new_record(seq, link: Link, intent, inverse, bindings, parent_hash, committed_at)`.** `insert_tx` and `insert_imported` write a `record_links` row for every non-edit record.
- **`m0002_history::run(txn)`** creates the following (the same text goes into `db::SCHEMA`):
  ```sql
  CREATE TABLE IF NOT EXISTS branches (
      id TEXT PRIMARY KEY, name TEXT NOT NULL, parent TEXT, fork_seq INTEGER NOT NULL,
      fork_hash TEXT, created_seq INTEGER, created_by TEXT NOT NULL, created_at INTEGER NOT NULL,
      archived INTEGER NOT NULL, merged INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS record_links (
      tx_id TEXT PRIMARY KEY, branch TEXT NOT NULL, seq INTEGER NOT NULL, kind TEXT NOT NULL,
      author TEXT NOT NULL, origin TEXT, origin_hash TEXT, from_branch TEXT, merge_id TEXT,
      targets TEXT, name TEXT, UNIQUE (branch, seq)
  );
  CREATE INDEX IF NOT EXISTS record_links_origin ON record_links (origin) WHERE origin IS NOT NULL;
  CREATE INDEX IF NOT EXISTS record_links_origin_hash ON record_links (origin_hash) WHERE origin_hash IS NOT NULL;
  CREATE INDEX IF NOT EXISTS record_links_merge ON record_links (merge_id) WHERE merge_id IS NOT NULL;
  ```
  It then runs `INSERT INTO branches VALUES ('main','main',NULL,0,NULL,NULL,'',0,0,0)` and `UPDATE meta SET value = '2' WHERE key IN ('formatVersion','minReaderVersion')`. Literals only.

- [x] **Step 1: Write failing tests.**
  - `edit_record_bytes_unchanged_from_4a`: each record line of the fixture's segments re-encodes byte-equal.
  - `every_kind_roundtrips_byte_stable`, `kind_names_are_camel_case`.
  - `link_fields_must_match_kind`: a table of bad shapes, each `INVALID_FORMAT`.
  - Change the `"undo"` → `NEWER_FORMAT` test to `"squash"`.
  - `committed_tx_link_omitted_for_edits`.
  - `edit_only_segment_is_v1`, `segment_with_branch_record_is_v2`, `pack_version_is_highest_of_contents`.
  - `reads_v1_and_v2_segments`.
  - `v2_segment_is_newer_for_a_v1_reader`: `decode_frame(.., reader_version = 1)` → `Newer`.
  - `snapshot_reader_version_is_its_own`: a snapshot with `minReaderVersion` 2 is `Newer` even though `segment::READER_VERSION` is 2.
  - `migration_0002_from_4a_fixture`: `user_version` is 2, `branches` holds `main`, the state equals `expected.json`, and `backups/pre-migrate-1-2-*` exists.
  - `sp2_fixture_migrates_through_0002`.
  - `fresh_and_migrated_schemas_match` and the m0001 pins: unchanged expectations.
  - `v1_segment_written_before_upgrade_is_adopted`: in a fixture copy, set `exported = 0` on the last segment's rows; open and tick. The v1 file is adopted, with no new file.
- [x] **Step 2: Run** `rtk cargo test -p fin-core -p fin-store`. Expected: FAIL.
- [x] **Step 3: Implement.** `kind_text` yields the camelCase name.
- [x] **Step 4: Run** the three gates, after regenerating types (`Link`, `CommittedTx.link`). Expected: PASS.
- [x] **Step 5: Commit** `feat(format): record kinds, content-versioned segments, DB migration 0002`.

### Task 4: Shift maps and mirrored mappings (fin-core) — tier: opus

**Files:**
- Rename `crates/fin-core/src/rebase.rs` → `rebase/mod.rs` (no behaviour change).
- Create: `crates/fin-core/src/rebase/map.rs`.
- Test: `crates/fin-core/tests/shift_map.rs`.

Tasks 4 and 5 are sequential: both build on the `rebase/` rename.

**Interfaces** (`fin_core::rebase::map`, re-exported):
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc { Before, After }
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StepMap { shifts: Vec<Shift> } // op order; each in the frame after the previous
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recover { pub shift: usize, pub offset: Rational }
// `deleted`: the first shift that deleted it, edges included; `across`: strictly inside some range
pub struct MapResult { pub pos: Rational, pub deleted: Option<Recover>, pub across: bool }
impl StepMap {
    pub fn from_shifts(shifts: Vec<Shift>) -> StepMap;
    pub fn from_diff(diff: &Diff) -> StepMap;
    pub fn shifts(&self) -> &[Shift];
    pub fn invert(&self) -> Result<StepMap, CoreError>; // reverse order, {from, by} → {from + by, -by}
    /// `by >= 0` inserts `[from, from+by)`: positions at or after `from` (After), or after
    /// `from` (Before), move by `by`. `by < 0` removes `[s, e)` = `[from+by, from)`: positions
    /// after `e` move by `by`; positions in `[s, e]` collapse to `s` and are `deleted` with
    /// `offset = pos − s`, except at the assoc edge (After: `e` is never deleted; Before: `s`
    /// is never deleted) (Decision 16); strictly inside `(s, e)` also sets `across`.
    pub fn map(&self, track: &str, pos: Rational, assoc: Assoc) -> Result<MapResult, CoreError>;
    /// The position `offset` into shift `r.shift`'s inserted range, if it inserts `by == size`.
    pub fn recover(&self, r: Recover, size: Rational) -> Result<Option<Rational>, CoreError>;
}
#[derive(Debug, Clone, Default)]
pub struct Mapping { maps: Vec<StepMap>, mirror: Vec<Option<usize>> }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmapped { Deleted { step: usize }, Overflow }
impl Mapping {
    pub fn new() -> Mapping;
    pub fn push(&mut self, map: StepMap) -> usize;
    pub fn set_mirror(&mut self, a: usize, b: usize); // a < b, b inverts a; neither mirrored yet
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    /// ProseMirror `Mapping.map`: a deletion at step i with mirror j > i resumes at
    /// `maps[j].recover(..)` when the two shifts have equal |by| on the same track; otherwise
    /// a position strictly inside the range is `Unmapped::Deleted { step: i }`, and one at its
    /// edge carries on from the range start.
    pub fn map(&self, track: &str, pos: Rational, assoc: Assoc) -> Result<Rational, Unmapped>;
    /// `map` through `maps[start..]` only (ProseMirror `slice(start)`), for per-op mappings.
    pub fn map_from(&self, start: usize, track: &str, pos: Rational, assoc: Assoc)
        -> Result<Rational, Unmapped>;
}
```

- [x] **Step 1: Write failing tests** (props use 256 cases):
  - `empty_mapping_is_identity`, `invert_is_involution`, `map_then_inverse_is_identity_outside_changed_ranges`;
  - `mirror_restores_positions_inside_inserts`;
  - `start_edge_of_inverted_insert_recovers_offset_zero` (After: a position at `s` of the inverse range resumes at `mirror.from + 0`);
  - `end_edge_with_after_is_not_deleted`;
  - `mirror_requires_equal_size` (unequal `|by|` → `Deleted`);
  - `after_assoc_matches_4a_shift_position` (prop, forward maps);
  - `other_tracks_are_untouched`, `overflow_is_reported`;
  - after review: `start_edge_without_mirror_carries_on` (a ripple delete of the next clip leaves an insert at its start edge mapped, as 4A did), `mirrored_inverse_then_map_is_identity` (prop), `map_from_skips_earlier_maps`, `recover_overflow_is_reported`.
- [x] **Step 2: Run** `rtk cargo test -p fin-core --test shift_map`. Expected: FAIL.
- [x] **Step 3: Implement** `map.rs` (< 300 lines, checked `Rational` arithmetic only).
- [x] **Step 4: Run** `rtk cargo test --workspace`, `rtk pnpm gates`. Expected: PASS.
- [x] **Step 5: Commit** `feat(core): exact per-track shift maps with mirrored mappings`.

### Task 5: Per-track, per-entity conflict rules — tier: opus

**Files:**
- Create: `crates/fin-core/src/rebase/footprint.rs`, `crates/fin-core/src/rebase/conflict.rs`
- Modify: each op module under `crates/fin-core/src/ops/` (adds `pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint`), `crates/fin-core/src/ops/mod.rs` (dispatch; `Op::positions_mut`)
- Modify: `crates/fin-store/examples/export_schema.rs` (`Conflict`)
- Test: `crates/fin-core/tests/conflict_rules.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Conflict { pub tx_id: String, pub op_index: usize, pub reason: String,
                      pub against_tx: Option<String>, pub fix: String }
pub mod reasons { /* REMOVED, SAME_EDGE, SAME_PARAM, DEPENDS_ON_SKIPPED, ALREADY_PRESENT,
                     ID_COLLISION, UNDOES_TARGET_RECORDS */ }
pub struct Conflicts(Vec<Conflict>); // non-empty: new(Vec) -> Option, one, push, first, as_slice
impl From<Conflicts> for CoreError; // CONFLICT; first conflict's 4A fields + details.conflicts
impl Conflict { pub fn from_rerun(e: &CoreError, tx_id: &str, op_index: usize, against: Option<String>) -> Conflict; }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch { Exists, Edge(Edge), Param(String) }
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint { pub tracks: BTreeSet<String>, pub touches: Vec<(String, Touch)>,
    pub removes: Vec<String>, pub creates: Vec<String>, pub uses: Vec<String> }
pub fn footprint(op: &Op, op_diff: &Diff) -> Footprint;
pub fn entity_conflict(ours: &Footprint, own: &BTreeSet<String>, theirs: &[(&str, Footprint)])
    -> Option<(&'static str, String)>;
pub fn split_diff(ops: &[Op], diff: &Diff) -> Result<Vec<Diff>, CoreError>;
pub fn committed_ops(c: &CommittedTx) -> Result<Vec<(Op, Diff)>, CoreError>;
pub fn committed_footprints(c: &CommittedTx) -> Result<Vec<Footprint>, CoreError>;
impl Op { pub(crate) fn positions_mut(&mut self) -> Vec<(String, &mut Rational)>; }
```
Rules:
- touching or using X when an unmirrored intervening record removed X → `REMOVED`. This covers a stale insert of an asset that `asset.remove` removed (closes the TASKS follow-up).
- the same `Edge` of X → `SAME_EDGE`; the same `Param` → `SAME_PARAM`.
- disjoint track sets never conflict (entities live on one track; entity rules themselves ignore tracks, so a 4C move between tracks still meets the other track's edits).

- [x] **Step 1: Write failing tests** (table: committed op, our op, expected):
  - an insert on `t2` against every op on `t1` → none;
  - trim end `c1` vs trim end `c1` → `SAME_EDGE`; trim start vs trim end → none;
  - trim vs delete of the same clip → `REMOVED`; delete vs trim → none;
  - insert using `a1` vs `asset.remove a1` → `REMOVED`;
  - `every_op_has_a_footprint` (an exhaustive match);
  - `conflict_into_core_error_keeps_4a_fields`;
  - also: `removed_then_restored_is_not_removed` (the last removal or creation of X decides), `removed_wins_over_same_edge`, `same_param_conflicts_and_disjoint_tracks_never_do` (a footprint with no track, an asset op, is judged by entities alone), `rerun_conflict_takes_the_policy_code`, `conflict_without_against_tx`; `positions_mut_lists_placements` (unit).
  - Until Task 6 the 4A struct is `rebase::LegacyConflict` (fields unchanged), so `Conflict` names the new type; Task 6 deletes it with the per-op `rebase` fns.
  - After review: `entity_conflict(ours, own, theirs)` takes `own`, the entities earlier ops of our tx created (at a stale base our placeholders bind the ids the intervening records allocated, so they are never judged); entity rules ignore tracks (an item may move between tracks in 4C); `Conflicts` is non-empty (`new` → `Option`, `one`, `push`); `split_diff`, `committed_ops` and `committed_footprints` are public for Task 6. Tests: `own_new_clip_is_not_their_removed_clip`, `own_new_clip_is_not_their_trimmed_clip`, `own_new_asset_is_not_their_removed_asset`, `an_existing_entity_ours_names_is_still_judged`, `entity_rules_ignore_tracks`, `remove_restore_remove_names_the_last_removal`, `earliest_standing_removal_is_named_across_entities`, `of_two_same_edge_records_the_earliest_is_named`, `split_diff_pairs_each_op_with_its_shift`.
- [x] **Step 2: Run** `rtk cargo test -p fin-core --test conflict_rules`. Expected: FAIL.
- [x] **Step 3: Implement.** Footprints come from the resolved op plus its own diff, never from a project.
- [x] **Step 4: Run** the three gates (types regenerated). Expected: PASS.
- [x] **Step 5: Commit** `feat(core): per-track and per-entity conflict footprints`.

### Task 6: Rebase engine rewrite and commit integration — tier: opus

**Files:**
- Modify: `crates/fin-core/src/rebase/mod.rs` (engine), op modules (delete the per-op `rebase` fns), `crates/fin-core/src/ops/clip_restore.rs` and `crates/fin-core/src/ops/mod.rs` (`allocates` covers `clip.restore` with a `$` id; `BoundIds` hands it out on replay)
- Modify: `crates/fin-store/src/commit.rs` (engine; frame from `state::state_at`). Create `crates/fin-store/src/state.rs` (single-branch `state_at`; Task 7 replaces it)
- Test: `crates/fin-core/tests/rebase.rs`, new `crates/fin-core/tests/rebase_props.rs`, `crates/fin-store/tests/store.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone)]
pub struct Step { pub tx_id: String, pub ops: Vec<Op>, pub diff: Diff,
                  pub mirror_of: Option<usize>, pub invert: bool }
impl Step { pub fn from_committed(c: &CommittedTx) -> Step; }
pub struct Rebase<'a> { pub tx: &'a Tx, pub own: &'a Diff, pub steps: &'a [Step] }
pub struct Rebased { pub tx: Tx, pub applied: Applied }
/// Op by op: map positions (Assoc::After) through [inverse(our earlier ops),
/// steps, our rebased earlier ops] with mirrors; check entity conflicts against unmirrored
/// steps; apply the op to the evolving head with full policy (`mode`); a failure → `Conflict`.
pub fn rebase_apply(head: &Project, r: &Rebase<'_>, ids: &mut dyn IdAlloc, mode: ApplyMode)
    -> Result<Rebased, Conflict>;
/// Stale commit: `own` = `tx` applied at `base` with `MemIds::after(base)`; steps = `since`
/// with rev > base_rev.
pub fn rebase_tx(base: &Project, head: &Project, tx: &Tx, since: &[CommittedTx],
                 ids: &mut dyn IdAlloc) -> Result<Rebased, CoreError>;
```
`Store::commit_at_generation`: when `base_rev < head`, the frame is `state_at(txn, branch, base_rev)`. The `Rebased.applied` replaces the second `apply_tx`.

- [x] **Step 1: Write failing tests.**
  - `tests/rebase.rs`:
    - keep every SP2 test;
    - replace `positions_after_our_own_trim_or_delete_conflict_when_the_track_moved` with `positions_after_our_own_trim_or_delete_follow_the_track`;
    - prose reasons become codes;
    - add `insert_before_own_earlier_insert`: our tx inserts A at 10, then B at 10 (B before A); a committed insert at 5 is 2 s long. Result: B at 12, A at 12 + B's duration.
  - `rebase_props.rs`:
    - `rebase_over_empty_interval_is_identity` (prop 2, 256 cases);
    - `stale_and_fresh_agree_multi_track` (prop 3, 128 cases): an anchor model on 3 tracks. The intention is "before clip k on track t" / "track end" / "trim k edge e by d" / "delete k", realised at base and at head; when both succeed, the states are equal. Ties use "before the clip that was at p".
  - Unit tests:
    - `stale_insert_of_removed_asset_conflicts_removed`;
    - `insert_inside_committed_clip_is_policy_conflict`;
    - `other_track_edits_never_conflict`;
    - `restore_with_placeholder_id_binds` (Client: `INVALID_OP`; Internal: binds `c<n>`; Replay with `BoundIds`: same id);
    - `copy_with_bound_ids_duplicate_is_already_present`.
  - `store.rs`: `commit_rebases_after_own_delete_exactly`, `conflict_details_list_conflicts`.
- [x] **Step 2: Run** `rtk cargo test -p fin-core -p fin-store`. Expected: FAIL.
- [x] **Step 3: Implement.** Delete `ops::rebase`, the per-op `rebase` fns, `inserted_range`, `removed_clip` and `blind`.
- [x] **Step 4: Measure the budget (first time):** `rtk proxy bash -c 'time cargo test -p fin-core --test rebase_props --test shift_map'`. Record the time in TASKS.md; it must be < 8 s.
- [x] **Step 5:** Regenerate `crates/fin/tests/fixtures/rpc/conflict.response.ndjson` with `apps/finisherd/scripts/capture-rpc-fixtures.ts`. Run the three gates. Expected: PASS.
- [x] **Step 6: Commit** `feat(core): exact rebase engine; per-track conflicts; restore placeholders`.
- As built: `Step::from_committed` returns `Result` (resolving the record's placeholders can fail) and `Step.ops` are resolved; `invert` steps map through the inverse of `diff` and swap `creates`/`removes` in their footprints. `Rebase` gains `same_origin: &BTreeSet<String>` (ids on the head bound by records of the same origin chain; Task 13/14 fill it, `Rebase::new` leaves it empty), which decides `ALREADY_PRESENT` vs `ID_COLLISION` for a re-run `DUPLICATE_ID`. A bad `mirror_of` is a conflict with code `INVALID_OP`. `rebase_tx` returns a tx's own error when it does not apply at its base. Ops are re-run through a stepwise `apply::Applier`, which `apply_tx_mode` now uses too. Tests added beyond the list: `mirrored_steps_are_not_judged`; the SP2 props now also check that the stored intent replays.
- After review: `rebase_tx` builds its steps with `steps_from`, so an undo or redo inside the interval mirrors its unit or undo (`undo_in_since_mirrors_its_unit`, `undo_of_several_units_mirrors_each`, `redo_mirrors_its_undo`). A tx that fails at its base (with no conflict) carries `details.baseRev` and a fix to resend against the head. `Conflict` gains optional `details` (a re-run error's own), and a re-run failure blames the one unmirrored step that changed an entity the op changes or the error names. The commit frame comes from `state::state_near_head`: a rewind from the head through stored inverses, or a replay from the nearest snapshot, whichever touches fewer records, after `txs_since` has filled pending diffs. Prop 3 also generates asset ops and an undo pair, and requires a stale refusal where the fresh edit applies to be `SAME_EDGE` or `REMOVED`.

### Task 7: Shared write path and lineage reads — tier: opus

**Files:**
- Create: `crates/fin-store/src/write.rs` (extracted from `commit.rs`), `crates/fin-store/src/branch/lineage.rs` (delete Task 6's `state.rs`), `crates/fin-store/src/branch/mod.rs` (module root only)
- Modify: `crates/fin-store/src/commit.rs`, `crates/fin-store/src/lib.rs` (`refresh` fork-hash fallback; reads through lineage), `crates/fin-store/src/db/records.rs` (`txs_since` → lineage), `crates/fin-store/src/diffs.rs` (fill per ancestor)
- Test: new `crates/fin-store/tests/lineage.rs`, `crates/fin-store/tests/store.rs`

**Interfaces:**
```rust
// branch/lineage.rs
pub(crate) struct LineageSeg { pub branch: String, pub after: u64, pub through: Option<u64> } // own first
pub(crate) fn lineage(conn: &Connection, branch: &str) -> Result<Vec<LineageSeg>, StoreError>; // `main` only until Task 8
pub(crate) fn state_at(conn: &Connection, branch: &str, seq: u64) -> Result<Project, StoreError>;
pub(crate) fn records_since(conn: &Connection, branch: &str, after: u64) -> Result<Vec<CommittedTx>, StoreError>; // diffs::fill per ancestor
pub(crate) fn hash_at(conn: &Connection, branch: &str, seq: u64) -> Result<Option<String>, StoreError>;
pub(crate) fn tip_hash(conn: &Connection, branch: &str) -> Result<Option<String>, StoreError>; // fork_hash fallback
// write.rs
pub(crate) struct Draft { pub branch: String, pub link: Link, pub intent: Tx, pub inverse: Vec<Op>,
    pub bindings: BTreeMap<String, String>, pub diff: Diff, pub project: Project }
pub(crate) struct WriteCtx<'a> { pub txn: &'a Transaction<'a>, pub epoch: Option<u64>, pub dry_run: bool /* heads, now */ }
impl WriteCtx<'_> {
    pub(crate) fn append(&mut self, d: Draft) -> Result<CommittedTx, StoreError>;
    pub(crate) fn head(&mut self, branch: &str) -> Result<&Head, StoreError>;
}
impl Store {
    /// Dedupe by `first_tx_id` (an existing row → `Ok(None)`). Real writes: read-only check,
    /// `lease.check_recent(1 s)`, IMMEDIATE txn, `f`, `lease.quick_check()` at the same epoch,
    /// commit. `dry_run`: no read-only/lease/fence checks (works on a follower; `epoch: None`),
    /// DEFERRED txn, `f`, rollback (Decision 26). Heads are adopted only after a commit.
    pub(crate) fn write<R>(&mut self, first_tx_id: &str, dry_run: bool,
        f: impl FnOnce(&mut WriteCtx<'_>) -> Result<R, StoreError>) -> Result<Option<R>, StoreError>;
}
```
`WriteCtx::append` does: `new_record` → `insert_tx` (at `epoch`; in a dry run, at 0, then rolled back) → `record_links` → snapshot every 200 → staged head. It updates the index from Task 8 on.

- [x] **Step 1: Write failing tests:**
  - `commit_through_write_keeps_4a_order` (the documented check order: dedupe before read-only, `BASE_REV_AHEAD`, `HISTORY_REWOUND`);
  - `write_dry_run_on_follower_plans_and_rolls_back`;
  - `write_failure_after_apply_rolls_back` (the `fail_next_commit_after_apply` hook moves into `write`);
  - `refresh_tip_falls_back_to_fork_hash` (a unit test with a hand-made index row);
  - `records_since_fills_pending_ancestor_diffs`.
- [x] **Step 2: Run** `rtk cargo test -p fin-store`. Expected: FAIL.
- [x] **Step 3: Implement.** `commit.rs` keeps its public signatures and docs, now through `Store::write`.
- [x] **Step 4: Run** the three gates. Expected: PASS.
- [x] **Step 5: Commit** `refactor(store): one write path (dry runs on followers) and lineage reads`.
- As built: `LineageSeg` gains `fork_hash`, and `lineage` already reads the `branches` index (parent, fork seq, fork hash; own first; each ancestor's `through` is the lowest fork below it, so a fork below the parent's own fork reads the grandparent; a branch without a row is a root; a parent loop is `DB_ERROR`), so Task 8 only has to write the rows. `state_at` replays from the latest snapshot on the lineage branch that covers the seq (else from that branch's fork state, recursively); `state_near_head` and its rewind also run through the lineage. `db::txs_since` is gone: `lineage::records_since` reads each branch's range with `db::txs_in` and fills pending diffs only on the branches it reaches; `diffs::fill` on a child starts at its fork. Below the fork, `ops_since` returns the ancestors' records with their own `tx.branch`. `WriteCtx` also holds `now` (one `committedAt` per write) and stages heads; `head()` refreshes the store cache only before the write's first row (committed state), later on the side. `append` takes the parent hash from the staged head's tip (the fork hash at a branch's start) and returns `CommittedTx.link`, so Task 12's undo/redo records carry it. The in-memory lease re-check runs after `f`, right before the SQLite commit (it ran before the inserts), inside the same transaction. `Store::commit_in(tx, generation, dry_run)` (crate-private) is the commit with a dry run, for tests and the history operations; `commit` loops once more if a deduped tx vanished between the dedupe and the read (an import rewound it). `import::apply` takes its parent hash from `lineage::tip_hash`. After review (Task 7): dry runs are IMMEDIATE too (Decision 26); `WriteCtx` tells uncommitted state by the connection's `total_changes()` since the write began, not by appends alone, so rows written through `txn` before the first append (ids, filled diffs, Task 8's index row and fork snapshot) never reach the shared cache (a cached head still current in the transaction is used without a copy); `diffs::fill` never deletes a branch's fork snapshot (`through > start`); lineage reads check that each branch's rows are contiguous and that an ancestor reaches the fork (`DB_ERROR` otherwise); `append` debug-asserts `intent.baseRev` = the head; the dedupe answer and `ops_since` may fill diffs outside a write transaction (documented in `write.rs`). Tests added: `dry_run_holds_the_write_lock_from_the_start`, `heads_after_uncommitted_rows_stay_out_of_the_cache`, `fill_at_the_fork_keeps_the_fork_snapshot`, `parent_short_of_the_fork_is_db_error`. Tests: `write_dry_run_on_follower_plans_and_rolls_back`, `write_failure_after_apply_rolls_back` and the lineage unit tests (`refresh_tip_falls_back_to_fork_hash`, `lineage_walks_parents_and_clamps_to_the_lowest_fork`, `reads_below_fork_read_the_parent`, `records_since_fills_pending_ancestor_diffs`, the moved `rewind_from_head_equals_replay`) are unit tests in `src/write_tests.rs` and `src/branch/lineage_tests.rs` (they need `Store::write` and hand-made index rows); `tests/lineage.rs` covers the public reads (`ops_since_below_fork_reads_parent`, `stale_commit_below_fork_rebases_over_lineage`, `ops_since_fills_pending_ancestor_diffs`); `tests/store.rs` has `commit_through_write_keeps_4a_order`.

### Task 8: Branch index, branch create/set, names — tier: opus

**Files:**
- Create: `crates/fin-core/src/branch/mod.rs`; test `crates/fin-core/tests/branch.rs`
- Create: `crates/fin-store/src/branch/index.rs` (table I/O, `absorb`), `crates/fin-store/src/branch/create.rs` (`Store::create_branch`, `Store::set_branch`), `crates/fin-store/src/derived.rs` (`recompute`)
- Modify: `crates/fin-store/src/branch/lineage.rs` (real lineage), `crates/fin-store/src/write.rs` (index updates; `BRANCH_ARCHIVED`), `crates/fin-store/src/lib.rs` (name resolution in `head`/`snapshot`/`ops_since`/commit), `crates/fin-store/src/wire.rs`, `crates/fin-store/examples/export_schema.rs` (`BranchView`, `BranchCreated`, `BranchState`), `crates/fin/src/output.rs` (`NAME_TAKEN`, `BRANCH_ARCHIVED` → exit 3)
- Test: new `crates/fin-store/tests/branches.rs`

**Interfaces:**
```rust
// fin_core::branch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BranchState { Active, Archived, Merged }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo { pub id: String, pub name: String, pub parent: Option<String>,
    pub fork_seq: u64, pub fork_hash: Option<String>, pub created_seq: Option<u64>,
    pub created_by: String, pub created_at: i64, pub archived: bool, pub merged: bool }
impl BranchInfo { pub fn state(&self) -> BranchState; } // archived > merged > active
pub fn validate_branch_name(name: &str) -> Result<(), CoreError>;
pub fn validate_checkpoint_name(name: &str) -> Result<(), CoreError>;
pub fn id_number(id: &str) -> Option<u64>; // "br10" → 10, numeric ordering
#[derive(Debug, Clone, Default)]
pub struct BranchIndex { /* id → BranchInfo */ }
impl BranchIndex {
    pub fn with_main() -> BranchIndex;
    pub fn absorb(&mut self, branch: &str, seq: u64, rec: &Record) -> Result<(), CoreError>;
    pub fn resolve(&self, r: &str) -> Option<&BranchInfo>; // id | non-archived name (case-insensitive) | name#id
    pub fn name_taken(&self, name: &str) -> bool;          // among non-archived (merged included)
    pub fn list(&self) -> Vec<(String /*display*/, &BranchInfo)>; // numeric id order; dup → name#id
    pub fn is_descendant(&self, a: &str, of: &str) -> bool;
    pub fn parents_first(&self) -> Vec<&str>;                  // depth, then numeric id
}
```
```rust
// fin_store
#[derive(Debug, Clone, Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct BranchView { pub id: String, pub name: String, pub display: String, pub parent: Option<String>,
    pub fork_seq: u64, pub head: u64, pub state: BranchState, pub created_by: String }
pub enum At { Seq(u64), Checkpoint(String) } // Checkpoint lands in Task 15
pub struct BranchCreate { pub tx_id: String, pub author: String, pub name: Option<String>,
    pub from: String, pub at: Option<At>, pub dry_run: bool }
pub struct BranchSet { pub tx_id: String, pub author: String, pub branch: String,
    pub name: Option<String>, pub archived: Option<bool>, pub dry_run: bool }
#[derive(Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct BranchCreated { pub branch: BranchView, pub record: CommittedTx, pub dry_run: bool }
impl Store {
    pub fn branches(&self, all: bool) -> Result<Vec<BranchView>, StoreError>;
    pub fn resolve_branch(&self, r: &str) -> Result<String, StoreError>; // BRANCH_NOT_FOUND
    pub fn create_branch(&mut self, r: BranchCreate) -> Result<BranchCreated, StoreError>;
    pub fn set_branch(&mut self, r: BranchSet) -> Result<CommittedTx, StoreError>;
}
// derived.rs
pub(crate) fn recompute(txn: &Transaction) -> Result<(), StoreError>; // record_links GC + index + merged (Task 14)
```
- **`create_branch`**, in one `write`:
  1. Resolve `from`; the fork seq is `at` or the parent head; `forkHash = lineage::hash_at`.
  2. Take `br<n>` from `Counters`.
  3. Use the given name (validated, `NAME_TAKEN`), or `try-<n>` when `None`.
  4. Append the `branch` record on the parent.
  5. Insert the index row and `snapshots(child, fork_seq) = state_at(parent, fork_seq)`.
- **`set_branch`:** writes `branchSet` on the branch's parent. `main` → `INVALID_OP`. Restoring onto a taken name → `NAME_TAKEN` (`fix: fin branch archive <b> --restore --as <new>`).

- [x] **Step 1: Write failing tests.**
  - `fin-core/tests/branch.rs`:
    - `branch_names_table`, `checkpoint_names_table`;
    - `index_folds_branch_and_branch_set`;
    - `duplicate_names_lower_numeric_id_keeps_name` (`br9` vs `br10`);
    - `resolve_forms` (id, name, `name#id`, archived by name → not found);
    - `merged_counts_as_holding_its_name`.
  - `fin-store/tests/branches.rs`:
    - `create_branch_appends_branch_record_on_parent`;
    - `branch_head_is_fork_seq_and_state_matches_parent_at_fork`;
    - `first_record_on_branch_links_to_fork_hash_and_seq_continues`;
    - `from_older_seq_forks_there`;
    - `fork_at_zero_has_null_fork_hash`;
    - `duplicate_active_name_refused_name_taken`;
    - `archived_name_can_be_reused`;
    - `restore_onto_taken_name_needs_as`;
    - `rename_and_archive_are_branch_set_on_parent`;
    - `archived_branch_refuses_commits`;
    - `main_cannot_be_renamed_or_archived`;
    - `resolve_by_name_or_id_everywhere` (the intent stores the id);
    - `ops_since_below_fork_reads_parent`;
    - `stale_base_below_fork_rebases_over_lineage`;
    - `recompute_matches_incremental`;
    - `branch_ids_never_reused_after_reopen`;
    - `create_branch_dry_run_writes_nothing` (also on a follower);
    - `create_branch_retry_same_tx_id_returns_first`;
    - `default_try_name_uses_branch_id`.
- [x] **Step 2: Run** `rtk cargo test -p fin-core --test branch` and `rtk cargo test -p fin-store --test branches`. Expected: FAIL.
- [x] **Step 3: Implement.**
- [x] **Step 4: Run** the three gates (types regenerated). Expected: PASS.
- [x] **Step 5: Commit** `feat(store): branches derived from the log; create, rename, archive`.
- As built: `fin_core::branch` also has `MAIN` (fin-store's `MAIN` aliases it), `NAME_MAX`, `BranchInfo::main()`, and on `BranchIndex` `from_infos` (the table, plus `main`), `get`, `holder` (the non-archived holder of a name, lowest numeric id first) and `display` (the name while it resolves to the branch, else `<name>#<id>`; archived branches list that way). `absorb` returns the id it changed (`Result<Option<String>, _>`) and refuses, `INVALID_FORMAT`, shapes the log never holds: a known id again, a non-`br<n>` id, a branch record on an unknown branch or with `forkSeq` ≥ its own seq, a `branchSet` of a branch that is not a child of the record's branch. `main` and `br\d+` are refused as names case-insensitively. `branch/index.rs` holds the table I/O, `absorb` (called by `WriteCtx::append` for every record; other kinds read nothing) and `resolve` (id, then index; a branch with rows but no index row is a root under its own id, as lineage reads take it). `write::writable` resolves a reference and refuses an archived branch (`BRANCH_ARCHIVED`, `details {branch, name}`, `fix: fin branch archive <id> --restore`); `commit::plan` calls it first under the lock, so the stored intent holds the id; `head`, `snapshot`, `ops_since`, `ops_since_at`, `generation` and `generation_token` resolve references too (`head` of an unknown one is still 0). `create_branch`: `at` past the parent's head is `INVALID_OP`; `At::Checkpoint` is `NOT_FOUND` (`details.kind` `checkpoint`) until Task 15; an archived parent is `BRANCH_ARCHIVED`; the id is allocated before the name check, inside the write, so a refused create consumes none. `set_branch`: nothing to set is `INVALID_OP`; `main` (any root) `INVALID_OP`; the record goes on the parent even when the parent is archived; an archived branch may be renamed without restoring it; `NAME_TAKEN` (`details {name, branch: holder id}`) is checked for the state after the set, and its `fix` is `fin branch archive <id> --restore --as <new>` for a plain restore. A retried txId answers from the log (a create with the branch as it is now and `dryRun: false`); a txId that holds another kind of record is `IDEM_KEY_REUSED`. `BranchView` and `BranchCreated` live in `wire.rs` (also `Deserialize`, `deny_unknown_fields`). `derived::recompute` collects `record_links` rows without a record and rebuilds the index breadth-first from `main` over `branch`/`branchSet` rows; `merged` stays false until Task 14; `Store::recompute_derived_for_test` is its test hook (`dead_code` allowed outside tests until Task 10 calls it). `BranchCreate`/`BranchSet` carry no `argsHash` yet (Tasks 17/19 add it for Decision 26). Tests added beyond the list: `id_numbers_order_numerically`, `parents_first_is_depth_then_numeric_id`, `branch_state_serialises_camel_case`.
- For Task 9: import does not absorb yet. `insert_imported` must run `branch::index::absorb` per applied record in the import transaction and, for a `branch` record, insert the child's fork snapshot (`snapshots(child, forkSeq) = lineage::state_at(parent, forkSeq)`, Decision 14) as `create_branch` does, or the child has no head on the follower. `db::branches` lists branches from `snapshots`, so a child with only its fork snapshot is already listed by export, status, verify and compaction.

### Task 9: Branch-aware import, export, follow, counters, status — tier: opus

> From the Task 7 review: check import's seed case `h.rev == 0` (`import/apply.rs`, `seed_for`) for a child forked at seq 0. Its head is rev 0 like a fresh `main`, but its start is the parent's state at 0 and its fork snapshot (Decision 14), not a project genesis.

**Files:**
- Modify: `crates/fin-store/src/chain/mod.rs` (`Known.fork`; `resolve` passes it to `Graph::build`), `crates/fin-store/src/chain/graph.rs` (`Graph::build(segs, fork: Option<&ForkPoint>)`, start check)
- Modify: `crates/fin-store/src/import/pass.rs` (worklist; set-aside branch records → child segments quarantined), `crates/fin-store/src/import/apply.rs` (absorb applied branch records), `crates/fin-store/src/import/known.rs`, `crates/fin-store/src/import/mod.rs` (`unknown_branches`)
- Modify: `crates/fin-store/src/export/rows.rs` (`pending_branches` parents first), `crates/fin-store/src/counters.rs` (raise from `branches.id` and `record_links`; `raise_to_quarantine`), `crates/fin-store/src/status/mod.rs` (`BranchStatus` gains `name, parent, fork_seq, state`; `ProjectStatus.unknown_branches`)
- Test: new `crates/fin-store/tests/branch_follow.rs`, `crates/fin-store/tests/chain_scenarios.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ForkPoint { pub seq: u64, pub hash: Option<String> }
pub struct Known { /* 4A fields */ pub fork: Option<ForkPoint> }
impl Known { pub fn for_branch(kind: StorageKind, fork: Option<ForkPoint>) -> Known; }
pub(super) fn build(segs: &[SegRef], fork: Option<&ForkPoint>) -> Graph<'_>; // graph.rs
pub struct ImportReport { /* 4A */ pub unknown_branches: Vec<String> }
impl Counters { pub(crate) fn raise_to_quarantine(conn: &Connection) -> Result<(), StoreError>; }
pub(crate) fn absorb_applied(txn: &Transaction, branch: &str, applied: &[(u64, Record)]) -> Result<Vec<String>, StoreError>; // branch::index
```
- **Graph start check:** on a branch other than `main`, a segment with `prevSegmentHash` null must start at `fork.seq + 1` with `parentHash == fork.hash`. Otherwise it is malformed ("does not start at the branch's fork point").
- **Import pass:**
  1. The worklist starts with `parents_first()`, and children are pushed as their branch records are applied. The cache key includes the fork point.
  2. Folder directories never reached are classified. If their branch record is a current quarantine row (search quarantine rows whose bytes have kind `branch` and that `branchId`), all their records are quarantined with that row's reason (Decision 8), recursively. Otherwise they are `unknown_branches`.
  3. `raise_to_quarantine` runs whenever quarantine rows were added.

- [x] **Step 1: Write failing tests:**
  - `branch_segment_must_start_at_fork_point`;
  - `branch_start_competition_uses_epoch_rule`;
  - `follower_learns_branch_from_parent_record`;
  - `child_segments_before_branch_record_wait` (`unknown_branches == ["br1"]`, `read_only() == None`, then both apply in one pass);
  - `export_writes_parent_before_child` (the segments' file mtimes or write order via a test hook; the follower never reports `unknown_branches` after the same poll);
  - `branch_set_and_rename_follow`;
  - `export_branch_first_segment_has_no_prev_and_parent_is_fork_hash`;
  - `ids_counter_raised_by_imported_branch_record`;
  - `ids_counter_raised_past_quarantined_ids`;
  - `set_aside_branch_record_quarantines_child_segments` (a stale-epoch line on `main` holding a `branch` record plus that child's segments: every child record is quarantined `STALE_EPOCH`, nothing waits, nothing is applied).
- [x] **Step 2: Run** `rtk cargo test -p fin-store`. Expected: FAIL.
- [x] **Step 3: Implement.** The import files stay ≤ 600 lines; branch absorption lives in `branch/index.rs`.
- [x] **Step 4: Run** the three gates. Expected: PASS.
- [x] **Step 5: Commit** `feat(store): branch-aware import, export order, counters and status`.
- As built: `ForkPoint`/`Known::fork`/`Known::for_branch` and the graph start check as planned (`fork: None`, `main` and `Known::empty`, keeps the 4A checks only); `known::fork_point` reads the index, `known::build` and `known_now` carry it, and the skip key holds the fork point and the set-aside `branch` records of the id. **Worklist** (`import/pass.rs`): `parents_first()`, then branches with rows but no index row; an applied `branch` record pushes its child, whose head comes from a local map (its fork snapshot is uncommitted) and is always adopted after the commit; an index row without any state is skipped (its directory is then a leftover). **Absorption** (deviation from `absorb_applied`): `branch::index::absorb_imported` runs per record *before* its row is inserted (in `import/records.rs` `one`), so an unfit record writes nothing: it folds `branch`/`branchSet` into the index, checks a `branch` record's `forkHash` against `lineage::hash_at(parent, forkSeq)`, and stores the child's fork snapshot (the state before the record when the fork is at `seq - 1`, else `lineage::state_at`). A record it refuses (`Stop::Aside`) is quarantined `MALFORMED` with every later record of the resolution (detail `<segment>: <why>` / `descends from set-aside record <hash12>`), no import error, nothing applied past it; a `forkHash` against `forkSeq` already fails `Record::link` at decode, so it stays the 4A "record cannot be read" import error. **Seeding** generalised to the branch start (`h.rev == fork_seq`, records from `fork_seq + 1`), and a seed never spans a `branch`/`branchSet` record (those are absorbed and replayed), so the review case (a child forked at 0) seeds only like `main` and never from a genesis. **Decision 8** in new `import/aside.rs`: `leftovers` (after the worklist, exact current set: this pass's verdicts and foreign hashes, plus `DIVERGED_TAIL`) quarantines every record of a directory whose `branch` record is a current set-aside row (`STALE_EPOCH`, `SUPERSEDED`, `DIVERGED_TAIL`, `MALFORMED`; found by a `LIKE '%"kind":"branch"%'` prefilter, then parsed) with that reason, origin `folder`, detail `branch record <hash12> set aside`, repeating for grandchildren; the rest is `ImportReport.unknown_branches` / `ProjectStatus.unknownBranches`. **Same id on two servers** (coordinator review): `aside::collisions` runs before resolving a known branch when a set-aside `branch` record made the same id: its segments with an epoch below the live `branch` record's `txs.epoch` are quarantined as above (a set-aside child forked at the same point as the live one would otherwise apply onto it; the live record's epoch is its segment label, an upper bound, so a holder that re-acquired with the record unexported and then collided could see its own older child segments set aside — documented, recoverable). During the worklist "current" is the DB flag (this pass's upserts set it), so a row the pass will clear counts for one pass. **Counters:** `raise_to_log` also reads `branches.id` and `record_links.targets` of `branch`/`branchSet`; `raise_to_quarantine` (meta `quarantineIdsScannedThrough`, by `quarantine.id`) runs on every pass (one indexed read when nothing was added) and reads bindings, resolved-op ids and `branchId`, from the raw JSON when the record does not decode. **Rewind:** the branch start of a 4B branch is `fork_seq + 1` (it deleted the fork snapshot before); rule (d) interim: a lost range holding a `branch`/`branchSet` record is not rewound (`CHAIN_FORK` until Task 10), branches with an index row are otherwise never "based inside", hand-made roots keep the `branchBase` meta rule. **Export:** `pending_branches` in index order, then index-less branches by name. **Status:** `BranchStatus` gains `name, parent, fork_seq, state` and lists `main` first, then by numeric id; RPC fixtures `project-status`, `project-takeover`, `project-events` regenerated (`capture-rpc-fixtures`; the other regenerated files only churned and were reverted). **Task 8 review fixes folded in:** `write::writable` refuses a non-`main` branch without an index row (`DB_ERROR`, rebuild fix; reads still take it as a root); `BranchIndex::absorb` validates names (`validate_branch_name`) and `forkHash.is_some() == (forkSeq > 0)`; default names skip a taken `try-<n>` (`try-<n>-2`, `-3`, …); a `branchSet` that changes nothing is `INVALID_OP` ("nothing to change"); the archived-parent note is in Task 22. Tests: `branch_segment_must_start_at_fork_point`, `branch_start_competition_uses_epoch_rule` (`chain_scenarios.rs`); `tests/branch_follow.rs`: `follower_learns_branch_from_parent_record` (the folder round trip: forks at the head, below it, at 0, a branch of a branch; same index, names, states, heads and status), `branch_set_and_rename_follow`, `export_writes_parent_before_child` (`FailPoint::AfterRename` after the first segment), `export_branch_first_segment_has_no_prev_and_parent_is_fork_hash`, `ids_counter_raised_by_imported_branch_record` (a follower's dry run draws `br2`), `child_segments_before_branch_record_wait`, `set_aside_branch_record_quarantines_child_segments` (with a grandchild), `ids_counter_raised_past_quarantined_ids`, `same_branch_id_on_two_servers_across_a_takeover`, `unfit_branch_records_are_set_aside_not_errors`, `waiting_child_is_listed_in_status_not_read_only`; `tests/branches.rs`: `default_try_name_skips_a_taken_one`, `branch_set_that_changes_nothing_is_refused`, `branch_without_index_row_refuses_writes`.
- **Task 9b (review of 12c26d3, after Task 10):** Decision 33 implemented. The Task 9 epoch-label rule (`aside::collisions`, `aside::records_for`, the `LIKE` scan) and `Stop::Aside` / `set_aside_rest` (which set a refused record *and the rest of its parent's chain* aside, so a follower silently stopped following `main`, review B1) are gone. New `import/branches.rs` `fold` runs after the row is stored: `Made` → index row, fork snapshot, worklist; `Set` → row; `Lost` / refused / bad `forkHash` → inert (`record_links.ignored`); `Replaced` → `cascade::subtree` + `cascade::cascade(Cause::voided)` (which recomputes the index from the log, giving the id to the winner and marking the loser inert), then the winner's fork snapshot and a second resolve of the id in the same pass (`seen` is cleared for a re-born branch). `derived::recompute` makes the same decisions (collision order, bad fork, refusals) from the log alone, re-folding an id whose holder changed. The pass resolves a known branch from the segments naming its index record and keeps every directory whole (`ctx.dirs`, for the cascade's origin rule); `aside::leftovers` classifies all other branch segments after the worklist by one primary-key lookup per named record (no scan, so no per-directory cache or watermark was needed). `index::absorb` (write path) refuses `Replaced`/`Lost` as `DB_ERROR` (a commit only makes new ids). Seeding: the highest seed *below* the first `branch`/`branchSet` record on the chain, not none (review (b)). Review (c): `quarantine.id` stays `INTEGER PRIMARY KEY` (m0001 is frozen); nothing but a rebuild deletes quarantine rows, so it only grows, documented in `import/quarantine.rs`. Review (d): a record on a resolved chain can no longer stall a branch for branch-index reasons (inert instead), and every inert record is in `BRANCH_RECORDS_IGNORED`. Tests: new `tests/branch_collide.rs` — `late_same_id_seen_first_converges` (review E4, real leases: A's late `br1` with an edit reaches F first, B's empty `br1` at the same fork point holds the id; then every arrival order), `reused_id_converges_in_every_arrival_order` (Task 10's dropped case, 24 orders), `same_id_on_different_parents_converges_in_every_order` (review E1/B1, 120 orders), `reacquired_holder_keeps_its_branch_on_followers` (review E2/B2 plus a compaction pack of the branch relabelled at epoch 3, which names the same record), `same_id_on_different_parents_test_lease` (E1 as the reviewer wrote it), `rebuild_seeds_below_the_first_branch_record`; `fin-core` `id_collision_is_held_by_higher_epoch_then_lower_hash`; `branch_follow.rs` `unfit_branch_records_are_inert_rows` (was `…_set_aside_not_errors`). E4 as the reviewer wrote it (test lease) stays `CHAIN_FORK`: the winner's epoch 2 is not backed by `lease.json` (rewind rule (c)), a harness artifact, so it runs on real leases instead.
- For Task 11: `verify`/`rebuild` must check that each branch segment's `branchRecord` is the index's record (or set aside), and compaction's `Known::empty` view should use `Known::for_branch`; the view already keeps only the segments naming the index's record. For Tasks 13–14: entity-id collisions, Decision 33's last point.
- For Task 10: replace the interim rule (d) in `import/rewind.rs` (the `kind IN (branch, branchSet)` refusal) with the cascade; after it and after a diverged tail run `derived::recompute` (a lost `branchSet` leaves the index stale today, which is why the interim refuses). `rewind::eligible` already starts a child's range at `fork_seq + 1`; the cascade deletes the child's fork snapshot and index row itself. `pass.rs` caches heads of branches made this pass off the shared cache; a cascaded branch must leave `heads` too (`new_heads` with `None`). After a cascade the child's folder directory becomes a leftover, so `aside::leftovers` quarantines it with the reason of its (now quarantined) `branch` record — check that the reasons match Decision 7 (`SUPERSEDED`/`DIVERGED_TAIL` from the cascade win; `upsert` keeps those sticky). `aside::records_for` reads the DB `current` flag mid-pass: rows the cascade upserts are seen by later branches of the same pass after `ctx.aside = None`.

### Task 10: Rewind and diverged-tail cascade; derived tables after deletions — tier: opus

> From the Task 7 review: the cascade must bump the **child's** generation as well, never only the parent's. `ops_since_at` returns the child's generation even when it reads parent rows below the fork, so a rewind of those parent rows that leaves the child's generation alone would let a stale `commit_at_generation` on the child through.

**Files:**
- Modify: `crates/fin-store/src/import/rewind.rs` (rule (d) → cascade), `crates/fin-store/src/import/apply.rs` (diverged tail cascade), `crates/fin-store/src/derived.rs`
- Create: `crates/fin-store/src/import/cascade.rs` (`cascade(ctx, lost_branch_records) -> Vec<Rewind>`)
- Test: `crates/fin-store/tests/integration_rewind.rs`, new `crates/fin-store/tests/cascade.rs`

**Interfaces:**
```rust
/// Every descendant whose `branch` record is among `lost` (recursively): rows → quarantine
/// (exported: SUPERSEDED, unexported: DIVERGED_TAIL; 4A origin rule), snapshots, `segments`
/// rows and index rows deleted, generation bumped, `history::record` per branch.
pub(super) fn cascade(ctx: &mut Ctx<'_>, lost: &[(String /*child id*/, u64)]) -> Result<Vec<Rewind>, StoreError>;
```
- Rule (d) is replaced (Decision 7), and the rewind doc comment is updated.
- After any `rewind::run`, diverged tail or cascade, `derived::recompute(txn)` runs before the commit.
- `StoreEvent::Rewound` is emitted per cascaded branch, so finisherd ends its subscribers.

- [x] **Step 1: Write failing tests:**
  - `rewind_cascades_to_child` (the child's rows are quarantined as `SUPERSEDED`, the index row is gone, `Rewound` is emitted for both branches);
  - `rewind_keeps_child_whose_branch_record_survives`;
  - `diverged_tail_with_branch_record_cascades_to_child` (the tail's `branch` record and the child's unexported rows → `DIVERGED_TAIL`);
  - `cascade_recurses_to_grandchildren`;
  - `cascade_converges_in_every_arrival_order` (exhaustive over the three segments: winner, late loser, the loser's child; two followers end with an equal DB state, quarantine set and index);
  - `record_links_follow_rewound_rows`;
  - `subscriber_on_cascaded_branch_gets_history_rewound` (fin-store event level).
- [x] **Step 2: Run** `rtk cargo test -p fin-store --test cascade --test integration_rewind`. Expected: FAIL.
- [x] **Step 3: Implement.**
- [x] **Step 4: Run** the three gates. Expected: PASS.
- [x] **Step 5: Commit** `feat(store): cascade rewinds and diverged tails to descendant branches`.
- As built: new `import/cascade.rs`. **Interface** (deviation from `cascade(ctx, lost: &[(String, u64)])`): `lost_children(conn, branch, from_seq) -> Vec<Child>` reads the index (children with `parent = branch` and `created_seq >= from_seq`, then all their descendants, parents first; `Child` carries the `branch` record's txHash for details) *before* the rows go; `cascade(ctx, dirs, &children, &Cause) -> Vec<Rewind>` rewinds each whole and then always runs `derived::recompute` (now `&Connection`, called in the pass transaction) and resets `ctx.aside`. `dirs` are the pass's folder segments by branch (the child's, for the 4A origin rule), so `apply::branch` takes them too. `rewind::run` was split: `rewind::take_rows(ctx, branch, from_seq, segs, reason, detail)` (quarantine by the origin rule, delete txs/snapshots/`segments` rows; returns `Taken` → `RewindInfo`) serves both. **Per child:** all rows (`take_rows` from 0, the fork snapshot included), `meta` `idsScannedThrough.<id>` and `diffsPending.<id>` deleted (`counters::scanned_key`, `diffs::pending_key` now `pub(crate)`), index row gone by the recompute, `history::record` with the parent's kind (`appliedLine`/`divergedTail`) and winner epoch, **from rev 0**: the whole branch is gone, below its fork too, and a branch the winner made under the same id may fork elsewhere, so a caller holding the old generation is `HISTORY_REWOUND` whatever its base (Task 7 note). Exported child rows' detail: `seq N (epoch e): its branch record <hash12> was set aside`. **Rule (d)** keeps only the hand-made-root `branchBase` rule; **rule (b)** (holder at E) also counts the cascaded children's rows at ≥ E: a holder whose own edits at E sit on a child that would cascade stays `CHAIN_FORK` (as 4A for its own tail on main). **Pass:** a branch skipped for having no state is no longer marked seen, so a `branch` record later in the pass that makes the same id again is followed; every cascaded branch is pushed to `new_heads` with `None` before any later head for the same id. After a cascade the child's folder directory is a leftover and `aside::leftovers` takes it with the reason of its `branch` record; the cascade's rows keep `SUPERSEDED`/`DIVERGED_TAIL` (sticky `upsert`), a follower that never applied the line gets `STALE_EPOCH`: the same quarantine set by hash. **Reusable entry** (coordinator, for the same-id follow-up): `cascade::subtree(conn, id)` (the branch and its descendants) with `Cause::voided(kind, winner_epoch, reason)` quarantines every row under one reason (e.g. `ID_COLLISION`), independent of how the loss was decided; unused until that follow-up (`allow(dead_code)`). Tests (`tests/cascade.rs`): `rewind_cascades_to_child`, `rewind_keeps_child_whose_branch_record_survives`, `diverged_tail_with_branch_record_cascades_to_child` (also: the lost id `br1` is never handed out again), `cascade_recurses_to_grandchildren`, `cascade_converges_in_every_arrival_order` (the 3! orders; states, quarantine set, index and links equal to a follower that saw all at once), `record_links_follow_rewound_rows` (a lost `branchSet` at main seq 3 against the winner's `branch` record at the same seq: without the recompute the `UNIQUE (branch, seq)` of `record_links` would fail), `subscriber_on_cascaded_branch_gets_history_rewound` (the winner reuses `br1` at the same fork; the old generation is refused with `rev 0`), `holder_with_own_edits_on_a_lost_child_does_not_rewind`; all in `cascade.rs` (none added to `integration_rewind.rs`, which keeps its 4A cases unchanged). **Known gap, for the same-id follow-up:** with the winner reusing the loser child's id, an arrival order where the loser's child segment comes before the loser's `branch` record applies it onto the winner's branch (Task 9's epoch-label rule cannot tell yet), and the follower ends with that record applied and the winner's child record not (e.g. the order `x1, yb, xb, y1`, one poll per arrival; the test stopped at the first failing order); the follow-up's classification by branch-record txHash removes it (the exhaustive test over that scenario was dropped from this task).

### Task 11: Verify, rebuild, compaction for branches — tier: opus

**Files:**
- Modify: `crates/fin-store/src/verify/mod.rs` and `crates/fin-store/src/verify/folder.rs` (continuity from `fork_seq + 1`; first parent = `forkHash`; check 9 `branch_index`), `crates/fin-store/src/rebuild/salvage.rs` (`MISSING_FROM_FOLDER` on every branch), `crates/fin-store/src/compact/view.rs` and `crates/fin-store/src/compact/finish.rs` (`Known::for_branch`)
- Test: additions to `crates/fin-store/tests/branch_follow.rs` (or a new `tests/branch_maint.rs`)

- [x] **Step 1: Write failing tests:**
  - `rebuild_restores_branches_and_index` (delete the DB and rebuild: branches, heads, snapshots, index, `merged` and recovered marks are equal; `branch_index` passes);
  - `verify_checks_branch_fork_and_index` (tamper with an index row → `branch_index` fails; a wrong first parent → `chain_continuity` fails);
  - `salvage_marks_missing_branch_rows`;
  - `compaction_packs_branch_segments` (`compact(force)` on `br1`; equivalence holds; the pack version follows its content).
- [x] **Step 2–4:** Run → FAIL; implement; run the three gates → PASS.
- [x] **Step 5: Commit** `feat(store): branch-aware verify, rebuild and compaction`.
- As built: **Verify** (`verify/folder.rs`): each 4B branch is resolved from the segments of its directory that name its index record (`branchRecord`, Decision 33) with `Known::for_branch` at the index's fork point; `main` takes all its segments; a hand-made root (rows, no index row) reads as before; a directory with neither index row nor rows has only "other" segments. `chain_continuity` starts a 4B branch at `forkSeq + 1` with `forkHash` as its first parent (problem `parent_hash is not the branch's forkHash`); roots keep the first-snapshot start. New check 9 **`branch_index`** (`verify/branches.rs`, last in the report; RPC fixtures `project-verify`, `project-rebuild` regenerated): `index` (the `branches` table against `derived::compute`, row by row), `links` (`record_links` against the rows the log's non-edit records give through `db::link_row`, `ignored` marks from `compute`; orphans too), `stateless` (a non-`main` index row with no state: the re-review case below can never be silent), `unindexed` (a branch with rows and no index row: it refuses writes), and `segments` (a directory's other segments must name a record that is inert or in quarantine — their records not yet held are `pending`, not a failure — or one unknown here, `waiting`, a failure unless files are arriving; a live record the index does not hold for that id, a record of another branch or kind, or no `branchRecord` fails). **`derived`** is split: `compute(conn) -> Derived {index, ignored}` reads only (the same fold, collision order, bad forks and refusals), `recompute` writes it; verify compares against `compute`, so verify still changes nothing and takes no write lock. **Rebuild**: no code change was needed (the import pass is branch-aware since Task 9, salvage already keeps every branch's rows, `MISSING_FROM_FOLDER` included); the rebuild report's verify runs `branch_index`. `REBUILD_WOULD_LOSE.exportedThrough` stays `main`'s. A rebuilt follower's quarantine has the same records; reasons are those of a follower that saw everything at once (`STALE_EPOCH` where the old DB had applied and rewound, `SUPERSEDED`): the new DB's own rows win over salvaged ones. `merged`, checkpoints and recovered marks have nothing to compare yet (Tasks 14–16 derive them from the log; their rebuild tests extend `rebuild_restores_branches_and_index`). **Compaction**: the view resolves at the fork point (`Known::for_branch`, also the fresh reader of phase 2's equivalence check), keeps the file names of the directory's other segments (`View::other_record`), and phase 2 abandons a manifest whose pack or replaced segment names another record than the branch's (the holder changed since phase 1); runs are cut where `branchRecord` changes (defensive: the view holds one record's segments). **Re-review of 9b (separate commit 008c6ba):** after every recompute in a cascade, a branch the index holds without any state (its `branch` record holds the id again because the record that took it is gone) gets its fork snapshot and joins the pass's worklist (`branches::restore_stateless`), so the pass resolves it as a fresh follower would; `ID_COLLISION` rows follow the pass's verdicts like `SUPERSEDED` (not sticky; `aside::named` no longer treats them as current on their own), and a voided cascade quarantines unexported rows as `DIVERGED_TAIL` (they exist nowhere else); a branch born under an id (fold or restore) forgets the rewinds recorded under it (`history::forget_rewinds`; the generation stays, so a caller holding the old one is refused from rev 0). Tests: `tests/branch_maint.rs` (`rebuild_restores_branches_and_index`: a fork at the head renamed, one below it archived, one at 0, a branch of a branch, a manual rebuild of the follower and a server without a DB give the same states, `txs`, snapshots, `branches`, `record_links` and quarantine; `verify_checks_branch_fork_and_index`: index drift, link drift and a stray inert mark, a wrong first parent, a stateless index row, a branch segment naming another branch's live record; `compaction_packs_branch_segments`: pack of `br1` at its fork link, format 2, originals deleted after the grace, `main` untouched, followers and a fresh server equal and verified, the branch keeps following), `tests/rebuild_safety.rs` `salvage_marks_missing_branch_rows` (manual rebuild refused with the branch's 2 rows missing; a corrupt rebuild salvages them `MISSING_FROM_FOLDER` and the branch comes back at its fork), `tests/branch_collide.rs` `winner_record_later_superseded_gives_the_id_back` (the reviewer's three-holder case on real leases, then every arrival order, verify, rebuild), `compaction_never_packs_across_branch_records`, and verify plus rebuild equivalence added to `same_id_on_different_parents_converges_in_every_order` and to every all-orders target.
- For Task 12 and later: verify's `branch_index` compares the whole index row (`merged` included) with `derived::compute`, so Task 14's `merged` must be computed there (not only incrementally) or verify fails; new link-bearing kinds need nothing more than `db::link_row`. A new derived table should get a `compute` part and a verify comparison the same way. `history::forget_rewinds` runs only where import makes a branch (`branches::born`); anything else that can make a branch under an id that existed before must call it too.

### Task 12: Undo and redo — tier: opus

> Stale-commit mirrors for undo and redo records are already wired (Task 6, `rebase::steps_from`, from `CommittedTx.link`); this task's real `undo`/`redo` records must carry their `Link` in `CommittedTx.link` (`ops_since`, `txs_since`) and keep each undo's ops as its units' inverses, newest unit first, one op per op. Undo and redo planning reuses `steps_from` for its own intervals.

**Files:**
- Create: `crates/fin-core/src/history/mod.rs` (`LogView`, `Planned`), `crates/fin-core/src/history/undo.rs`, `crates/fin-core/tests/common/memlog.rs` (an in-memory `LogView` with `commit`/`append` helpers)
- Create: `crates/fin-store/src/history_ops/mod.rs` (`SqlLog`), `crates/fin-store/src/history_ops/undo.rs`
- Modify: `crates/fin-store/src/wire.rs` (`HistoryResult`, `Stacks`), `crates/fin-store/examples/export_schema.rs`, `crates/fin/src/output.rs` (`NOTHING_TO_UNDO`, `NOTHING_TO_REDO` → 3)
- Test: `crates/fin-core/tests/undo.rs`, new `crates/fin-store/tests/undo.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone)]
pub struct LogRec { pub branch: String, pub seq: u64, pub record: Record, pub diff: Diff }
pub struct Lineage { pub segs: Vec<(String, u64, Option<u64>)> }
pub trait LogView {
    fn lineage(&self, branch: &str) -> Result<Lineage, CoreError>;
    fn records(&self, branch: &str, after: u64, through: u64) -> Result<Vec<LogRec>, CoreError>;
    fn head(&self, branch: &str) -> Result<(u64, Project), CoreError>;
    fn state_at(&self, branch: &str, seq: u64) -> Result<Project, CoreError>;
    fn find(&self, tx_id: &str) -> Result<Option<LogRec>, CoreError>;
    fn merge_group(&self, merge_id: &str) -> Result<Vec<LogRec>, CoreError>;
    fn is_descendant(&self, a: &str, of: &str) -> Result<bool, CoreError>;
    /// txIds of `branch`'s own records that are the origin of a `merge` record with
    /// `from = branch` on a branch that is not a descendant of `branch` (Decision 22).
    /// Cherry-picks never count.
    fn merged_away(&self, branch: &str) -> Result<HashSet<String>, CoreError>;
}
pub struct Planned { pub link: Link, pub intent: Tx, pub applied: Applied }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct UnitEntry { pub id: String, pub records: Vec<String>, pub last_seq: u64,
    /// The record whose stored `inverse` an undo uses: the unit's records, or the redo
    /// record that recreated it (Decision 23).
    pub source: UnitSource }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UnitSource { Records, Redo { tx_id: String } }
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Stacks { pub undo: Vec<UnitEntry>, pub redo: Vec<UnitEntry> }
pub fn derive_stacks(own: &[LogRec], author: &str, merged_away: &HashSet<String>) -> Stacks;
pub fn plan_undo(log: &dyn LogView, branch: &str, author: &str, tx_id: &str, ids: &mut dyn IdAlloc) -> Result<Planned, CoreError>;
pub fn plan_redo(log: &dyn LogView, branch: &str, author: &str, tx_id: &str, ids: &mut dyn IdAlloc) -> Result<Planned, CoreError>;
```
- **Simulation** (spec §4.5 with Decision 23):
  - A content record by `author`, or the first record of a merge group by `author`, pushes a unit and clears redo.
  - `undo` pops its `undoes` units, and pushes the undo record onto redo.
  - `redo` pops its undo from redo, and pushes **the redo record as a unit** with `source = Redo`.
  - `checkpoint`, `branch` and `branchSet` are skipped.
  - Units in `merged_away` drop the undo stack from that unit down.
- **`plan_undo`:**
  - The ops are the stored inverses of `source` (the unit's records newest first, or the redo record's).
  - The frame is `state_at(branch, last seq of source)`, and `own` is that inverse applied there in `Internal` mode.
  - The steps are the own records after it, with mirrors (Decision 24).
  - Then `rebase_apply(.., Internal)`. The link is `Undo { undoes: [unit id] }`, where the unit id is the redo txId when `source = Redo`.
- **`plan_redo`:** the undo record's stored inverse at its frame; the link is `Redo { redoes }`.
```rust
pub struct HistoryReq { pub tx_id: String, pub author: String, pub branch: String, pub dry_run: bool }
#[derive(Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct HistoryResult { pub committed: Vec<CommittedTx>, pub undone: Vec<String>, pub dry_run: bool }
impl Store {
    pub fn undo(&mut self, r: HistoryReq) -> Result<HistoryResult, StoreError>;
    pub fn redo(&mut self, r: HistoryReq) -> Result<HistoryResult, StoreError>;
    pub fn undo_stacks(&self, branch: &str, author: &str) -> Result<Stacks, StoreError>;
}
```
`SqlLog` implements `LogView` over the write transaction.

- [ ] **Step 1: Write failing tests.**
  - `fin-core/tests/undo.rs`:
    - `stack_simulation_table`;
    - `new_edit_clears_redo`;
    - `merge_group_is_one_unit`;
    - `merged_away_drops_stack_only_for_merge_into_non_descendant` (a merge into a child does not drop; a cherry-pick does not drop);
    - `redo_is_a_unit_with_redo_source`;
    - `undo_maps_over_later_insert_before_it`;
    - `undo_of_undone_pair_is_exact_via_mirrors`.
  - `fin-store/tests/undo.rs`:
    - `undo_appends_inverse_and_restores_state`;
    - `undo_twice_then_redo_once`;
    - `undo_redo_undo_again` (the state after the third step equals the state before the edit; redo again restores it; `undoes == [<redo txId>]`);
    - `undo_rebases_over_other_authors_edit`;
    - `undo_conflicts_when_other_author_removed_target`;
    - `redo_after_new_edit_is_nothing_to_redo`;
    - `nothing_to_undo_on_fresh_branch`;
    - `undo_on_follower_is_read_only`;
    - `undo_dry_run_on_follower_plans`;
    - `undo_retry_same_tx_id_returns_first`;
    - `undo_replays_on_follower`.
- [ ] **Step 2–4:** Run (`rtk cargo test -p fin-core --test undo`, `-p fin-store --test undo`) → FAIL; implement; the three gates → PASS.
- [ ] **Step 5: Commit** `feat(history): undo and redo derived from the log`.

### Task 13: Merge — tier: opus

**Files:**
- Create: `crates/fin-core/src/history/merge.rs` (common point, candidate steps, `plan_merge`), `crates/fin-core/src/history/candidates.rs`
- Create: `crates/fin-store/src/history_ops/merge.rs` (`Store::merge`)
- Modify: `crates/fin-store/src/segment.rs` (test hook `#[cfg(any(test, feature = "test-hooks"))] pub fn set_max_records_for_test(n: usize)`; `MAX_RECORDS` read through `max_records()`), `crates/fin-store/src/export/rows.rs` (uses `max_records()`), `crates/fin-store/src/wire.rs` (`MergeReport`), `crates/fin-store/examples/export_schema.rs`
- Test: `crates/fin-core/tests/merge.rs`, new `crates/fin-store/tests/merge.rs`

**Interfaces:**
```rust
pub fn common_point(a: &Lineage, b: &Lineage) -> (String, u64);
pub struct Candidates { pub picks: Vec<usize>, pub excluded: Vec<(usize, &'static str /*UNDOES_TARGET_RECORDS*/)> }
pub fn candidates(source_path: &[LogRec], target_path: &[LogRec]) -> Candidates; // Decision 19
pub fn candidate_steps(source_path: &[LogRec], i: usize, target_path: &[LogRec],
                       planned: &[(usize, CommittedTx)]) -> Vec<Step>;
#[derive(Debug, Clone, Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct MergePlan { pub merge_id: String, pub planned: Vec<Planned>, pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>, pub excluded: Vec<Conflict> }
pub fn plan_merge(log: &dyn LogView, from: &str, into: &str, author: &str, merge_id: &str,
                  skip_conflicting: bool) -> Result<MergePlan, CoreError>;
```
- **Re-running a candidate:**
  - `BoundIds` from the source record, with full policy: `Client`, or `Internal` for undo/redo/revert candidates.
  - The copy's link is `Merge { merge_id, from, origin }` and its txId `<mergeId>-<n>`.
  - `intent.author` and the record author are the merging `author` (Decision 20); `bindings` are the source's.
  - `DUPLICATE_ID` → `ALREADY_PRESENT` or `ID_COLLISION` (Decision 18).
- **Conflicts:** a failing candidate is recorded and treated as skipped. Its dependents → `DEPENDS_ON_SKIPPED`.
```rust
pub struct MergeReq { pub tx_id: String, pub author: String, pub from: String, pub into: String,
    pub skip_conflicting: bool, pub dry_run: bool }
#[derive(Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct MergeReport { pub merge_id: String, pub committed: Vec<CommittedTx>, pub merged: Vec<String>,
    pub skipped: Vec<Conflict>, pub excluded: Vec<Conflict>, pub source_state: BranchState, pub dry_run: bool }
impl Store { pub fn merge(&mut self, r: MergeReq) -> Result<MergeReport, StoreError>; }
```

- [ ] **Step 1: Write failing tests.**
  - `fin-core/tests/merge.rs`:
    - `common_point_parent_child_and_siblings`;
    - `candidates_exclude_merged_cherry_picked_and_merge_records`;
    - `source_undo_of_merge_from_target_is_excluded`;
    - `candidate_steps_mirror_restores_inside_positions`;
    - `skipped_insert_makes_dependent_depends_on_skipped`;
    - `merge_copies_keep_ids_and_take_merging_author`.
  - `fin-store/tests/merge.rs`:
    - `merge_child_without_target_edits_applies_all`;
    - `merge_rebases_over_target_edits_on_same_track`;
    - `merge_conflict_returns_full_list_and_commits_nothing`;
    - `skip_conflicting_merges_rest_and_reoffers_skipped`;
    - `merge_records_share_merge_id_and_txids_are_derived`;
    - `merge_retry_same_merge_id_is_idempotent`;
    - `merge_is_all_or_nothing_on_crash`;
    - `follower_applies_partial_merge_prefix` (`set_max_records_for_test(2)`, so a 3-record merge spans 2 segments; deliver only the first);
    - `update_branch_from_main` (`merge main --into br1`);
    - `merge_both_directions` (main → br1, then br1 → main: no record applied twice; the states are equal);
    - `merge_dry_run_on_follower_plans`.
- [ ] **Step 2–3:** Run → FAIL; implement (`merge.rs` and `candidates.rs` each ≤ 600 lines).
- [ ] **Step 4: Measure the budget (second time):** `rtk proxy bash -c 'time cargo test -p fin-core --test rebase_props --test shift_map --test undo --test merge'`. Record it; it must be < 15 s.
- [ ] **Step 5:** Run the three gates → PASS. **Commit** `feat(history): merge through the rebase engine`.

### Task 14: Cherry-pick, revert, merged state — tier: opus

**Files:**
- Modify: `crates/fin-core/src/history/merge.rs` (`plan_cherry_pick`, `plan_revert`), `crates/fin-store/src/history_ops/merge.rs`, `crates/fin-store/src/derived.rs` (`merged`, Decision 21)
- Test: `crates/fin-store/tests/merge.rs` (additions)

**Interfaces:**
```rust
pub fn plan_cherry_pick(log: &dyn LogView, origin: &str, into: &str, author: &str, tx_id: &str) -> Result<Planned, CoreError>; // NOT_FOUND {kind:"tx"}
pub fn plan_revert(log: &dyn LogView, merge_id: &str, branch: &str, author: &str, tx_id: &str,
                   ids: &mut dyn IdAlloc) -> Result<Planned, CoreError>; // NOT_FOUND {kind:"merge"}
pub struct CherryPickReq { pub tx_id: String, pub author: String, pub origin: String, pub into: String, pub dry_run: bool }
pub struct RevertReq { pub tx_id: String, pub author: String, pub merge_id: String, pub branch: String, pub dry_run: bool }
impl Store {
    pub fn cherry_pick(&mut self, r: CherryPickReq) -> Result<HistoryResult, StoreError>;
    pub fn revert(&mut self, r: RevertReq) -> Result<HistoryResult, StoreError>;
}
```
`revert` takes the merge group's inverses newest first; its frame is after the group's last record, and its steps are the target records after that.

- [ ] **Step 1: Write failing tests:**
  - `cherry_pick_single_record`;
  - `cherry_pick_excluded_from_later_merge`;
  - `cherry_pick_does_not_drop_source_stack`;
  - `revert_merge_group_newest_first`;
  - `revert_may_conflict`;
  - `undo_merge_is_one_unit_on_target`;
  - `source_stack_dropped_after_merge_into_parent`;
  - `merged_state_when_nothing_left`;
  - `new_edit_reactivates_merged_branch`;
  - `follower_recomputes_merged_state` (the follower imports the merge records → `branches()` shows `merged`);
  - `cherry_pick_and_revert_dry_run`.
- [ ] **Step 2–4:** Run → FAIL; implement; the three gates → PASS.
- [ ] **Step 5: Commit** `feat(history): cherry-pick, revert and derived merged state`.

### Task 15: Checkpoints and `undo --to` — tier: opus

**Files:**
- Create: `crates/fin-core/src/history/checkpoint.rs` (`plan_undo_to`), `crates/fin-store/src/history_ops/checkpoint.rs`
- Modify: `crates/fin-store/src/history_ops/undo.rs` (`to`), `crates/fin-store/src/branch/create.rs` (`At::Checkpoint`)
- Test: `crates/fin-core/tests/undo.rs`, new `crates/fin-store/tests/checkpoints.rs`

**Interfaces:**
```rust
pub fn plan_undo_to(log: &dyn LogView, branch: &str, author: &str, checkpoint_seq: u64, tx_id: &str,
                    ids: &mut dyn IdAlloc) -> Result<Planned, CoreError>;
pub struct CheckpointReq { pub tx_id: String, pub author: String, pub branch: String, pub name: String, pub dry_run: bool }
impl Store {
    pub fn checkpoint(&mut self, r: CheckpointReq) -> Result<CommittedTx, StoreError>;
    pub fn find_checkpoint(&self, branch: &str, name: &str) -> Result<(String, u64), StoreError>; // lineage, newest first
}
pub struct HistoryReq { /* Task 12 */ pub to: Option<String> }
```
`plan_undo_to` pops undo units while `last_seq > checkpoint_seq` (all own units when the checkpoint is at or before the fork). Each unit's source inverse goes in newest first, mapped over the later records and over the inverses already planned (mirrored). The result is one `Undo { undoes }` record, all or nothing.

- [ ] **Step 1: Write failing tests:**
  - `checkpoint_record_has_no_ops_and_is_skipped_by_stacks`;
  - `checkpoint_names_unique_per_branch`;
  - `branch_from_checkpoint`;
  - `branch_from_seq_and_checkpoint_of_another_branch` (`main@v1` from `br1`);
  - `undo_to_checkpoint_one_record_newest_first`;
  - `undo_to_checkpoint_skips_other_authors_and_rebases_over_them`;
  - `undo_to_includes_redo_units`;
  - `redo_reverses_whole_undo_to`;
  - `undo_to_conflict_is_all_or_nothing`;
  - `undo_to_before_fork_undoes_all_own`;
  - `unknown_checkpoint_is_not_found`;
  - `checkpoint_dry_run`.
- [ ] **Step 2–4:** Run → FAIL; implement; the three gates → PASS.
- [ ] **Step 5: Commit** `feat(history): checkpoints and undo --to`.

### Task 16: Recovered branches — tier: opus

**Files:**
- Create: `crates/fin-store/src/recover/mod.rs` (groups, `Store::recover`), `crates/fin-store/src/recover/remap.rs` (fresh ids), `crates/fin-store/src/recover/import_dir.rs`
- Modify: `crates/fin-store/src/status/warnings.rs` (a row is recovered when `record_links.origin_hash` names it, Decision 5; recovered rows are not counted; the `DIVERGED_TAIL` text names `fin project recover`), `crates/fin-store/src/import/quarantine.rs` (`IMPORTED`), `crates/fin-store/src/wire.rs`, `crates/fin-store/examples/export_schema.rs`
- Test: new `crates/fin-store/tests/recover.rs`

**Interfaces:**
```rust
#[derive(Debug, Clone, Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct RecoverGroup { pub id: String, pub branch: String, pub fork_seq: u64, pub fork_hash: Option<String>,
    pub records: usize, pub reasons: BTreeMap<String, usize>, pub origin: String, pub server: String,
    pub first_committed_at: i64, pub name: String, pub children: Vec<String>,
    pub recoverable: bool, pub problems: Vec<Conflict> /* ID_COLLISION, no common history */ }
pub enum RecoverSel { All, Group(String) }
pub struct RecoverReq { pub tx_id: String, pub author: String, pub create: Option<RecoverSel>,
    pub import_dir: Option<PathBuf>, pub dry_run: bool }
#[derive(Debug, Clone, Serialize, JsonSchema)] #[serde(rename_all = "camelCase")]
pub struct RecoverReport { pub groups: Vec<RecoverGroup>, pub created: Vec<BranchView>,
    pub committed: Vec<CommittedTx>, pub imported: usize, pub manual: Option<String> }
// remap.rs
pub(crate) struct Remap { map: BTreeMap<String, String> }
impl Remap {
    /// `rec.intent` with every id its bindings created turned back into a `$placeholder`, and
    /// every literal id in `remap` rewritten; `ID_COLLISION` for a literal id neither remapped
    /// nor at the fork state that the DB holds.
    pub(crate) fn prepare(&mut self, rec: &Record, fork_state: &Project, db_ids: &dyn Fn(&str) -> bool) -> Result<Tx, Conflict>;
    pub(crate) fn learn(&mut self, rec: &Record, new_bindings: &BTreeMap<String, String>);
}
impl Store {
    pub fn recover_groups(&self) -> Result<Vec<RecoverGroup>, StoreError>;
    pub fn recover(&mut self, r: RecoverReq) -> Result<RecoverReport, StoreError>;
}
```
- **`create`**, per group, in one `write`:
  1. `create_branch` at the fork point.
  2. Each record goes through `Remap::prepare`, then `apply` with `Counters` in `Internal` mode (full policy), then is appended as `CherryPick { origin, from, origin_hash }`; then `Remap::learn`.
  3. Child groups follow, recursively (Decision 27).

  The names get `-2`, `-3`, … on a collision.
- **`--import`** is allowed while read-only. A dry run lists without writing.

- [ ] **Step 1: Write failing tests:**
  - `diverged_tail_becomes_recovered_branch`;
  - `recovered_tail_gets_fresh_ids_after_takeover` (server A's tail used `c10`; server B, after the takeover, allocated `c10` too; the recovered copy gets a new id, and later records in the group that named `c10` use the new id);
  - `literal_id_collision_is_reported_and_group_not_created`;
  - `superseded_rows_after_rewind_become_branch`;
  - `set_aside_child_branch_recovers_as_child_group`;
  - `groups_split_by_parent_chain`;
  - `group_without_common_history_is_listed_not_created`;
  - `recovered_marks_rebuild_from_log` (rebuild: still not counted);
  - `recovered_marks_follow_to_followers`;
  - `import_dir_reads_moved_segments_into_quarantine` (a `CHAIN_FORK`, one side moved out, `--import`, then `--create all`: the fork is gone and the branch holds that side);
  - `recover_retry_same_tx_id_is_idempotent`;
  - `recover_names_unique_within_one_second`.
- [ ] **Step 2–4:** Run → FAIL; implement; the three gates → PASS.
- [ ] **Step 5: Commit** `feat(store): recovered-* branches with fresh ids`.

### Task 17: fin-node bindings — tier: sonnet

**Files:**
- Create: `crates/fin-node/src/history.rs` (a second `#[napi] impl Engine` block)
- Modify: `crates/fin-node/src/tasks.rs` (`Call` variants), `crates/fin-node/src/lib.rs`, `packages/fin-node/index.js`, `packages/fin-node/index.d.ts`
- Test: new `packages/fin-node/test/history.test.ts`

**Interfaces:**
- **Short calls** (`AsyncTask`): `BranchCreate { req_json }`, `Branches { all }`, `BranchSet { req_json }`, `ResolveBranch { name }`, `Undo`, `Redo`, `Checkpoint`, `Merge`, `CherryPick`, `Revert` (each `{ req_json }`).
- **Long call** (worker): `recover`.
- **napi:** `branchCreate(reqJson)`, `branches(all)`, `branchSet(reqJson)`, `resolveBranch(name)`, `undo(reqJson)`, `redo(reqJson)`, `checkpoint(reqJson)`, `merge(reqJson)`, `cherryPick(reqJson)`, `revert(reqJson)`, `recover(reqJson)`. Each returns `Promise<string>` (JSON). The request JSON is the camelCase form of the Rust request structs (they derive `Deserialize`).

- [ ] **Step 1:** Write `history.test.ts`: branch create/list/set; commit on a branch; merge; undo; redo; checkpoint; cherry-pick; revert; recover list; a dry run on a follower engine (a second server-data).
- [ ] **Step 2–4:** Run (`rtk pnpm --filter @finisher/fin-node build && rtk pnpm --filter @finisher/fin-node build:test && rtk pnpm test`) → FAIL; implement; the three gates → PASS.
- [ ] **Step 5: Commit** `feat(fin-node): branch and history bindings`.

### Task 18: Contracts, finisherd routing, RPC fixtures — tier: sonnet

**Files:**
- Create: `packages/contracts/src/history.ts`, `apps/finisherd/src/history.ts`
- Modify: `packages/contracts/src/rpc.ts`, `packages/contracts/src/index.ts`, `packages/contracts/src/index.test.ts`, `packages/contracts/src/envelopes.test.ts`, `apps/finisherd/src/handlers.ts`, `apps/finisherd/src/projects.ts` (resolve the branch before `snapshot`/`subscribe`; hubs keyed by id), `apps/finisherd/src/recover.ts`, `apps/finisherd/scripts/capture-rpc-fixtures.ts`
- Test: new `apps/finisherd/test/history.test.ts`, regenerated `crates/fin/tests/fixtures/rpc/*`

**RPCs** (the error type is `FinError` throughout):

| Tag | Payload | Success |
|---|---|---|
| `branch.create` | `{projectId, txId, author, name?, from, at?: {seq} \| {checkpoint}, dryRun?}` | `BranchCreated` |
| `branch.list` | `{projectId, all?}` | `{branches: BranchView[]}` |
| `branch.set` | `{projectId, txId, author, branch, name?, archived?, dryRun?}` | `CommittedTx` |
| `history.undo` | `{projectId, txId, author, branch, to?, dryRun?}` | `HistoryResult` |
| `history.redo` | `{projectId, txId, author, branch, dryRun?}` | `HistoryResult` |
| `history.checkpoint` | `{projectId, txId, author, branch, name, dryRun?}` | `CommittedTx` |
| `history.merge` | `{projectId, txId, author, from, into, skipConflicting?, dryRun?}` | `MergeReport` |
| `history.cherryPick` | `{projectId, txId, author, origin, into, dryRun?}` | `HistoryResult` |
| `history.revert` | `{projectId, txId, author, mergeId, branch, dryRun?}` | `HistoryResult` |
| `project.recover` | `{projectId, txId?, author?, create?: "all" \| string, importDir?, dryRun?}` | `RecoverReport` |

- The TS types come from `@finisher/core-types`. `BranchViewSchema` and `ConflictSchema` are checked in `envelopes.test.ts`; the larger results are `Schema.Unknown` narrowed to the generated type.
- **Publishing:** each call runs under `commitLock`. Afterwards every `CommittedTx` in the result (`record`, `committed`, or the bare result) is published with `hubs.publishTx(projectId, c)` to `c.tx.branch`, in rev order. A dry run publishes nothing.
- **Endings:** `Rewound` events for cascaded branches (Task 10) end those branches' subscribers, through the existing endings path.

- [ ] **Step 1: Write failing tests** (`apps/finisherd/test/history.test.ts`):
  - `branch.create then branch.list (all)`;
  - `commit on a branch publishes only to that branch`;
  - `merge publishes each record to the target hub in rev order`;
  - `undo conflict is CONFLICT with details.conflicts`;
  - `dryRun publishes nothing`;
  - `ops.subscribe by branch name resolves to the id`;
  - `project.recover create publishes committed records`.
  - `index.test.ts`: the new tags. `envelopes.test.ts`: the new schemas.
- [ ] **Step 2–3:** Run → FAIL; implement. Add the capture scenarios `branch-create`, `branch-list`, `branch-set`, `history-undo`, `history-redo`, `history-merge`, `history-merge-conflict`, `history-cherry-pick`, `history-revert`, `history-checkpoint` and `project-recover-groups`; regenerate the fixtures.
- [ ] **Step 4:** Run `rtk cargo test --workspace` (`rpc_contract.rs` replays the fixtures) and the TS gates → PASS.
- [ ] **Step 5: Commit** `feat(rpc): branch and history RPCs, routed per branch`.

### Task 19: `fin` commands, global `--branch`, `try` — tier: sonnet

**Files:**
- Create: `crates/fin/src/commands/branch.rs`, `crates/fin/src/commands/history.rs`, `crates/fin/src/commands/try_cmd.rs`, `crates/fin/src/config.rs`
- Modify: `crates/fin/src/main.rs` (`Cmd`, parse samples), `crates/fin/src/registry.rs`, `crates/fin/src/commands/mod.rs` (`Globals.branch`; `Builder::tx` branch and author), `crates/fin/src/backend.rs` (`snapshot` branch), `crates/fin/src/commands/project.rs` (recover args; `acknowledge` uses the global `--branch`), `crates/fin/src/output.rs`
- Test: new `crates/fin/tests/cli_history.rs`, `crates/fin/tests/cli_offline.rs`, `crates/fin/tests/rpc_contract.rs`

**Interfaces:**
- **`Globals.branch: Option<String>`** (`--branch`, global). Resolution: the flag, then `config::default_branch`, then `"main"`.
- **`config.rs`:** `default_branch(server_data, project_id) -> Option<String>`, `set_default_branch(server_data, project_id, Option<&str>)`, `record_try(server_data, branch_id, parent_id)`, `try_parent(server_data, branch_id) -> Option<String>`. The file is `{defaultBranch: {}, tries: {}}`, written temp-then-rename, 0600 on unix.
- **Commands** (one JSON document each; `--dry-run` everywhere it writes; `--idem-key` per Decision 26):
  - `fin branch new <name> [--from <b>[@<seq|checkpoint>]]` → `{branch}`
  - `fin branch ls [--all]` → `{branches}`
  - `fin branch rename <old> <new>` → `{rev}`
  - `fin branch archive <b> [--restore [--as <new>]]` → `{rev}`
  - `fin checkpoint <name>` → `{rev}`
  - `fin undo [--to <checkpoint>]` → `{undone, rev}`
  - `fin redo` → `{redone, rev}`
  - `fin merge <branch> [--into <b>] [--skip-conflicting]` → `{merge}`; a conflict exits 4 with `details.conflicts`
  - `fin cherry-pick <txId> [--into <b>]` → `{rev}`
  - `fin revert <mergeId>` → `{rev}`
  - `fin try [name]` → `{branch}`: records the try and switches the default
  - `fin try keep` → `{merge}`: merges into `try_parent` and switches back
  - `fin try drop` → `{branch}`: archives and switches back. Not on a recorded try branch → `USAGE`.
  - `fin project recover [--create <group|all>] [--import <dir>]` → `{groups, created, imported, manual}`
- **`@` parsing:** all digits means a seq; anything else names a checkpoint.

- [ ] **Step 1: Write failing tests** (`cli_history.rs`, live server):
  - `branch_new_ls_rename_archive`;
  - `branch_new_from_seq_and_checkpoint`;
  - `branch_ls_all_shows_archived`;
  - `archive_restore_as`;
  - `global_branch_flag_targets_writes_and_views`;
  - `undo_redo_roundtrip`;
  - `undo_to_checkpoint`;
  - `merge_with_conflict_exit_4_lists_conflicts`;
  - `merge_skip_conflicting`;
  - `cherry_pick_and_revert`;
  - `try_without_name_then_keep`;
  - `try_drop_archives`;
  - `try_keep_outside_try_is_usage`;
  - `fin_author_env_scopes_undo`;
  - `recover_lists_groups`;
  - `idem_key_retry_merge_returns_first`;
  - `dry_run_each_history_command_writes_nothing` (a table over every write command).
  - `main.rs`: parse samples. `cli_offline.rs`: the map under 1,500 tokens.
- [ ] **Step 2–4:** Run (`rtk cargo test -p fin`) → FAIL; implement (each command file ≤ 400 lines); the three gates and `rtk pnpm e2e` → PASS.
- [ ] **Step 5: Commit** `feat(fin): branch, merge, cherry-pick, revert, undo, redo, checkpoint, try`.

### Task 20: Property suite and time budget — tier: opus

**Files:**
- Create: `crates/fin-core/tests/props_apply.rs` (properties 1 and 6), `crates/fin-core/tests/props_history.rs` (property 4), `crates/fin-store/tests/props_replay.rs` (property 5)
- Modify: `crates/fin-core/tests/rebase_props.rs` (properties 2 and 3), `crates/fin-core/tests/common/memlog.rs`, `crates/fin-core/tests/ops_insert.rs`, `crates/fin-core/tests/ops_trim_delete.rs` (move duplicates of property 1), `TASKS.md`

**Properties** (spec §4.6):
1. `apply_then_inverse_is_exact_for_every_op_kind` (256 cases), internal ops and `$` restore ids included.
2. `rebase_over_empty_interval_is_identity` (256).
3. `stale_and_fresh_agree_multi_track` (128; 3 tracks, 0–6 committed edits).
4. `undo_redo_matches_reference_model` (64), with this concrete model:
   - **Generated edits:** gapless ripple tracks only: inserts at a clip boundary or exactly the track end, deletes, end trims. A track's state is then exactly its ordered list of `(clipId, asset, sourceIn, duration)`; positions are prefix sums.
   - **Driver:** 2–4 authors each run 0–12 steps, interleaved, on `main` and one side branch: edit, undo, redo, cherry-pick of a side-branch record, and merge of the side branch.
   - **Model:**
     - per-author undo and redo stacks of unit ids, following Decisions 22–23;
     - per branch, the ordered clip lists;
     - an inverse per unit, as list operations: remove an id, reinsert an id before its recorded successor id (or at the end), set a duration.
   - **Predictions:**
     - undo of an insert fails `REMOVED` if another author deleted the clip;
     - undo of a trim fails `SAME_EDGE` if another author later trimmed the same end, and `REMOVED` if the clip is gone;
     - undo of a delete reinserts before the recorded successor while it is present on the same track; otherwise the model checks membership and durations only;
     - copies by merge or cherry-pick apply the same list operation on the target, and fail exactly when the model's operation is impossible.
   - **Check:** after every step, the real `MemLog` head (built with the real planners) has, per track, the model's id order and durations, and every `CONFLICT` reason equals the model's prediction.
5. `replay_and_export_import_roundtrip` (16, fin-store, tempdir): a generated log with branches, merges, undos and a checkpoint. Replaying each branch from its first snapshot equals the live head. A fresh server-data that imports the folder has equal heads, index and `merged` state.
6. `apply_tx_never_panics_and_success_keeps_invariants` (512).

- [ ] **Step 1:** Write the files. Seed a bug locally (flip the recover edge in `map.rs`), confirm that properties 3 and 4 fail, and revert.
- [ ] **Step 2: Measure:** `rtk proxy bash -c 'time cargo test -p fin-core --test props_apply --test props_history --test rebase_props --test shift_map'` plus `rtk proxy bash -c 'time cargo test -p fin-store --test props_replay'`. The total must be < 30 s in debug; record both numbers in TASKS.md.
- [ ] **Step 3:** Run the three gates → PASS. **Commit** `test(core): 4B property suite`.

### Task 21: e2e history scenario — tier: sonnet

**Files:**
- Create: `scripts/e2e/history.mjs`
- Modify: `scripts/e2e.mjs` (`SCENARIOS.history`), `scripts/e2e.test.mjs`

- [ ] **Step 1:** Write the scenario. The asset is the e2e fixture MP4, which has video and audio (`writeFixture`, `audio: true`), so `check_kind` accepts it on `t1` (video) and `t2` (audio).
  1. Create a project; `asset add`; insert two clips on `t1`.
  2. `fin branch new feature`. Then, with `--branch feature`: insert on `t2`, and trim `c1`'s end by +1s.
  3. On `main`: trim `c1`'s end by −1s, and insert on `t1` at the end.
  4. `fin merge feature` → exit 4, one conflict `SAME_EDGE`.
  5. `fin merge feature --skip-conflicting` → exit 0: one merged, one skipped.
  6. `fin undo` (the same `FIN_AUTHOR` as the merge) → the merge unit is undone.
  7. `snapshot.get main` equals the snapshot saved after step 3. `fin branch ls` shows `feature` `active`.
  8. A WS `ops.subscribe` on `main`, started after step 3, received the merge record and then the undo record, in order.
- [ ] **Step 2: Run** `rtk pnpm e2e`. Expected: all three scenarios pass.
- [ ] **Step 3: Commit** `test(e2e): history scenario — branch, conflicting merge, skip, undo`.

### Task 22: Spec and tracker — tier: sonnet

**Files:**
- `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md`:
  - §4 as built: Decisions 1–32, codes, RPC table, commands, header state line, §4.8 "Validation results (Task 23) — pending";
  - **§3.2:** `clip.restore` may name its clip with a `$` id, which replay hands out through `BoundIds`; the record field list gains the kind fields in Decision 1's order;
  - **§3.4:** segment version by content, and per-kind reader versions;
  - **§3.6.2 step 6a (d):** cascade instead of refusal;
  - **§4.2:** `undoes` is a list; `branch` adds `forkSeq`/`forkHash`; `cherryPick` adds `originHash`;
  - **§4.5:** redo as a unit, and dropping only on a `merge` into a non-descendant.
- `docs/specs/2026-11-01-finisher-architecture-design.md`: a §27 pointer for 4B.
- `TASKS.md`:
  - tick the 4B section;
  - tick the 4A follow-ups this closes: "Rebase: per-track conservatism…", "Rewind rule (d) needs 4B…", and, partially, "A sync folder with losing segments re-reports the fork… until 4B recover";
  - add the follow-up "in-band fork resolution record kind (Decision 29)".
- `README.md`: "Run it" gains `fin branch`, `fin merge` and `fin undo`.
- **§4.1** (from the Task 8 review): an archived parent still takes `branchSet` records (a child of an archived branch can be renamed, archived or restored) but refuses `branch` records (no new branches from it) and content (`BRANCH_ARCHIVED`). Also record Task 9/9b's import rules: a set-aside `branch` record takes the segments that name it along (Decision 8), and Decision 33 (the `epoch` field, id collisions, inert records, `branchRecord` in segment headers, `ID_COLLISION`, `BRANCH_RECORDS_IGNORED`) goes into §3.4, §3.6.2 and §4.1–4.2.

- [ ] **Step 1:** Write the edits. **Step 2: Run** `rtk pnpm gates` → ok. **Step 3: Commit** `docs: record 4B decisions, formats, RPCs and commands`.

### Task 23: Cross-platform and cross-host validation (controller-run; needs hosts) — tier: opus (controller)

**Files:**
- `crates/fin-store-xhost/src/main.rs` and `crates/fin-store-xhost/src/node.rs` (subcommands `brancher` and `merger`)
- `tests/storage-xhost/run.mjs`, `tests/storage-xhost/analyse.mjs` (row `history`)
- `tests/storage-xhost/README.md` (no host details)
- Results go to `archive/spike/sp4b-xhost/`; durable conclusions go into spec §4.8.

**Interfaces:**
- `xhost brancher --dir D --secs N`: hold the lease, create branch `x`, commit N edits alternating between `x` and `main` (one same-edge conflict included), export, then stop without closing.
- `xhost merger --dir D --branch x`: open, wait for the takeover, `merge x --skip-conflicting`, export, then emit `{merged, skipped, stateHash, indexHash}` as JSONL.
- `xhost verify` (existing) gains per-branch heads and the index hash.

- [ ] **Step 1:** Run the full suites on macOS, Windows 11 and the Linux VM: `cargo test --workspace` (`FIN_REQUIRE_SERVER=1`), `pnpm test`, `pnpm gates`, `pnpm e2e`. Record the counts.
- [ ] **Step 2:** Run the `history` row for each NAS pair (Mac↔Windows, Mac↔Linux, Windows↔Linux), 5 runs each. Required: 0 lost records, the skipped record reported, and the same `stateHash`/`indexHash` on both hosts after one follow cycle.
- [ ] **Step 3:** Run a **mixed-version row**: a 4A follower and a 4B holder on one folder. Edit-only use keeps the 4A follower following. The first branch record makes it read-only `NEWER_FORMAT` (Decision 4).
- [ ] **Step 4:** Re-run the 4A race row (20 per pair) and the stall row: 0 double holders, 0 applied stale writes.
- [ ] **Step 5:** Fix any defect, each with a regression test in the owning crate, and re-run the affected rows. Record the conclusions in spec §4.8 and TASKS.md. **Commit** `test(storage): cross-host history row; record 4B validation`.
