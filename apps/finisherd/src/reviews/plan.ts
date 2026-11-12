// A branch review's change list (5E plan Decisions 7 and 9): fin-store's `ReviewPlan`, derived at
// the item's snapshot of both heads and never stored. It is cached in memory per `(item,
// snapshot)` (the newest `PLAN_CACHE_MAX`) and planned again on a miss.
//
// A plan made now matches the snapshot when both heads have the same seq and hash; otherwise the
// item is stale. A stale item's list is the cached one, or, after a miss, the plan made now
// without the source records above the snapshot's source seq: what the item's merge could still
// copy (`sourceUpTo`), which is what a decision may name.
import type { FinError, ReviewItem, Snapshot } from "@finisher/contracts";
import type { BranchView, CommittedTx, EntryStatus, ReviewPlan } from "@finisher/core-types";
import { Effect } from "effect";
import type { CallMeta } from "../diagnostics.ts";
import type { HistoryOp, HistoryPayload } from "../history.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import { attemptJson, finError } from "../errors.ts";

/** Change lists kept in memory (Decision 9). */
export const PLAN_CACHE_MAX = 64;

/** What the workflow needs of the project registry: reads, the plan, and the three writes it makes. */
export interface ReviewEngine {
  readonly resolveBranch: (projectId: string, ref: string) => Effect.Effect<string, FinError>;
  /** Every branch, archived ones too. */
  readonly branches: (projectId: string) => Effect.Effect<ReadonlyArray<BranchView>, FinError>;
  readonly plan: (projectId: string, from: string, into: string) => Effect.Effect<ReviewPlan, FinError>;
  /** The records of a branch's whole lineage (`opsSince(branch, 0)`: each ancestor's up to its fork, then its own), oldest first; `undrop.ts` reads who dropped an origin. */
  readonly records: (projectId: string, branch: string) => Effect.Effect<ReadonlyArray<CommittedTx>, FinError>;
  /** A write through the registry's one path (the commit lock, the hubs, the guard in `meta`). */
  readonly write: (projectId: string, op: Extract<HistoryOp, "merge" | "drop" | "branchSet">, payload: HistoryPayload, meta: CallMeta) => Effect.Effect<unknown, FinError>;
}

export function registryEngine(registry: Pick<ProjectRegistryShape, "resolveBranch" | "branches" | "history" | "ensure" | "engineOf">): ReviewEngine {
  return {
    records: (projectId, branch) =>
      Effect.andThen(registry.ensure(projectId), () => {
        const engine = registry.engineOf(projectId);
        if (engine === undefined) return Effect.fail(finError("PROJECT_NOT_FOUND", `project ${projectId} is not open`, { projectId }));
        return attemptJson<ReadonlyArray<CommittedTx>>(() => engine.opsSince(branch, 0));
      }),
    resolveBranch: registry.resolveBranch,
    branches: (projectId) => Effect.map(registry.branches(projectId, true), (r) => r.branches),
    plan: (projectId, from, into) => Effect.map(registry.history(projectId, "reviewPlan", { projectId, from, into }), (r) => r as ReviewPlan),
    write: registry.history,
  };
}

/** Statuses of entries a merge would still have to decide about (not present, not dropped). */
const UNDECIDABLE: ReadonlySet<EntryStatus> = new Set<EntryStatus>(["alreadyPresent", "dropped"]);

/** Whether an entry is still open: neither on the target already nor dropped for it. */
export const isOpenEntry = (e: { readonly status: EntryStatus }): boolean => !UNDECIDABLE.has(e.status);

/** The snapshot a plan was made at. */
export function snapshotOf(plan: ReviewPlan, plannedAt: number): Snapshot {
  return {
    sourceSeq: plan.source.seq,
    ...(plan.source.hash == null ? {} : { sourceHash: plan.source.hash }),
    targetSeq: plan.target.seq,
    ...(plan.target.hash == null ? {} : { targetHash: plan.target.hash }),
    plannedAt,
  };
}

/** Same heads: seqs and hashes (`plannedAt` ignored), so a rewind to the same seq is caught. */
export function sameSnapshot(a: Snapshot, b: Snapshot): boolean {
  return a.sourceSeq === b.sourceSeq && (a.sourceHash ?? null) === (b.sourceHash ?? null) && a.targetSeq === b.targetSeq && (a.targetHash ?? null) === (b.targetHash ?? null);
}

/** The plan without the source branch's records above `sourceSeq` (the merge's `sourceUpTo`). */
export function restrict(plan: ReviewPlan, sourceSeq: number): ReviewPlan {
  const source = plan.source.branch;
  return { ...plan, entries: plan.entries.filter((e) => e.branch !== source || e.seq <= sourceSeq) };
}

const keyOf = (s: Snapshot) => `${s.sourceSeq}:${s.sourceHash ?? ""}:${s.targetSeq}:${s.targetHash ?? ""}`;

/** The newest `max` change lists by `(item, snapshot)`. */
export class PlanCache {
  readonly #max: number;
  readonly #plans = new Map<string, ReviewPlan>();

  constructor(max = PLAN_CACHE_MAX) {
    this.#max = max;
  }

  get(itemId: string, s: Snapshot): ReviewPlan | undefined {
    const k = `${itemId}|${keyOf(s)}`;
    const plan = this.#plans.get(k);
    if (plan !== undefined) {
      this.#plans.delete(k);
      this.#plans.set(k, plan);
    }
    return plan;
  }

  set(itemId: string, s: Snapshot, plan: ReviewPlan): void {
    const k = `${itemId}|${keyOf(s)}`;
    this.#plans.delete(k);
    this.#plans.set(k, plan);
    while (this.#plans.size > this.#max) this.#plans.delete(this.#plans.keys().next().value as string);
  }

  get size(): number {
    return this.#plans.size;
  }
}

/** Plans for review items: fresh ones, and the change list at an item's snapshot. */
export class Planner {
  readonly engine: ReviewEngine;
  readonly cache: PlanCache;
  /** Plans made (tests: a cache hit makes none). */
  planned = 0;

  constructor(engine: ReviewEngine, cache = new PlanCache()) {
    this.engine = engine;
    this.cache = cache;
  }

  /** A plan of `source` into `target` at their heads now. */
  now(projectId: string, source: string, target: string): Effect.Effect<ReviewPlan, FinError> {
    return Effect.suspend(() => {
      this.planned++;
      return this.engine.plan(projectId, source, target);
    });
  }

  /** Keeps `plan` as the change list of `item` at `snapshot`. */
  remember(itemId: string, snapshot: Snapshot, plan: ReviewPlan): void {
    this.cache.set(itemId, snapshot, plan);
  }

  /**
   * The change list at the item's snapshot and whether either head moved since. A cache hit
   * compares the heads' seqs with the branch list (no plan; there is no cheap read of a head's
   * hash, so a rewind to the same seq is seen only on a miss); a miss plans now and compares seqs
   * and hashes. `approximate`: a miss whose source moved, so the list is the plan now cut at the
   * snapshot's source seq and may hold records the snapshot never had (a rewind re-resolved at
   * or below that seq).
   */
  atSnapshot(item: ReviewItem): Effect.Effect<{ readonly plan: ReviewPlan; readonly stale: boolean; readonly approximate: boolean }, FinError> {
    const { snapshot, source, target } = item;
    if (snapshot === undefined || source === undefined || target === undefined) {
      return Effect.die(new Error(`review item ${item.id} has no snapshot to plan at`));
    }
    const cached = this.cache.get(item.id, snapshot);
    if (cached !== undefined) {
      return Effect.map(this.engine.branches(item.projectId), (branches) => {
        const head = (id: string) => branches.find((b) => b.id === id)?.head;
        return { plan: cached, stale: head(source) !== snapshot.sourceSeq || head(target) !== snapshot.targetSeq, approximate: false };
      });
    }
    return Effect.map(this.now(item.projectId, source, target), (plan) => {
      const now = snapshotOf(plan, snapshot.plannedAt);
      if (sameSnapshot(now, snapshot)) {
        this.remember(item.id, snapshot, plan);
        return { plan, stale: false, approximate: false };
      }
      const sourceMoved = now.sourceSeq !== snapshot.sourceSeq || (now.sourceHash ?? null) !== (snapshot.sourceHash ?? null);
      return { plan: restrict(plan, snapshot.sourceSeq), stale: true, approximate: sourceMoved };
    });
  }

  /**
   * Where each origin of the item's change list stands now (a fresh plan cut at the snapshot's
   * source seq): `open` (a merge would still copy it), `present` or `dropped`. An origin with
   * one open entry is open. Decisions and merges read drops from here, never from the cached
   * snapshot list, which a later drop or undrop does not change.
   */
  statusNow(item: ReviewItem): Effect.Effect<ReadonlyMap<string, "open" | "present" | "dropped">, FinError> {
    return Effect.flatMap(this.now(item.projectId, item.source!, item.target!), (plan) => {
      // A cut list cannot say where the origins past it stand: never guess them not open.
      if (plan.truncated) {
        return Effect.fail(finError("REVIEW_STALE", "the branch has more transactions than one review plans", { itemId: item.id, reason: "truncated" }, "Merge or review the branch in smaller parts."));
      }
      const out = new Map<string, "open" | "present" | "dropped">();
      for (const e of restrict(plan, item.snapshot!.sourceSeq).entries) {
        const s = e.status === "alreadyPresent" ? "present" : e.status === "dropped" ? "dropped" : "open";
        if (out.get(e.origin) !== "open") out.set(e.origin, s);
      }
      return Effect.succeed(out);
    });
  }
}
