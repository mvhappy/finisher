// The reviews service (5E): the store and the hub that publishes what it writes, built once per
// server and shared by every transport's handlers.
import type { ChainReviews } from "../chain/resolve.ts";
import type { ControlStore } from "../control/db.ts";
import { Context } from "effect";
import { ReviewHub } from "./hub.ts";
import { ReviewStore } from "./store.ts";

export interface Reviews {
  readonly store: ReviewStore;
  readonly hub: ReviewHub;
  /** What the chain reads: an item's project and branches, and a project's protected branch ids. */
  readonly chain: ChainReviews;
}

export class ReviewsTag extends Context.Service<ReviewsTag, Reviews>()("finisherd/Reviews") {}

export function makeReviews(control: ControlStore, clock?: () => number): Reviews {
  const hub = new ReviewHub();
  const store = new ReviewStore(control, clock, (item) => hub.publish(item));
  return { store, hub, chain: { item: (id) => store.get(id), protectedBranches: (projectId) => store.policy(projectId).protected } };
}
