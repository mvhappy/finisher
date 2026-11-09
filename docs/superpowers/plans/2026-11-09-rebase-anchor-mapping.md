# Rebase anchor mapping — Implementation Plan

> **For agentic workers:** steps use checkbox (`- [ ]`) syntax. Every task is TDD: write the failing test, run it and see it fail, implement, see it pass, then run the task's suites. Each task gets a review by a fresh subagent before the next one starts.

**Goal:** when a stale op is rebased, a position it names maps through the **item edge it was written against**, resolved where that edge is at the evolving head. It falls back to `Diff.shifts` only when there is no anchor or the anchor is gone. This replaces shift-only mapping (spec: `docs/specs/2026-11-04-sub-project-4-core-breadth-design.md` §4.3, *Design note: shift-only position mapping*, 2026-11-09, commit `11f86595`). Owner decision, 2026-11-09: redesign now, and stop carving out.

**Acceptance:**
- The seven open seeds pass as named tests under the strict oracle: `cc 5cee31d6`, `1c600fdf`, `3130272a`, `aa390d30`, `1477599b`, `c65a9221` (in `rebase_moves_props`) and `cc 5b02bc6b` (in `props_rebase_4c`).
- Every oracle carve-out the redesign makes unnecessary is gone. Each removal is backed by its named fixed case, which passes with the carve-out removed. A carve-out that is still needed stays and is documented as a true limit.
- `rebase_moves_props` and `props_rebase_4c` pass 100k cases × 5 runs each. Every fin-core property suite passes at 2000 cases, and so do fin-store `chain_props` and `props_replay`.
- Stored ops and inverses stay absolute: the rebased tx records where each op landed. The wire format, replay and inverses are unchanged, as with `rebase::places`.

## Design decisions

### D1. What an anchor is (derived in the op's own frame)

For each position `(track T, p)` an op names (`Op::positions_mut`: `item.insert.at`, `item.move.at`, `item.restore.item.start`, and their `clip.*` aliases), the planner reads its **frame** F: the frame the rebase already has, with our earlier ops applied, exactly as `places::places` reads it.

1. **`Start(B)`**: some item B on T starts at p. A cut is both the end of A and the start of B, and `Assoc::After` picks B: what is inserted at the cut goes before us, so we stick to what follows.
2. **`End(A)`**: no item starts at p, and some item A on T ends at p (the track's last item, or an item followed by a gap).
3. **`Empty`**: T has no items in F.
4. **none**: p is in a gap away from every edge, or inside an item (an overwrite into its middle). The position maps through shifts, as today.
5. **Lift restores take no anchor.** An `item.restore` with `mode: lift` refills the exact time span it was taken from. These appear in the inverses of overwrite inserts, moves and trims, and of lift deletes. That span is a time, not an edge (case 4). Anchoring it to a neighbour that another author rolled or ripple-moved would move the put-back off its span (`CLIP_OVERLAP`, or a refill at another time). A ripple restore (the undo of a ripple delete or move) is an insertion, so it takes anchors (D9).

A position's anchor holds both candidates, so that D2's fallback can be encoded: `Anchor { start: Option<Name>, end: Option<Name>, empty: bool }`. At a cut, both are set.

An item our op itself moves is a valid anchor. Positions are in the frame before the op (§4.3), so "move M to its own end" is `End(M)`. Ids are named canonically, as in `places`: an id that an earlier op of the transaction bound is named by its placeholder, so a planning id never stands for another author's item at the head.

*Why edges only.* An edge is what "before B" and "after A" mean, and it is all the keyed oracle models. A gap or interior position names a time, not an item. Shifts keep it in step with the ripples on its track, which is what it already does. Anchoring it to the nearest edge would invent an intent.

*Why `Empty`.* Seeds `1c600fdf` and `3130272a` are moves to a track that was empty where they were written. At the head that track is empty again, after the head inserted onto it and then lifted the item away with an overwrite move (which records no shift). Shifts carry the position past the departed item.

### D2. Resolution at the evolving head

This happens right before the op re-runs, in `run.project()`: the head with our earlier rebased ops applied.

- **`Start(B)`**: B's start, when B is on T at the head.
- **`End(A)`**: A's end, when A is on T at the head. From there the resolution walks forward over each item N that starts exactly at the current point and that the head *inserted there* since F (D3). After each one passed, the point is N's end.
- **`Empty`**: p itself, when T is still empty at the head. Everything the head put on T since has gone again, so a fresh edit writes the same p.
- **Fallback.** When the anchor is gone, the other neighbour is tried: a cut has both `Start(B)` and `End(A)`, and B is tried first. When neither usable anchor is left, the position maps through shifts exactly as today, with the same `REMOVED` rule for a position strictly inside a removed range. An anchor is gone when it was removed, or moved to another track (D4).

### D3. What the `End` walk passes

The walk stands in for `Assoc::After`: content inserted at our position goes before us. It passes exactly what the keyed oracle's `Assoc::After` walk passes (`Realiser::anchor`), computed in fin-core from the unmirrored steps' ops and diffs. An item N at the point is passed when it is not ours (no op of our tx created it) and:
- a committed ripple `item.move` put it somewhere (its `start` changed with that op's shift on its track). It arrived by an insertion, as a shift would;
- or it was created since (an `Added` change of a committed op: an insert of any mode, the right part of a committed split, a restore). The exception is an item that a committed op has since *relocated* without a shift: moved or slid, or had an edge or its start changed with no shift on its track by that op (a roll or overwrite trim, a carve). The head fitted such an item against its neighbours rather than inserting it at our point, so it is not passed;
- or it is the right part that our own earlier split cut from an item this walk passed. This never applies to a part cut from the anchor itself: the oracle passes a part only after it has passed another item.

Mirrored pairs are left out, as for conflicts: an undo with its unit, a redo with its undo, a copy with its inverted original. An undone record has no effect, so it says nothing about who put an item where.

**The oracle changes to match** (review finding 1). Today its `ripple_moved` and `relocated` sets range over every committed record, including undone units and their undos. They will range only over the records still in effect, neither undone nor an undo. This is a model fix in the direction of intent, not a carve-out: a fresh author at the head never sees an undone record.

Each step's own facts are worked out once in `Prepared`. The engine combines the unmirrored ones in `rebase_prepared`, as `unmirrored_footprints` does, because a merge sets `mirror_of` per candidate after `Prepared::new`. An inverted step (a source record standing in a merge as its own inverse) swaps `Added` with `Removed`, and its moves count as relocations, not ripple placements.

*Why mirror the oracle exactly.* The oracle is the model of intent the properties check. Its walk is the ProseMirror `Assoc::After` convention restated over items, and it is the only definition both sides can be held to. Departing from it in fin-core would turn into a new carve-out.

### D4. Cross-track anchors

An anchor that the head moved to another track is gone for this position: the fallback of D2 applies. The op keeps the track it names. Following the anchor onto its new track would change which track an edit lands on: that is a different edit, not the same edit at a different time. A conflict would add refusals where a fresh edit applies. The oracle has no fresh realisation here (its anchor is on another track), so neither choice is checked. The fallback is the old behaviour.

### D5. Overwritten or deleted anchors

- **Deleted** (ripple, lift, or removed by an overwrite): the anchor is gone, so D2's fallback applies.
- **Overwritten in part**: the anchor's edge resolves where it is now. Example: a committed overwrite insert carved A's tail. `End(A)` is A's new end, and the walk passes the overwriting item, which was created since and not relocated. A carve never splits an item (`OVERWRITE_SPLITS_ITEM`); `compose::overwrite` splits first. So the right remainder is a split's right part (created), whose start the carve then moves without a shift (relocated): it is not passed. `Start(B)` with B's head carved is B's new start, after the overwriting item.

### D6. Linked partners

No special rule is needed. A partner is an item like any other. When our later op is anchored on a partner, it resolves to wherever the rebased earlier op (or the head's link changes) put that partner at the head. The named item's `at` is still the only stored position, and the partners follow from it at the head, as today. This is what removes *partner anchor*, *partner drift*, *group grew* and *group changed*.

### D7. Blame for refusals

The blame rule is **unchanged**: a re-run that fails is blamed on the one unmirrored step that changed an entity the op changes, would create, or the error names. Anchors are not added to that set.

*Why.* Under anchor mapping, a relocated anchor is followed, so the unblamed refusals of the family no longer arise. Any that remain are findings to fix, not records to point at. Adding the anchor's toucher to the blame set would also widen what the oracle accepts (any blamed policy refusal is allowed), which is a carve-out in disguise. And an op that is blamed today could lose its blame when a second record touched its anchor.

### D8. Stored form, mapping order and own mirrors

- Only the position fields are rewritten at the head. Stored ops, inverses, diffs and the wire format are unchanged.
- The `Mapping` (inverses of our earlier ops, the steps, our rebased ops, with mirrors) is still built and pushed in the same order. Positions without a usable anchor use it, and `map_content` (content offsets) is untouched.
- Anchors that are our own earlier inserts, splits or moves resolve through the head applier's bindings (`run.bound()`). This replaces the Decision 16 concern about the mapping order of our own earlier ops.

### D9. User-visible changes (for the owner)

All of these are "edits land where you meant":
- The family's stale edits land at the anchor's edge, where they previously landed N frames off.
- The family's unblamed policy refusals (`INSERT_INSIDE_CLIP`, `MOVE_INTO_ITSELF`, `OVERWRITE_SPLITS_ITEM`, `NOT_ADJACENT`, `SLIDE_BLOCKED`, `NOT_FOUND`) go away where a fresh edit applies.
- **Undo, redo, merge, cherry-pick and revert follow the same rule.** An undo of a ripple delete puts the item back *before the item that followed it*, even when another author moved that item along the same track since. Previously it went back at the shifted time: after the preceding item, because the follower's removal collapsed the position. This is the timeline counterpart of `places`, where an undo of a removal goes back between the neighbours it had. It is called out here because a user could notice it. If goldens or history properties show it in a case where it reads as wrong, the implementer stops and reports to the owner.

## Affected paths

Every path through `rebase_prepared` takes anchors. Each caller reads anchors in the frame it already reads, or will read, for `places`:

| Path | Caller | Frame |
|---|---|---|
| stale commit | `rebase::engine::rebase_tx_env` (and its `failed_at_base` prefix) | `base` |
| undo, redo | `history::undo::try_rebase_over` | `state_at(tx.branch, tx.base_rev)` |
| `undo --to` | `history::undo::rebase_steps`, `history::checkpoint` | same |
| merge | `history::merge` via `history::source_places` | `state_at(src.branch, src.seq − 1)` |
| cherry-pick, revert | `history::pick` via `source_places` | same |

`source_places` becomes `source_frame` and returns places and anchors together. It reads the frame only when some op names an index or a position. Cost: one more frame read per merge or pick candidate that has positional ops. The merge performance tests (4B Task 20) check that this stays affordable.

**Re-checking goldens and histories.** Run every fin-core and fin-store test, including:
- `ops_*` goldens (`golden_apply`: apply only, expected to be unchanged);
- `rebase`, `rebase_moves`, `rebase_modes`, `merge`, `pick`, `undo`, `conflict_rules`, `restore_conflicts`;
- `props_history*`, `undo_to_props`, `pick_props`, `rebase_props`, `props_places`;
- fin-store `fixture_4b`, `chain_props`, `props_replay`;
- the finisherd RPC fixtures (`pnpm test`, `pnpm e2e`).

Every changed expectation is inspected by hand. It is updated only when the new value is where the edit was meant to land, and the commit states why for each one. A change that is not explainable that way is a bug, or a D9 case to report.

## Files

- Create `crates/fin-core/src/rebase/anchors.rs` (≤ 600 lines): `Anchor`, `anchors(frame, tx, mode)`, `any(ops)`, `Anchor::resolve(..)`.
- Create `crates/fin-core/src/rebase/placed.rs`: per-step placement facts (D3) for `Prepared`, if `steps.rs` would pass 600 lines.
- Modify `crates/fin-core/src/rebase/{mod.rs, engine.rs, steps.rs}`, `crates/fin-core/src/history/{mod.rs, undo.rs, checkpoint.rs, merge.rs, pick.rs}`, and `crates/fin-core/tests/rebase.rs` (struct literals).
- Modify the oracle in `crates/fin-core/tests/keyed4c/`, splitting `mod.rs` (1452 lines) so that each file is ≤ 1500 lines: the carve-outs and refusal rule go to `limits.rs`, and the intent types and realiser stay in `mod.rs`.
- Docs: SP4 design §4.3 (the mapping rules, as built, and the limitations list), §5.11, and arch spec §27 if it names shift-only mapping.

## Tasks

### Task 1: anchors derived in the frame

- [ ] Write failing unit tests (`crates/fin-core/tests/anchors.rs`) for `rebase::anchors::anchors`:
  - an insert at a cut gives `Start(B)`;
  - at the track end, `End(A)`;
  - on an empty track, `Empty`;
  - in a gap or inside an item, none;
  - a move to its own end gives `End(M)`;
  - an anchor that an earlier op of the tx created is named by its placeholder;
  - a restore in an inverse gives the follower's `Start`;
  - ops without positions give empty lists.
- [ ] Implement `anchors.rs`: run an `Applier` over the frame (as `places::places` does) and, per op and per position, read the anchor before the op runs.
- [ ] Pass: `cargo test -p fin-core --test anchors`, then clippy.

### Task 2: placement facts per step

- [ ] Write failing tests for each D3 class of an item: a ripple insert, a ripple move, an overwrite move, a slide, a roll or overwrite trim of a created item, a split's right part, an inverted step, and a mirrored pair left out.
- [ ] Implement each step's own `Placed` in `Prepared` (computed once, shared like the footprints): the `ripple_moved`, `created` and `relocated` id sets. The engine combines the unmirrored ones (D3).
- [ ] Oracle: `ripple_moved`, `relocated` and `lifted_tracks` range only over the records in effect (D3). Add fixed cases for the review's two scenarios:
  - a committed insert after `c1`, whose later overwrite trim is undone, and then ours inserts after `c1`;
  - a ripple move of `X` that is undone, then a ripple move of `A` before `X`, and then ours inserts after `A`.
- [ ] Pass the unit tests and `rebase` / `merge`. The engine is unchanged so far.

### Task 3: resolution in the engine, wired through every path

- [ ] Write failing tests: the seven seed tests (already written, failing) and the fixed cases `trim_of_concurrent_insert_leaves_gap`, `slide_after_lifted_end_refuses_unblamed` and `unlink_then_trim_and_move_at_former_partner`, checked by a strict variant of `check` (no carve-outs) used by those tests only.
- [ ] Add `anchors: &[Vec<Option<Anchor>>]` to `Rebase` and `PreparedRebase`, with `with_anchors`. In `rebase_prepared`, a position with an anchor resolved at `run.project()` (D2, D3) takes the resolved value, and everything else maps as today.
- [ ] Wire every caller (table above). `source_places` becomes `source_frame`, returning places and anchors.
- [ ] Unit tests for D2's fallback:
  - B removed → `End(A)`;
  - both gone → shifts, and `REMOVED` strictly inside a removed range;
  - an anchor moved to another track (D4) → shifts on the op's own track;
  - a lift restore → shifts.
- [ ] Fixed cases for D9 in `tests/undo.rs` and `tests/merge.rs`:
  - the undo of a ripple delete after the follower was ripple-moved along its track (the item goes back before the follower);
  - the undo of a ripple move, in the same situation;
  - the undo of an overwrite move after a neighbour was rolled (a lift restore, so it goes back to its old span).
- [ ] Keep `engine.rs` ≤ 600 lines: resolution lives in `anchors.rs`.
- [ ] Pass: the seed tests and fixed cases under the strict check, then all fin-core tests.
- [ ] Measure the merge cost of the extra frame read (the merge performance tests, 4B Task 20) before and after.

### Task 4: remove the carve-outs, each with its fixed case

For each carve-out:
- [ ] remove it;
- [ ] run its fixed case(s) and the two properties at 20k cases;
- [ ] when they pass, the removal stays; when they fail, investigate (core bug, or a true limit to keep and document).

The carve-outs: `Anchor::Zero` exclusion, relocated-anchor exclusion, `self_relocates` / `self_drift`, `group_grew`, `group_changed`, `partner_anchor`, `partner_drift`, `lifted_on`, `concurrent_reedged`, `anchor_drift`, `lifted_gap`. `index_in_changed_list` belongs to `places`, which has its own oracle (`props_places`), and stays.

- [ ] Split `keyed4c/mod.rs` (≤ 1500 lines), and update the `keyed4c` module docs to list what is left and why.
- [ ] Turn the strict-check variant of Task 3 into the only `check`.
- [ ] Add the seven cc lines to the regression files.

### Task 5: goldens and history

- [ ] Write a parity test: for random histories, fin-core's combined placement facts equal the oracle's `ripple_moved` and `relocated` over the records in effect.
- [ ] Run the whole fin-core and fin-store suites, then `FIN_REQUIRE_SERVER=1 cargo test --workspace`, `pnpm test`, `pnpm gates` and `pnpm e2e`.
- [ ] Inspect each changed expectation (rules above), and update it with a stated reason or fix the core.

### Task 6: spec "as built"

- [ ] SP4 §4.3:
  - rewrite the mapping rule (anchors first, shifts as the fallback, D1–D8);
  - replace the limitations list and the design note with an as-built note (what was removed, what remains and why);
  - move the *Partner drift* follow-up in §5.11 to done.
- [ ] Arch spec: check §6.5 and §27 for "shift-only" wording.

### Task 7: final validation

- [ ] `rebase_moves_props` and `props_rebase_4c` at 100k cases × 5 runs each (`PROPTEST_CASES=100000`), with distinct seeds.
- [ ] Every fin-core property suite at 2000 cases, and fin-store `chain_props` and `props_replay`.
- [ ] Clippy `-D warnings` and fmt, then the full checks of Task 5.

## Review (fresh subagent, 2026-11-09) and what changed

1. **Blocker:** the oracle counts undone records in `ripple_moved` and `relocated`, and the plan did not. D3 now aligns the oracle with the records in effect, with two fixed cases (Task 2).
2. Placement facts are now combined per candidate in the engine, not in `Prepared` (Task 2).
3. Lift restores in inverses would have followed re-edged neighbours. They now map through shifts only (D1, rule 5).
4. D9 has no oracle: the undo model accepts any order once the successor has moved. Explicit fixed cases are added (Task 3).
5. D5 named a carve remainder that does not exist. Reworded.
6. The walk passes our split part only after another item. D3 is reworded, with a unit test.
7. All callers and struct literals are covered. An anchor needs both candidates, hence the pair type (D1).
8. Once finding 1 is fixed, no carve-out beyond `index_in_changed_list` has to stay.
9. A hand simulation of six seeds and seven fixed cases agrees under the rules.
10. Missing steps (the parity test, fallback tests, the line budget and the merge cost) are added to Tasks 3 and 5.

## Revision after the implementation review (2026-11-09)

The second review ran `props_history` and found a merge that reversed the source's own order. Running the history suites at scale then gave a second case in `props_history_4c`. Both trace to D3, so the design is revised:

- **D1 gains `track_end`.** No item starts at the position, and it is the end of the track's last item, so the position is an append. At the head it resolves to the end of the track's last item, whatever the head appended, moved or trimmed there since. When the track is empty there, the position maps through shifts. This is what both test models mean by "at the end": the keyed oracle's `before: None`, and the undo model's reinsertion with no successor.
- **D3 (the `End` walk and `Placed`) is withdrawn.**
  - Every end anchor the oracles produce is a track end, which `track_end` resolves without a walk.
  - The walk's mirrored-pair rule could not be made right either way. An undo's ripple restore, or a merge's copy after its inverted source, can put an item back at a different place than it took it from. Leaving the pairs out missed that; counting them changed intent.
  - An end anchor followed by a gap now resolves to that item's end. A concurrent insert at exactly that point is a tie with no intent signal, and sticking to the item is the simpler answer.
  - The parity property goes with the walk. The oracle's walk, its base set, `ripple_moved`, `relocated` and `in_effect` are removed, and `After(last)` is realised as the track end.
- **Removed carve-outs:** unchanged.
- **New named cases:** `merge_keeps_the_source_order_past_an_earlier_copy` and `merged_move_to_the_end_goes_to_the_targets_end` (`tests/rebase_anchors.rs`), plus the two seeds in the history regression files.
