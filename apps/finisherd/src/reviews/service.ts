// The reviews service (5E): the store, the hub that publishes what it writes and the policy
// gate, built once per server and shared by every transport's handlers and chains.
import type { ChainReviews } from "../chain/resolve.ts";
import type { ControlStore } from "../control/db.ts";
import { Context } from "effect";
import { PolicyGate } from "./gate.ts";
import { ReviewHub } from "./hub.ts";
import { ReviewStore } from "./store.ts";

export interface Reviews {
  readonly store: ReviewStore;
  readonly hub: ReviewHub;
  /** The per-project policy gate the chain and `review.policy` share (Decision 18). */
  readonly gate: PolicyGate;
  /** What the chain reads: an item's project and branches, and a project's protected branch ids. */
  readonly chain: ChainReviews;
}

export class ReviewsTag extends Context.Service<ReviewsTag, Reviews>()("finisherd/Reviews") {}

export function makeReviews(control: ControlStore, clock?: () => number): Reviews {
  const hub = new ReviewHub();
  const store = new ReviewStore(control, clock, (item) => hub.publish(item));
  return { store, hub, gate: new PolicyGate(), chain: { item: (id) => store.get(id), protectedBranches: (projectId) => store.policy(projectId).protected } };
}
