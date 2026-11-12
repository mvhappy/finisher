// The branch review workflow (5E plan Decisions 9, 12–15, 20; spec §8.2): request, refresh, get,
// comment, decide and close; the merge is `merge.ts`. A person decides each origin transaction
// of the change list planned at the item's snapshot. A rejection is a `drop` record on the
// target, written before the item changes, so it outlives the item, a move and a rebuild.
//
// Every write goes through the project registry with the chain's guard (`Caller.meta`), so
// protected branches and branch limits hold inside the store's transaction too. The chain holds
// the project's policy gate shared for `review.decide|merge|close` until they answer; nothing
// here takes it again. Review text never goes into an error, an audit row or the journal
// (`reason` alone, training only: `verdicts.ts`).
import type { FinError, Principal, ReviewItem, ReviewView, Snapshot } from "@finisher/contracts";
import { REVIEW_OPEN_STATES } from "@finisher/contracts";
import type { CommittedTx, ReviewPlan } from "@finisher/core-types";
import { Effect } from "effect";
import { authorOf } from "../chain/policy.ts";
import type { Audit } from "../control/audit.ts";
import { ulid } from "../control/ulid.ts";
import { type CallMeta, type PolicyActor, policyVerdict } from "../diagnostics.ts";
import { finError, toFinError } from "../errors.ts";
import { dropPayload } from "../history.ts";
import { isOpenEntry, Planner, type ReviewEngine, sameSnapshot, snapshotOf } from "./plan.ts";
import type { DecisionInput, ReviewStore } from "./store.ts";
import { NO_VERDICTS, type ReviewVerdictSink, type ReviewVerdictValue, verdictBy, verdictEntries } from "./verdicts.ts";

/** fin-core's `MAX_DROP_IDS`: origins one `drop` record names, drops and undrops together. */
export const DROP_RECORD_MAX = 4096;

/** The principal of the server-internal auto-merge actor (`auto-merge.ts`): never a connection's. */
export interface PolicyPrincipal {
  readonly kind: "policy";
  readonly id: "autoMerge";
}

/** Who calls: the principal, the call's actor with the chain's guard, the author its records carry. */
export interface Caller {
  readonly principal: Principal | PolicyPrincipal;
  readonly meta: CallMeta;
  /** The chain's stamp for a credential, else `<kind>:<id>`. */
  readonly author: string;
  /** The transport, or `internal` for the policy actor (never audited as a transport). */
  readonly transport: string;
  /** Set only for the auto-merge policy's actor (Decision 19): its verdicts say `by: policy`, `via: autoMerge`. */
  readonly internal?: PolicyActor | undefined;
}

export interface WorkflowDeps {
  readonly store: ReviewStore;
  readonly engine: ReviewEngine;
  readonly audit?: Pick<Audit, "append"> | undefined;
  readonly verdicts?: ReviewVerdictSink | undefined;
  readonly clock?: (() => number) | undefined;
}

export interface ReviewRequest {
  readonly projectId: string;
  readonly source: string;
  readonly into?: string | undefined;
  readonly title: string;
  readonly summary?: string | undefined;
}

export interface ReviewDecide {
  readonly itemId: string;
  readonly expect: Snapshot;
  readonly decisions?: ReadonlyArray<DecisionInput> | undefined;
  readonly requestChanges?: { readonly reason?: string | undefined } | undefined;
}

export interface ReviewClose {
  readonly itemId: string;
  readonly state: "rejected" | "dismissed";
}

export interface ReviewCommentReq {
  readonly itemId: string;
  readonly body: string;
  readonly at?: string | undefined;
  readonly txId?: string | undefined;
}

/** A merge's outcome as `observeMerge` hears it (Task 8): the report, or the error that refused it. */
export type MergeOutcome = { readonly report: unknown } | { readonly error: FinError };

export const principalKey = (p: { readonly kind: string; readonly id: string }): string => `${p.kind}:${p.id}`;

/** A caller of `principal` (a read: no guard); `author` defaults to `<kind>:<id>`. */
export function callerOf(principal: Principal, meta: CallMeta, transport: string, author?: unknown): Caller {
  return { principal, meta, transport, author: typeof author === "string" && author.length > 0 ? author : authorOf(principal) };
}

export const reviewNotFound = (itemId: string): FinError =>
  finError("REVIEW_NOT_FOUND", "no such review item", { itemId }, "Check the item's id with a list of the project's reviews.");

export const wrongKind = (item: ReviewItem): FinError =>
  finError("INVALID_REQUEST", `a ${item.kind} item is not decided or merged this way`, { field: "itemId", reason: "kind", kind: item.kind }, "Close the item instead.");

export const reviewClosed = (item: ReviewItem): FinError =>
  finError("REVIEW_CLOSED", `the review item is ${item.state}`, { itemId: item.id, state: item.state }, "Open a new review for the branch to decide more of it.");

/** Store calls throw `FinError`s: as an Effect. */
export const sync = <A>(f: () => A): Effect.Effect<A, FinError> => Effect.try({ try: f, catch: toFinError });

const isOpen = (item: ReviewItem) => REVIEW_OPEN_STATES.includes(item.state);

/** The distinct origins of the plan's entries that are still open (not on the target, not dropped). */
export function openOrigins(plan: ReviewPlan): ReadonlyArray<string> {
  return [...new Set(plan.entries.filter(isOpenEntry).map((e) => e.origin))];
}

export class ReviewWorkflow {
  readonly store: ReviewStore;
  readonly planner: Planner;
  readonly audit: Pick<Audit, "append"> | undefined;
  readonly verdicts: ReviewVerdictSink;
  readonly clock: () => number;

  constructor(deps: WorkflowDeps) {
    this.store = deps.store;
    this.planner = new Planner(deps.engine);
    this.audit = deps.audit;
    this.verdicts = deps.verdicts ?? NO_VERDICTS;
    this.clock = deps.clock ?? Date.now;
  }

  get engine(): ReviewEngine {
    return this.planner.engine;
  }

  /** The item, or `REVIEW_NOT_FOUND`. */
  item(itemId: string): Effect.Effect<ReviewItem, FinError> {
    return Effect.suspend(() => {
      const item = this.store.get(itemId);
      return item === undefined ? Effect.fail(reviewNotFound(itemId)) : Effect.succeed(item);
    });
  }

  /** An open `branchReview`: other kinds are `INVALID_REQUEST {reason: kind}`, closed ones `REVIEW_CLOSED`. */
  openBranchReview(itemId: string): Effect.Effect<ReviewItem, FinError> {
    return Effect.flatMap(this.item(itemId), (item) => {
      if (item.kind !== "branchReview") return Effect.fail(wrongKind(item));
      return isOpen(item) ? Effect.succeed(item) : Effect.fail(reviewClosed(item));
    });
  }

  /** `expect` must be the item's snapshot (seqs and hashes): `REVIEW_STALE` with the item's change list otherwise. */
  checkExpect(item: ReviewItem, expect: Snapshot): Effect.Effect<void, FinError> {
    if (item.snapshot !== undefined && sameSnapshot(item.snapshot, expect)) return Effect.void;
    return Effect.flatMap(this.view(item), (view) =>
      Effect.fail(
        finError(
          "REVIEW_STALE",
          "the review changed since it was read",
          { itemId: item.id, snapshot: item.snapshot ?? null, stale: view.stale, plan: view.plan ?? null },
          "Read the review again and decide on what it lists now.",
        ),
      ),
    );
  }

  /** The item with its change list at its snapshot, `stale`, its decisions and comments. */
  view(item: ReviewItem): Effect.Effect<ReviewView, FinError> {
    const rest = { decisions: [...this.store.decisions(item.id).values()], comments: this.store.comments(item.id) };
    if (item.kind !== "branchReview" || item.snapshot === undefined) return Effect.succeed({ item, stale: false, ...rest });
    return Effect.map(this.planner.atSnapshot(item), ({ plan, stale }) => ({ item, stale, plan, ...rest }));
  }

  get(itemId: string): Effect.Effect<ReviewView, FinError> {
    return Effect.flatMap(this.item(itemId), (item) => this.view(item));
  }

  /** Whether `c` may hand a `changesRequested` item back: the source's author, or who asked for the review. */
  #reopens(c: Caller, item: ReviewItem): Effect.Effect<boolean, FinError> {
    if (item.state !== "changesRequested") return Effect.succeed(false);
    if (item.requestedBy.principal === principalKey(c.principal)) return Effect.succeed(true);
    const p = c.principal;
    if (p.kind === "policy") return Effect.succeed(false);
    return Effect.map(this.engine.branches(item.projectId), (bs) => bs.find((b) => b.id === item.source)?.createdBy === authorOf(p));
  }

  /** Decision 12: plans the change list and opens the item; the open item of the pair answers `existing: true`. */
  request(c: Caller, req: ReviewRequest): Effect.Effect<{ item: ReviewItem; existing: boolean }, FinError> {
    const self = this;
    return Effect.gen(function* () {
      const source = yield* self.engine.resolveBranch(req.projectId, req.source);
      const target = yield* self.engine.resolveBranch(req.projectId, req.into ?? "main");
      if (source === target) {
        return yield* Effect.fail(finError("INVALID_REQUEST", "a branch cannot be reviewed into itself", { field: "into", reason: "sameBranch" }, "Name another target branch."));
      }
      const branch = (yield* self.engine.branches(req.projectId)).find((b) => b.id === source);
      if (branch?.state === "archived") {
        return yield* Effect.fail(finError("INVALID_REQUEST", "the source branch is archived", { field: "source", reason: "archived", branch: source }, "Restore the branch first."));
      }
      const existing = self.store.findOpen(req.projectId, "branchReview", source, target);
      if (existing !== undefined) {
        if (!(yield* self.#reopens(c, existing))) return { item: existing, existing: true };
        return { item: yield* self.#replan(c, existing, true), existing: true };
      }
      const plan = yield* self.planner.now(req.projectId, source, target);
      const snapshot = snapshotOf(plan, self.clock());
      const item = yield* sync(() =>
        self.store.open({
          projectId: req.projectId,
          kind: "branchReview",
          source,
          target,
          snapshot,
          title: req.title,
          ...(req.summary === undefined ? {} : { summary: req.summary }),
          requestedBy: { principal: principalKey(c.principal), actor: c.meta.actor },
        }),
      );
      self.planner.remember(item.id, snapshot, plan);
      return { item, existing: false };
    });
  }

  /** A new snapshot of both heads; `reopen` hands a `changesRequested` item back (`open`). */
  #replan(c: Caller, item: ReviewItem, reopen: boolean): Effect.Effect<ReviewItem, FinError> {
    return Effect.flatMap(this.planner.now(item.projectId, item.source!, item.target!), (plan) => {
      const snapshot = snapshotOf(plan, this.clock());
      this.planner.remember(item.id, snapshot, plan);
      const by = principalKey(c.principal);
      return sync(() => (reopen ? this.store.setState(item.id, "open", by, { snapshot }) : this.store.setSnapshot(item.id, snapshot, by)));
    });
  }

  /** Re-plans at both heads now; decisions are keyed by origin and survive it. */
  refresh(c: Caller, itemId: string): Effect.Effect<ReviewView, FinError> {
    const self = this;
    return Effect.gen(function* () {
      const item = yield* self.openBranchReview(itemId);
      const updated = yield* self.#replan(c, item, yield* self.#reopens(c, item));
      return yield* self.view(updated);
    });
  }

  comment(c: Caller, req: ReviewCommentReq) {
    return Effect.flatMap(this.item(req.itemId), () =>
      Effect.map(
        sync(() =>
          this.store.comment({
            itemId: req.itemId,
            principal: principalKey(c.principal),
            actor: c.meta.actor,
            body: req.body,
            ...(req.at === undefined ? {} : { at: req.at }),
            ...(req.txId === undefined ? {} : { txId: req.txId }),
          }),
        ),
        (comment) => ({ comment }),
      ),
    );
  }

  /**
   * `drop` records on the item's target naming `drops` and `undrops` (at most `DROP_RECORD_MAX`
   * ids each), each audited `history.drop {item, count}`; the records written.
   */
  writeDrops(c: Caller, item: ReviewItem, drops: ReadonlyArray<string>, undrops: ReadonlyArray<string>): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> {
    const ids = [...drops.map((id) => ({ id, drop: true })), ...undrops.map((id) => ({ id, drop: false }))];
    const chunks: Array<typeof ids> = [];
    for (let i = 0; i < ids.length; i += DROP_RECORD_MAX) chunks.push(ids.slice(i, i + DROP_RECORD_MAX));
    return Effect.forEach(chunks, (chunk) => {
      const d = chunk.filter((x) => x.drop).map((x) => x.id);
      const u = chunk.filter((x) => !x.drop).map((x) => x.id);
      const request: { projectId: string; txId: string; author: string; from: string; into: string; drops?: string[]; undrops?: string[]; argsHash?: string } = {
        projectId: item.projectId,
        txId: `review-${ulid(this.clock())}`,
        author: c.author,
        from: item.source!,
        into: item.target!,
        ...(d.length === 0 ? {} : { drops: d }),
        ...(u.length === 0 ? {} : { undrops: u }),
      };
      const payload = dropPayload(request);
      return Effect.tap(Effect.map(this.engine.write(item.projectId, "drop", payload, c.meta), (r) => r as CommittedTx), () =>
        Effect.sync(() =>
          this.audit?.append({
            kind: "history.drop",
            principal: { kind: c.principal.kind, id: c.principal.id },
            outcome: "ok",
            details: { projectId: item.projectId, item: item.id, count: chunk.length, transport: c.transport },
          }),
        ),
      );
    });
  }

  /** Verdict entries for `item` (Decision 20); never fails. */
  emit(c: Caller, item: ReviewItem, verdict: ReviewVerdictValue, txIds: ReadonlyArray<string>, counts: "accepted" | "rejected" | "none", reason?: string): void {
    const p = c.principal;
    const session = c.internal !== undefined || p.kind === "agent" || p.kind === "mcp" ? c.meta.actor.session : undefined;
    const who = c.internal !== undefined ? policyVerdict(c.internal) : { by: verdictBy(p) };
    const entries = verdictEntries({ item, verdict, ...who, txIds, counts, sessionId: session, reason, now: this.clock() });
    this.verdicts.emit(item.projectId, c.meta.actor, entries);
  }

  /**
   * Decision 13: records decisions on origins of the snapshot's change list. Newly rejected
   * origins are dropped, and rejected ones moved back undropped, in `drop` records written
   * before the decisions are; `requestChanges` hands the branch back (`changesRequested`).
   */
  decide(c: Caller, req: ReviewDecide): Effect.Effect<ReviewView, FinError> {
    const self = this;
    return Effect.gen(function* () {
      let item = yield* self.openBranchReview(req.itemId);
      yield* self.checkExpect(item, req.expect);
      const { plan } = yield* self.planner.atSnapshot(item);
      const decidable = new Set(plan.entries.filter((e) => e.status !== "alreadyPresent").map((e) => e.origin));
      const wanted = new Map<string, DecisionInput>();
      for (const d of req.decisions ?? []) {
        if (!decidable.has(d.txId)) {
          return yield* Effect.fail(
            finError("INVALID_REQUEST", "a decision names a transaction the review does not list", { field: "decisions", reason: "notInSnapshot", txId: d.txId }, "Decide only on the transactions the review lists."),
          );
        }
        wanted.delete(d.txId);
        wanted.set(d.txId, d);
      }
      const current = self.store.decisions(item.id);
      const all = [...wanted.values()];
      // Drops as they are now, not as the snapshot's list says (review of Task 7, B1 and S1): a
      // rejection drops what is not dropped; an accept undrops what is (whoever dropped it); a
      // pending takes back only the review's own rejection; nothing undrops what is not dropped.
      const status = all.length === 0 ? new Map<string, string>() : yield* self.planner.statusNow(item);
      const dropped = (id: string) => status.get(id) === "dropped";
      const drops = all.filter((d) => d.decision === "reject" && status.get(d.txId) === "open").map((d) => d.txId);
      const undrops = all.filter((d) => dropped(d.txId) && (d.decision === "accept" || (d.decision === "pending" && current.get(d.txId)?.decision === "reject"))).map((d) => d.txId);
      const written = yield* self.writeDrops(c, item, drops, undrops);
      const by = principalKey(c.principal);
      if (all.length > 0) yield* sync(() => self.store.setDecisions(item.id, all, by));
      if (req.requestChanges !== undefined) item = yield* sync(() => self.store.setState(item.id, "changesRequested", by));
      if (written.length > 0) item = yield* self.#afterOwnDrops(c, item, written);
      const batch = (v: "accept" | "reject") => all.filter((d) => d.decision === v);
      const reasonOf = (ds: ReadonlyArray<DecisionInput>) => {
        const reasons = new Set(ds.map((d) => d.reason).filter((r) => r !== undefined));
        return reasons.size === 1 ? [...reasons][0] : undefined;
      };
      const accepted = batch("accept");
      const rejected = batch("reject");
      if (accepted.length > 0) self.emit(c, item, "accepted", accepted.map((d) => d.txId), "accepted", reasonOf(accepted));
      if (rejected.length > 0) self.emit(c, item, "rejected", rejected.map((d) => d.txId), "rejected", reasonOf(rejected));
      if (req.requestChanges !== undefined) self.emit(c, item, "changesRequested", [], "none", req.requestChanges.reason);
      return yield* self.view(self.store.get(item.id) ?? item);
    });
  }

  /**
   * After the item's own drop records: when they are the only records on the target since the
   * snapshot (they follow its head directly) and the source has not moved, the snapshot moves
   * to the target's new head, so a decision does not make its own review stale.
   */
  #afterOwnDrops(c: Caller, item: ReviewItem, written: ReadonlyArray<CommittedTx>): Effect.Effect<ReviewItem, FinError> {
    const snap = item.snapshot!;
    const direct = written.every((w, i) => w.rev === snap.targetSeq + 1 + i);
    if (!direct) return Effect.succeed(item);
    return Effect.flatMap(this.planner.now(item.projectId, item.source!, item.target!), (plan) => {
      const moved = snapshotOf(plan, this.clock());
      const own = moved.sourceSeq === snap.sourceSeq && (moved.sourceHash ?? null) === (snap.sourceHash ?? null) && moved.targetSeq === written[written.length - 1]!.rev;
      if (!own) return Effect.succeed(item);
      this.planner.remember(item.id, moved, plan);
      return sync(() => this.store.setSnapshot(item.id, moved, principalKey(c.principal), { reason: "ownDrop" }));
    });
  }

  /**
   * Decision 15: `rejected` drops every transaction the item would still merge into the target
   * (accepted ones too: an accepted mark on a rejected item would let a later plain merge bring
   * it in); `dismissed` writes nothing.
   */
  close(c: Caller, req: ReviewClose): Effect.Effect<{ item: ReviewItem }, FinError> {
    const self = this;
    return Effect.gen(function* () {
      const item = yield* self.item(req.itemId);
      if (!isOpen(item)) return yield* Effect.fail(reviewClosed(item));
      const by = principalKey(c.principal);
      if (item.kind !== "branchReview" || req.state === "dismissed") {
        const closed = yield* sync(() => self.store.setState(item.id, req.state, by));
        if (item.kind === "branchReview") self.emit(c, closed, "dismissed", [], "none");
        return { item: closed };
      }
      // What a merge of the item could still copy: the plan now, up to the snapshot's source seq.
      const plan = yield* self.planner.now(item.projectId, item.source!, item.target!);
      const upTo = item.snapshot?.sourceSeq ?? plan.source.seq;
      const origins = openOrigins({ ...plan, entries: plan.entries.filter((e) => e.branch !== plan.source.branch || e.seq <= upTo) });
      yield* self.writeDrops(c, item, origins, []);
      const closed = yield* sync(() => self.store.setState(item.id, "rejected", by));
      self.emit(c, closed, "rejected", origins, "rejected");
      return { item: closed };
    });
  }

  /** Task 8's seam: a merge's outcome for `mergeConflict` items. Nothing yet. */
  observeMerge(_projectId: string, _from: string, _into: string, _outcome: MergeOutcome): void {}
}
