// What a rollback writes on one branch (plan 7B Decision 23, Task 8): planned from the session's
// later-run records there before anything is written, with 4B's undo and redo only (no new op).
//
// The session's author is the session's alone, so its undo and redo stacks on the branch are
// what its records made them (fin-core `history/undo.rs`, `derive_stacks`): from `afterSeq` (the
// seq before its first later-run record there) the later runs pushed and popped them. The end of
// run `toRun` is the stacks' state at `afterSeq`; getting back there takes two things, in order:
//
// 1. **Redo** the later runs' undo records that took back what was there at `afterSeq` (a unit
//    made before it, or recreated by a redo of an undo made before it, as `plan_undo_to` counts
//    "made"), top of the redo stack first, each a plain 4B redo. One that a later edit cleared
//    off the redo stack cannot be redone, one that took back units on both sides of `afterSeq`
//    cannot be redone alone, and one below a redo-stack entry that must stay is out of reach:
//    each is `CONFLICT {reason: notReversible}` before anything is written.
// 2. **Undo** every unit of the author's made after `afterSeq` in one record (`undo` with
//    `afterSeq`, `plan_undo_to` as `undo --to` takes a checkpoint): the later runs' edits, picks
//    and merges still there, and their redos of undos made after `afterSeq` (a redo of an undo
//    made before it recreates what was not there: it is taken too). Their undos and redos of one
//    another cancel out (`NOTHING_TO_UNDO`: nothing is written on the branch).
//
// After step 1 the redo units sit on top of the undo stack and count as made before `afterSeq`:
// step 2's walk would stop at them, but nothing made after `afterSeq` is below them (with only
// top-of-stack undos and redos, a later edit clears the redo stack, so an undo record that is
// still to be redone was made with nothing of the later runs left on the undo stack).
import type { CommittedTx } from "@finisher/core-types";
import { finError } from "../errors.ts";

/** A later-run record of the session's, as `tx.get` answers it (`link` absent: an edit). */
export type LaterRec = Pick<CommittedTx, "rev" | "link"> & { readonly txId: string };

/** One branch's writes: the undo records to redo (top first), then one undo after `afterSeq`. */
export interface BranchPlan {
  readonly afterSeq: number;
  readonly redo: readonly string[];
}

/** When a unit was made, against `afterSeq` (`plan_undo_to`'s `Made`). */
type Made = "before" | "after" | "both";

const combine = (xs: readonly Made[]): Made => (xs.length === 0 ? "before" : xs.every((x) => x === xs[0]) ? xs[0]! : "both");

/** The plan of one branch from its later-run records (any order); `CONFLICT` when it cannot be exact. */
export function planBranch(branch: string, records: readonly LaterRec[]): BranchPlan {
  const recs = [...records].sort((a, b) => a.rev - b.rev);
  const afterSeq = (recs[0]?.rev ?? 1) - 1;
  const made = new Map<string, Made>();
  const undoes = new Map<string, readonly string[]>();
  const madeOf = (unit: string): Made => made.get(unit) ?? "before";
  // The redo stack's entries the later runs pushed (an earlier one's are below, out of sight).
  let redo: string[] = [];
  const cleared = new Map<string, string>();
  let group: string | undefined;
  for (const r of recs) {
    const link = r.link ?? { kind: "edit" as const };
    if (link.kind === "merge" && link.mergeId === group) continue;
    group = link.kind === "merge" ? link.mergeId : undefined;
    switch (link.kind) {
      case "edit":
      case "cherryPick":
      case "revert":
      case "merge":
        made.set(link.kind === "merge" ? link.mergeId : r.txId, "after");
        for (const u of redo) cleared.set(u, r.txId);
        redo = [];
        break;
      case "undo":
        undoes.set(r.txId, link.undoes);
        redo.push(r.txId);
        break;
      case "redo": {
        const of = undoes.get(link.redoes);
        // A redo of a later-run undo recreates its units (made when they were); of an earlier one, what was not there.
        made.set(r.txId, of === undefined ? "after" : combine(of.map(madeOf)));
        const i = redo.lastIndexOf(link.redoes);
        if (i >= 0) redo.splice(i, 1);
        break;
      }
      default:
        break;
    }
  }
  const needs = (u: string) => combine((undoes.get(u) ?? []).map(madeOf));
  const refuse = (tx: string, why: string, message: string) =>
    finError("CONFLICT", `the rollback cannot restore branch ${branch} exactly: ${message}`, { reason: "notReversible", why, branch, tx, afterSeq }, "undo or redo on that branch by hand (fin undo, fin redo), then roll back again");
  for (const [u, by] of cleared) {
    if (needs(u) !== "after") throw refuse(u, "redoCleared", `the later edit ${by} cleared the undo ${u} off the redo stack, and what it took back was there at the end of the run`);
  }
  const top: string[] = [];
  let stop = false;
  for (const u of [...redo].reverse()) {
    const n = needs(u);
    if (n === "both") throw refuse(u, "straddles", `the undo ${u} took back units from both before and after the run's end`);
    if (n === "after") stop = true;
    else if (stop) throw refuse(u, "buried", `the undo ${u} is below a later one the redo stack must keep`);
    else top.push(u);
  }
  return { afterSeq, redo: top };
}
