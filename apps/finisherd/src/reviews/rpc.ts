// The `review.*` handlers (5E plan Decision 23). The chain has checked scopes, reach and the item
// (`review` resolver: the payload carries the item's `projectId`, `source` and `target`); a
// project another principal reaches is `PROJECT_NOT_FOUND` before this. `review.decide|merge|
// close` are writes: their records carry the chain's guard (`writeGuardOf`) and, for a
// credential, the author the chain stamped; the chain holds the project's policy gate until they
// answer.
import type { FinError, ReviewKind, ReviewState, Snapshot } from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf, writeGuardOf } from "../chain/chain.ts";
import type { ChainProjects } from "../chain/resolve.ts";
import type { Audit } from "../control/audit.ts";
import { actorFor } from "../diagnostics.ts";
import { mergeReview } from "./merge.ts";
import { policyHandler } from "./policy.ts";
import type { Reviews } from "./service.ts";
import { type Caller, callerOf, type ReviewClose, type ReviewCommentReq, type ReviewDecide, type ReviewRequest } from "./workflow.ts";

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;
type Options = { readonly headers?: unknown };

export function reviewHandlers({
  guard,
  reviews,
  projects,
  audit,
  transport,
  roles,
}: {
  readonly guard: Guard;
  readonly reviews: Reviews;
  readonly projects: Pick<ChainProjects, "resolveBranch">;
  readonly audit: Audit;
  readonly transport: string;
  /** The loaded roles' names, for `review.policy`'s `autoMerge.roles` (7B Task 12). */
  readonly roles?: () => ReadonlySet<string>;
}) {
  const policy = policyHandler({ reviews, projects, audit, transport, ...(roles === undefined ? {} : { roles }) });
  const w = reviews.workflow;
  /** The caller of a read: its principal and actor (a call that did not pass the chain is a defect). */
  const reader = (options: Options): Effect.Effect<Caller> =>
    Effect.suspend(() => {
      const c = chainOf(options);
      if (c === undefined) return Effect.die(new Error("a review call reached its handler without the chain"));
      return Effect.succeed(callerOf(c.principal, actorFor(c.principal, options.headers), transport));
    });
  /** The caller of a write: with the chain's guard and stamped author. */
  const writer = (options: Options, payload: object): Effect.Effect<Caller> =>
    Effect.flatMap(writeGuardOf(options), (g) => {
      const c = chainOf(options)!;
      return Effect.succeed(callerOf(c.principal, { ...actorFor(c.principal, options.headers), guard: g }, transport, (payload as { readonly author?: unknown }).author));
    });
  return {
    "review.list": ({ projectId, state, kind }: { projectId: string; state?: ReviewState | undefined; kind?: ReviewKind | undefined }) =>
      guard("review.list", Effect.sync(() => ({ items: reviews.store.list(projectId, { state, kind }) }))),
    "review.watch": ({ projectId }: { projectId: string }) => reviews.hub.watch(projectId, () => reviews.store.list(projectId)),
    "review.policy.get": ({ projectId }: { projectId: string }) => guard("review.policy.get", Effect.sync(() => ({ policy: reviews.store.policy(projectId) }))),
    "review.request": (payload: ReviewRequest, options: Options) => guard("review.request", Effect.flatMap(reader(options), (c) => w.request(c, payload))),
    "review.comment": (payload: ReviewCommentReq, options: Options) => guard("review.comment", Effect.flatMap(reader(options), (c) => w.comment(c, payload))),
    "review.refresh": ({ itemId }: { itemId: string }, options: Options) => guard("review.refresh", Effect.flatMap(reader(options), (c) => w.refresh(c, itemId))),
    "review.get": ({ itemId }: { itemId: string }) => guard("review.get", w.get(itemId)),
    "review.decide": (payload: ReviewDecide, options: Options) => guard("review.decide", Effect.flatMap(writer(options, payload), (c) => w.decide(c, payload))),
    "review.merge": (payload: { itemId: string; expect: Snapshot; acceptPending?: boolean | undefined; idemKey: string }, options: Options) =>
      guard("review.merge", Effect.flatMap(writer(options, payload), (c) => mergeReview(w, c, payload))),
    "review.close": (payload: ReviewClose, options: Options) => guard("review.close", Effect.flatMap(writer(options, payload), (c) => w.close(c, payload))),
    "review.policy": (payload: Parameters<typeof policy>[0], options: object) => guard("review.policy", policy(payload, options)),
  };
}
