// An accept that cancels a drop another principal made (5E plan Decision 13, amendment by the
// owner, 2026-11-12). Accepting a dropped origin undrops it; when the review did not make that
// drop, the accepting principal must hold `review.decide` on the item's target: the scope, and
// the target within its branch reach (the chain's guard: in `allowBranches` when there is one,
// never in `denyBranches`). Both are checked here, not inferred from the chain having let the
// call through. Allowed: each cancelled drop is named in the answer and audited
// (`history.drop {item, txId, dropTx, droppedBy}`: ids only). Refused: `CONFLICT {itemId,
// reason: droppedByOther, txId, droppedBy, dropTx}`, before anything is written.
import type { FinError, ReviewItem } from "@finisher/contracts";
import type { CommittedTx } from "@finisher/core-types";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import type { Caller, ReviewWorkflow } from "./workflow.ts";

/** A drop an accept cancels: the origin, the drop record and its author. */
export interface CancelledDrop {
  readonly txId: string;
  readonly dropTx: string;
  readonly droppedBy: string;
}

/** The audit's form of an author: a credential's `<kind>:<id>`, else `other` (a free-text author is a name). */
const AUTHOR_WORD = /^(agent|mcp|device|plugin|local|console):[0-9A-Za-z]{1,64}$/;
/** The audit's form of a txId: a key of word characters, else `other`. */
const TX_WORD = /^[0-9A-Za-z._:-]{1,128}$/;

/**
 * The newest `drop` record on the target's lineage that names `origin` (its own records, then
 * each ancestor's up to the fork), when it drops it.
 */
function dropOf(w: ReviewWorkflow, item: ReviewItem, origin: string): Effect.Effect<CancelledDrop | undefined, FinError> {
  return Effect.gen(function* () {
    const branches = yield* w.engine.branches(item.projectId);
    let branch: string | undefined = item.target!;
    let upTo = Number.POSITIVE_INFINITY;
    while (branch !== undefined) {
      const records: ReadonlyArray<CommittedTx> = yield* w.engine.records(item.projectId, branch);
      for (let i = records.length - 1; i >= 0; i--) {
        const r = records[i]!;
        if (r.rev > upTo || r.link?.kind !== "drop") continue;
        if (r.link.drops?.includes(origin) === true) return { txId: origin, dropTx: r.tx.txId, droppedBy: r.tx.author };
        if (r.link.undrops?.includes(origin) === true) return undefined;
      }
      const view = branches.find((b) => b.id === branch);
      branch = view?.parent ?? undefined;
      upTo = view?.forkSeq ?? 0;
    }
    return undefined;
  });
}

/** Whether `c` holds `review.decide` on the item's target: the scope and the target in its branch reach. */
export function decidesOnTarget(c: Caller, item: ReviewItem): boolean {
  const g = c.meta.guard;
  const target = item.target!;
  return c.principal.scopes.includes("review.decide") && g !== undefined && (g.allowBranches === undefined || g.allowBranches.includes(target)) && !(g.denyBranches ?? []).includes(target);
}

/**
 * The drops of `origins` (dropped now, not by the review) that accepting them cancels; refused
 * with `CONFLICT {reason: droppedByOther}` unless the caller decides on the target.
 */
export function othersDrops(w: ReviewWorkflow, c: Caller, item: ReviewItem, origins: ReadonlyArray<string>): Effect.Effect<ReadonlyArray<CancelledDrop>, FinError> {
  return Effect.forEach(origins, (origin) =>
    Effect.flatMap(dropOf(w, item, origin), (found) => {
      const drop = found ?? { txId: origin, dropTx: "", droppedBy: "" };
      if (decidesOnTarget(c, item)) return Effect.succeed(drop);
      return Effect.fail(
        finError(
          "CONFLICT",
          "the transaction was dropped by someone else, and only a reviewer of the target branch may bring it back",
          { itemId: item.id, reason: "droppedByOther", txId: origin, droppedBy: drop.droppedBy, dropTx: drop.dropTx },
          "Ask a reviewer of the target branch to accept it, or leave it pending.",
        ),
      );
    }),
  );
}

/** One `history.drop` audit row per cancelled drop: who accepted (the principal), whose drop, which transaction. */
export function auditCancelled(w: ReviewWorkflow, c: Caller, item: ReviewItem, cancelled: ReadonlyArray<CancelledDrop>): void {
  const word = (v: string, re: RegExp) => (re.test(v) ? v : "other");
  for (const d of cancelled) {
    w.audit?.append({
      kind: "history.drop",
      principal: { kind: c.principal.kind, id: c.principal.id },
      outcome: "ok",
      details: { projectId: item.projectId, item: item.id, count: 1, txId: word(d.txId, TX_WORD), dropTx: word(d.dropTx, TX_WORD), droppedBy: word(d.droppedBy, AUTHOR_WORD), transport: c.transport },
    });
  }
}
