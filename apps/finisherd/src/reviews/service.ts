// The reviews service (5E): the store, the hub that publishes what it writes, the policy gate
// and the workflow, built once per server and shared by every transport's handlers and chains.
import type { ChainReviews } from "../chain/resolve.ts";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import { Context, Effect } from "effect";
import { finError } from "../errors.ts";
import { PolicyGate } from "./gate.ts";
import { ReviewHub } from "./hub.ts";
import type { ReviewEngine } from "./plan.ts";
import { ReviewStore } from "./store.ts";
import type { ReviewVerdictSink } from "./verdicts.ts";
import { ReviewWorkflow } from "./workflow.ts";

export interface Reviews {
  readonly store: ReviewStore;
  readonly hub: ReviewHub;
  /** The per-project policy gate the chain and `review.policy` share (Decision 18). */
  readonly gate: PolicyGate;
  /** What the chain reads: an item's project and branches, and a project's protected branch ids. */
  readonly chain: ChainReviews;
  /** Request, refresh, get, comment, decide, merge (`merge.ts`) and close (Task 7). */
  readonly workflow: ReviewWorkflow;
}

export class ReviewsTag extends Context.Service<ReviewsTag, Reviews>()("finisherd/Reviews") {}

export interface ReviewsOptions {
  readonly clock?: () => number;
  /** The project registry's side (`registryEngine`); without it the workflow answers `INTERNAL`. */
  readonly engine?: ReviewEngine;
  readonly audit?: Pick<Audit, "append">;
  /** Where `review.verdict` entries go (`journalReviewVerdicts`); none by default. */
  readonly verdicts?: ReviewVerdictSink;
}

const noEngine = (): Effect.Effect<never, ReturnType<typeof finError>> => Effect.fail(finError("INTERNAL", "the reviews service has no project registry", { reason: "noEngine" }));
const NO_ENGINE: ReviewEngine = { resolveBranch: noEngine, branches: noEngine, plan: noEngine, write: noEngine };

export function makeReviews(control: ControlStore, options: ReviewsOptions = {}): Reviews {
  const hub = new ReviewHub();
  const store = new ReviewStore(control, options.clock, (item) => hub.publish(item));
  const workflow = new ReviewWorkflow({ store, engine: options.engine ?? NO_ENGINE, audit: options.audit, verdicts: options.verdicts, clock: options.clock });
  return { store, hub, gate: new PolicyGate(), chain: { item: (id) => store.get(id), protectedBranches: (projectId) => store.policy(projectId).protected }, workflow };
}

export { registryEngine } from "./plan.ts";
export { journalReviewVerdicts } from "./verdicts.ts";
