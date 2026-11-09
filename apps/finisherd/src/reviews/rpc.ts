// The `review.*` handlers (5E plan Decision 23). Task 5 serves the reads that need only the store
// and the hub (`review.list`, `review.watch`, `review.policy.get`); the workflow (request,
// comment, refresh, get, decide, merge, close) and the policy write answer `INTERNAL {reason:
// notYet}` until Tasks 6 and 7 replace them. The chain has checked scopes, reach and the item
// (`review` resolver); a project another principal reaches is `PROJECT_NOT_FOUND` before this.
import type { FinError, ReviewKind, ReviewState } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";
import type { Reviews } from "./service.ts";

type Guard = <A, R>(tag: string, self: Effect.Effect<A, FinError, R>) => Effect.Effect<A, FinError, R>;

const notYet = (tag: string): Effect.Effect<never, FinError> =>
  Effect.fail(finError("INTERNAL", `${tag} is not implemented yet`, { reason: "notYet", tag }));

export function reviewHandlers({ guard, reviews }: { readonly guard: Guard; readonly reviews: Reviews }) {
  const pending = (tag: string) => () => guard(tag, notYet(tag));
  return {
    "review.list": ({ projectId, state, kind }: { projectId: string; state?: ReviewState | undefined; kind?: ReviewKind | undefined }) =>
      guard("review.list", Effect.sync(() => ({ items: reviews.store.list(projectId, { state, kind }) }))),
    "review.watch": ({ projectId }: { projectId: string }) => reviews.hub.watch(projectId, () => reviews.store.list(projectId)),
    "review.policy.get": ({ projectId }: { projectId: string }) => guard("review.policy.get", Effect.sync(() => ({ policy: reviews.store.policy(projectId) }))),
    "review.request": pending("review.request"),
    "review.comment": pending("review.comment"),
    "review.refresh": pending("review.refresh"),
    "review.get": pending("review.get"),
    "review.decide": pending("review.decide"),
    "review.merge": pending("review.merge"),
    "review.close": pending("review.close"),
    "review.policy": pending("review.policy"),
  };
}
