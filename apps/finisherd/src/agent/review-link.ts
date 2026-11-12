// Agent sessions and 5E's review workflow (plan 7B Task 11, Decisions 25 and 26; spec §5.4,
// §6.2, §9.1):
//
// - **A review request from a session** (`review.request` through the chain, the session's own
//   credential, `review.write` in `draft` and `auto`): after the handler answered, an item whose
//   source lies in the session's subtree is recorded on the session (`session_state.reviews`,
//   agent 0011) and `branchReady {sessionId, projectId, branch, headSeq, reviewId}` is emitted,
//   `headSeq` the source's head as the request left it. A request by anyone else, or for a branch
//   outside the session's subtree, links nothing.
// - **A session that ends** (any reason but `deleted`) with a root branch whose own records are
//   not all on its fork parent (a fresh review plan of the root into its parent has an open
//   entry of the root, as 5E's re-derivation decides) and no review recorded emits
//   `branchReady` without `reviewId`, once: `ended` is final. A session with no `session_txs`
//   row made no commit, and is not planned.
// - **`branchReady` is an in-process event**, never an RPC: `onBranchReady` subscribes (7F's
//   `BranchReady` hooks, Task 12's `auto` merge trigger); none by default. Listeners run after
//   the call that caused it has answered, each on its own; what they throw or fail with is
//   reported on stderr and never reaches that call.
// - **Critics** (Decision 26): a `critic` session started with `base` the source of an open
//   `branchReview` item gets its root as a child branch of it, `<reviewed name>.critic-<n>`
//   (`criticUnder`, the `under` of `openSession`), so its reach excludes the reviewed branch;
//   its `base.seq` is the reviewed head it started from.
// - **Archives** (5E residual): a branch the runtime archives (one made for a session that failed
//   to record) supersedes that branch's `mergeConflict` items (`archived`), as `branch.set` does.
import type { FinError, FinRpcTag, Principal, Session } from "@finisher/contracts";
import { REVIEW_OPEN_STATES } from "@finisher/contracts";
import type { BranchView, ReviewPlan } from "@finisher/core-types";
import { Effect } from "effect";
import type { ChainHooks } from "../chain/hooks.ts";
import type { ChainProjects } from "../chain/resolve.ts";
import { isOpenEntry } from "../reviews/plan.ts";
import type { Reviews } from "../reviews/service.ts";
import type { BranchLineage } from "./reach.ts";
import type { AgentStore } from "./store.ts";

/** A session's branch is ready for review (spec §9.1 `BranchReady`): internal, never on the wire. */
export interface BranchReady {
  readonly sessionId: string;
  readonly projectId: string;
  /** The branch's id: the request's source, or the session's root at its end. */
  readonly branch: string;
  /** That branch's head seq when it was found ready. */
  readonly headSeq: number;
  /** The `branchReview` item, when a review request made it ready. */
  readonly reviewId?: string;
}

/** A subscriber: its result (a promise or an Effect) is awaited, its failure reported. */
export type BranchReadyListener = (ev: BranchReady) => void | Promise<unknown> | Effect.Effect<unknown, unknown>;

/** What the link reads of 5E (bound once the reviews are built: `reviewPort`). */
export interface ReviewPort {
  resolveBranch(projectId: string, ref: string): Effect.Effect<string, FinError>;
  branches(projectId: string): Effect.Effect<ReadonlyArray<BranchView>, FinError>;
  /** Whether `branch` (an id) is the source of an open `branchReview` item. */
  underReview(projectId: string, branch: string): boolean;
  /** A fresh plan of `source` into `target`. */
  plan(projectId: string, source: string, target: string): Effect.Effect<ReviewPlan, FinError>;
  /** `branch` (an id) was archived by `by`: its conflict items are superseded. Never fails. */
  archived(projectId: string, branch: string, by?: string): Effect.Effect<void>;
}

/** The port over the server's reviews service. */
export function reviewPort(reviews: Reviews): ReviewPort {
  const w = reviews.workflow;
  return {
    resolveBranch: (projectId, ref) => w.engine.resolveBranch(projectId, ref),
    branches: (projectId) => w.engine.branches(projectId),
    underReview: (projectId, branch) =>
      reviews.store.list(projectId, { kind: "branchReview" }).some((i) => i.source === branch && REVIEW_OPEN_STATES.includes(i.state)),
    plan: (projectId, source, target) => w.planner.now(projectId, source, target),
    archived: (projectId, branch, by) => w.observeArchived(projectId, branch, by),
  };
}

export interface ReviewLinkDeps {
  readonly store: AgentStore;
  readonly lineage: BranchLineage;
  readonly warn?: (message: string) => void;
}

const warnStderr = (m: string) => void process.stderr.write(`finisherd: warning: ${m}\n`);

/** The reason of a session ended by `agent.session.delete`: abandoned, never ready. */
const DELETED = "deleted";

export class ReviewLink {
  readonly #d: ReviewLinkDeps;
  readonly #warn: (m: string) => void;
  readonly #listeners = new Set<BranchReadyListener>();
  readonly #pending = new Set<Promise<void>>();
  #port: ReviewPort | undefined;

  constructor(deps: ReviewLinkDeps) {
    this.#d = deps;
    this.#warn = deps.warn ?? warnStderr;
  }

  /** The reviews, once built (`main.ts`, after the agent runtime). */
  bind(port: ReviewPort): void {
    this.#port = port;
  }

  /** Subscribes to `branchReady`; answers the unsubscribe. */
  onBranchReady(listener: BranchReadyListener): () => void {
    this.#listeners.add(listener);
    return () => void this.#listeners.delete(listener);
  }

  /** Resolves once every emission so far, and the checks of every session end, have finished. */
  async settled(): Promise<void> {
    while (this.#pending.size > 0) await Promise.allSettled([...this.#pending]);
  }

  /**
   * `review.request` answered `result` for `principal`: when it is a session and the item's
   * source is in its subtree, the item is recorded on the session and `branchReady` emitted.
   */
  requested(principal: Principal, result: unknown, projects: ChainProjects): Effect.Effect<void> {
    const item = (result as { item?: { id?: unknown; projectId?: unknown; source?: unknown; kind?: unknown } } | null)?.item;
    const s = principal.kind === "agent" ? this.#d.store.get(principal.id) : undefined;
    if (s?.rootBranch === undefined || item?.kind !== "branchReview" || typeof item.id !== "string" || typeof item.source !== "string") return Effect.void;
    if (item.projectId !== s.projectId) return Effect.void;
    const { id: reviewId, source } = { id: item.id, source: item.source };
    const task = Effect.gen({ self: this }, function* () {
      const own = yield* this.#d.lineage.subtree(projects, s.projectId, s.rootBranch!, [source]);
      if (!own.has(source)) return;
      this.#d.store.reviews.record(s.id, reviewId);
      const port = this.#port;
      const head = port === undefined ? undefined : (yield* port.branches(s.projectId)).find((b) => b.id === source)?.head;
      if (head === undefined) return;
      this.#emit({ sessionId: s.id, projectId: s.projectId, branch: source, headSeq: head, reviewId });
    });
    return this.#quiet(task, `cannot link session ${s.id}'s review request`);
  }

  /** `s` has ended: ready when its root has unmerged records of its own and no review was asked. */
  sessionEnded(s: Session): void {
    if (s.rootBranch === undefined || s.statusReason === DELETED) return;
    const task = Effect.gen({ self: this }, function* () {
      const port = this.#port;
      if (port === undefined || this.#d.store.reviews.of(s.id).length > 0) return;
      if (this.#d.store.txsOfSession(s.id).length === 0) return;
      const root = (yield* port.branches(s.projectId)).find((b) => b.id === s.rootBranch);
      if (root === undefined || root.state !== "active" || root.parent == null) return;
      const plan = yield* port.plan(s.projectId, root.id, root.parent);
      if (!plan.entries.some((e) => e.branch === root.id && isOpenEntry(e))) return;
      this.#emit({ sessionId: s.id, projectId: s.projectId, branch: root.id, headSeq: root.head });
    });
    this.#track(Effect.runPromise(this.#quiet(task, `cannot check session ${s.id}'s branch at its end`)));
  }

  /**
   * The branch a `critic` session's root is made under (Decision 26): `base` (a reference) as
   * its id when it is the source of an open `branchReview`; else undefined (an ordinary root).
   */
  criticUnder(projectId: string, base: string): Effect.Effect<string | undefined> {
    const port = this.#port;
    if (port === undefined) return Effect.succeed(undefined);
    return Effect.orElseSucceed(
      Effect.map(port.resolveBranch(projectId, base), (id) => (port.underReview(projectId, id) ? id : undefined)),
      () => undefined,
    );
  }

  /** The runtime archived `branch` (an id): 5E's archive observer. Never fails. */
  archived(projectId: string, branch: string, by?: string): Effect.Effect<void> {
    return this.#port === undefined ? Effect.void : this.#port.archived(projectId, branch, by);
  }

  #quiet(task: Effect.Effect<void, unknown>, what: string): Effect.Effect<void> {
    return Effect.catchCause(task, (cause) => Effect.sync(() => this.#warn(`${what}: ${String(cause)}`)));
  }

  #track(p: Promise<void>): void {
    const tracked = p.catch(() => undefined).finally(() => this.#pending.delete(tracked));
    this.#pending.add(tracked);
  }

  /** Every listener, detached from the caller; one that fails is reported. */
  #emit(ev: BranchReady): void {
    for (const listener of this.#listeners) {
      this.#track(
        (async () => {
          try {
            const r = listener(ev);
            if (Effect.isEffect(r)) await Effect.runPromise(r as Effect.Effect<unknown, unknown>);
            else await r;
          } catch (e) {
            this.#warn(`a branchReady listener failed for session ${ev.sessionId}: ${String(e)}`);
          }
        })(),
      );
    }
  }
}

/** The chain's after stage for the link, after `hooks`' own (`review.request` only). */
export function withReviewLink(hooks: ChainHooks, link: ReviewLink, projects: ChainProjects): ChainHooks {
  const first = hooks.after;
  const after: ChainHooks["after"] = (tag: FinRpcTag, principal, payload, result, headers) =>
    Effect.andThen(first === undefined ? Effect.void : first(tag, principal, payload, result, headers), () =>
      tag === "review.request" ? link.requested(principal, result, projects) : Effect.void,
    );
  return { ...hooks, after };
}
