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

/** The drop records on the target's lineage, per origin (`dropIndex`). */
export interface DropIndex {
  /** The newest drop record naming `origin` since its newest undrop; none when it is not dropped. */
  newest(origin: string): CancelledDrop | undefined;
  /** Every drop record naming `origin` since its newest undrop, newest first. */
  since(origin: string): ReadonlyArray<CancelledDrop>;
}

export const NO_DROPS: DropIndex = { newest: () => undefined, since: () => [] };

/**
 * Reads the target's lineage once (`opsSince(target, 0)` answers every ancestor's records up to
 * each fork, then the target's own) and indexes its drop records per origin.
 */
export function dropIndex(w: ReviewWorkflow, item: ReviewItem): Effect.Effect<DropIndex, FinError> {
  return Effect.map(w.engine.records(item.projectId, item.target!), (records) => {
    const drops = records.filter((r) => r.link?.kind === "drop");
    const since = (origin: string): ReadonlyArray<CancelledDrop> => {
      const out: CancelledDrop[] = [];
      for (let i = drops.length - 1; i >= 0; i--) {
        const r = drops[i]!;
        const link = r.link as { drops?: ReadonlyArray<string>; undrops?: ReadonlyArray<string> };
        if (link.undrops?.includes(origin) === true) break;
        if (link.drops?.includes(origin) === true) out.push({ txId: origin, dropTx: r.tx.txId, droppedBy: r.tx.author });
      }
      return out;
    };
    return { since, newest: (origin) => since(origin)[0] };
  });
}

/** The txIds of the drop records the review itself wrote (`dropTxs` in its `decided` events). */
export function reviewDrops(w: ReviewWorkflow, itemId: string): ReadonlySet<string> {
  const out = new Set<string>();
  for (const e of w.store.events(itemId)) for (const tx of (e.details as { dropTxs?: ReadonlyArray<string> }).dropTxs ?? []) out.add(tx);
  return out;
}

/** The drops holding `origin` now that the review did not make, newest first. */
export function othersOf(index: DropIndex, own: ReadonlySet<string>, origin: string): ReadonlyArray<CancelledDrop> {
  return index.since(origin).filter((d) => !own.has(d.dropTx));
}

/**
 * Whether the drop that holds `origin` now is the review's alone: since the origin's newest
 * undrop, every drop record naming it is one the review wrote (round 4: a drop the review joined,
 * or one made after it, names its own author).
 */
export function isReviewsOwn(index: DropIndex, own: ReadonlySet<string>, origin: string): boolean {
  return index.newest(origin) !== undefined && othersOf(index, own, origin).length === 0;
}

/** Whether `c` holds `review.decide` on the item's target: the scope and the target in its branch reach. */
export function decidesOnTarget(c: Caller, item: ReviewItem): boolean {
  const g = c.meta.guard;
  const target = item.target!;
  // The auto-merge actor has no scopes: it never accepts, so it never cancels another's drop.
  const scopes = "scopes" in c.principal ? c.principal.scopes : [];
  return scopes.includes("review.decide") && g !== undefined && (g.allowBranches === undefined || g.allowBranches.includes(target)) && !(g.denyBranches ?? []).includes(target);
}

/** `REVIEW_STALE {reason: dropUnknown}`: the plan says dropped, yet no drop record names the origin. */
export const dropUnknown = (item: ReviewItem, origin: string): FinError =>
  finError("REVIEW_STALE", "no drop record holds a transaction the review lists as dropped", { itemId: item.id, reason: "dropUnknown", txId: origin }, "Refresh the review and decide again.");

/**
 * The drops of `origins` (dropped now, not the review's alone) that accepting them cancels: every
 * drop record since the origin's newest undrop that the review did not write, newest first, each
 * its own entry. Refused with `CONFLICT {reason: droppedByOther}` (naming the newest of them)
 * unless the caller decides on the target; `REVIEW_STALE {reason: dropUnknown}` when none is found.
 */
export function othersDrops(c: Caller, item: ReviewItem, index: DropIndex, own: ReadonlySet<string>, origins: ReadonlyArray<string>): Effect.Effect<ReadonlyArray<CancelledDrop>, FinError> {
  return Effect.map(
    Effect.forEach(origins, (origin) => {
      const others = othersOf(index, own, origin);
      const newest = others[0];
      if (newest === undefined) return Effect.fail(dropUnknown(item, origin));
      if (decidesOnTarget(c, item)) return Effect.succeed(others);
      return Effect.fail(
        finError(
          "CONFLICT",
          "the transaction was dropped by someone else, and only a reviewer of the target branch may bring it back",
          { itemId: item.id, reason: "droppedByOther", txId: origin, droppedBy: newest.droppedBy, dropTx: newest.dropTx },
          "Ask a reviewer of the target branch to accept it, or leave it pending.",
        ),
      );
    }),
    (lists) => lists.flat(),
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
