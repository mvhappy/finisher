// Re-derived reviews (5E plan Decision 22, spec §8.1 and §3.1's damage rule): items live in this
// server's `control.db`, so a project opened on a server that never saw its branches (a move,
// a copy, a reset `control.db`) has agent branches with changes and no review. When a project is
// loaded, every active branch an `agent:` or `mcp:` credential made, that still has changes to
// merge into its fork parent and has no open `branchReview` into that parent and none a person dismissed or rejected, gets one marked
// `rederived: true`. Comments and accept marks of the old server's items are lost; rejections
// are not (they are `drop` records, so a dropped transaction is no longer a change to merge).
// Idempotent; at most `REDERIVE_MAX` per load, the rest on the next one. Never fails the load.
import { REVIEW_OPEN_STATES, REVIEW_TITLE_MAX } from "@finisher/contracts";
import { Effect } from "effect";
import { isOpenEntry, snapshotOf } from "./plan.ts";
import type { ReviewWorkflow } from "./workflow.ts";

/** Items one load may make. */
export const REDERIVE_MAX = 64;

/** Who the system's own writes are. */
const SYSTEM = "system:finisherd";

const CREDENTIAL_AUTHOR = /^(agent|mcp):/;

/** Re-derives the project's reviews; answers the ids of the items it opened. Never fails. */
export function rederive(w: ReviewWorkflow, projectId: string): Effect.Effect<ReadonlyArray<string>> {
  const task = Effect.gen(function* () {
    const made: string[] = [];
    const branches = yield* w.engine.branches(projectId);
    // Skipped: a branch with an open review into its parent, or one a person dismissed or rejected.
    // A `merged` (or superseded) review does not count: changes left after a partial merge are reviewed again.
    const reviews = w.store.list(projectId, { kind: "branchReview" });
    const seen = (b: { readonly id: string; readonly parent?: string | null }) =>
      reviews.some((i) => i.source === b.id && i.target === b.parent && (REVIEW_OPEN_STATES.includes(i.state) || i.state === "dismissed" || i.state === "rejected"));
    for (const b of branches) {
      if (made.length >= REDERIVE_MAX) break;
      if (b.state !== "active" || b.parent == null || !CREDENTIAL_AUTHOR.test(b.createdBy) || seen(b)) continue;
      yield* Effect.ignore(
        Effect.gen(function* () {
          const plan = yield* w.planner.now(projectId, b.id, b.parent!);
          if (!plan.entries.some((e) => e.branch === b.id && isOpenEntry(e))) return;
          const snapshot = snapshotOf(plan, w.clock());
          const opened = w.store.open({
            projectId,
            kind: "branchReview",
            source: b.id,
            target: b.parent!,
            snapshot,
            title: [...`Review of ${b.name}`].slice(0, REVIEW_TITLE_MAX).join(""),
            requestedBy: { principal: SYSTEM },
            rederived: true,
          });
          w.planner.remember(opened.id, snapshot, plan);
          made.push(opened.id);
        }),
      );
    }
    return made as ReadonlyArray<string>;
  });
  return task.pipe(Effect.catch(() => Effect.succeed([] as ReadonlyArray<string>)), Effect.catchDefect(() => Effect.succeed([] as ReadonlyArray<string>)));
}
