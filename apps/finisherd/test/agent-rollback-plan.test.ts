// A rollback's plan of one branch (plan 7B Task 8, `rollback-plan.ts`): which later-run undo
// records it redoes, and what it refuses before writing.
import { describe, expect, test } from "vitest";
import { type LaterRec, planBranch } from "../src/agent/rollback-plan.ts";

const edit = (txId: string, rev: number): LaterRec => ({ txId, rev, link: null });
const undo = (txId: string, rev: number, ...undoes: string[]): LaterRec => ({ txId, rev, link: { kind: "undo", undoes } });
const redo = (txId: string, rev: number, redoes: string): LaterRec => ({ txId, rev, link: { kind: "redo", redoes } });
const why = (recs: LaterRec[]) => {
  try {
    planBranch("b", recs);
    return undefined;
  } catch (e) {
    return (e as { details: { why: string } }).details.why;
  }
};

describe("planBranch", () => {
  test("only edits: no redo, one undo after the seq before the first", () => {
    expect(planBranch("b", [edit("B", 7), edit("A", 5)])).toEqual({ afterSeq: 4, redo: [] });
  });

  test("an undo of an earlier unit is redone; one of a later-run unit cancels out", () => {
    expect(planBranch("b", [undo("UX", 5, "X")])).toEqual({ afterSeq: 4, redo: ["UX"] });
    expect(planBranch("b", [edit("A", 5), undo("UA", 6, "A")])).toEqual({ afterSeq: 4, redo: [] });
    // A, its undo, then X's: only X's is redone (top first); A's stays on the redo stack.
    expect(planBranch("b", [edit("A", 5), undo("UA", 6, "A"), undo("UX", 7, "X"), undo("UY", 8, "Y")]).redo).toEqual(["UY", "UX"]);
  });

  test("a redo of a later-run undo of an earlier unit counts as made before: its undo is redone", () => {
    expect(planBranch("b", [undo("UY", 5, "Y"), redo("RY", 6, "UY"), undo("V", 7, "RY")]).redo).toEqual(["V"]);
    // A redo of an earlier undo recreates what was not there: undoing it is the later undo's.
    expect(planBranch("b", [redo("R", 5, "U0"), undo("V", 6, "R")]).redo).toEqual([]);
  });

  test("refused before any write: cleared off the redo stack, both sides, or buried", () => {
    expect(why([undo("UX", 5, "X"), edit("C", 6)])).toBe("redoCleared");
    expect(why([edit("A", 5), undo("U", 6, "A", "X")])).toBe("straddles");
    expect(why([edit("A", 5), undo("UX", 6, "X")])).toBeUndefined();
    expect(why([undo("UX", 5, "X"), edit("A", 6), undo("UA", 7, "A")])).toBe("redoCleared");
    expect(why([undo("UX", 5, "X"), redo("R", 6, "U0"), undo("V", 7, "R")])).toBe("buried");
  });
});
