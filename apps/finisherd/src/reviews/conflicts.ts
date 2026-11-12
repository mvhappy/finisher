// `mergeConflict` items (5E plan Decision 21, spec §8.5): `history.merge` hands every outcome to
// `ReviewWorkflow.observeMerge` (the handler's call-site lines, so it works whether or not 7A-2's
// chain hooks exist), and so does `review.merge`. A `CONFLICT` opens or updates the one open item
// of `(source, target)` with the conflict list in `subject`; a later successful merge of the
// pair, or archiving the source, makes it `superseded`. Rebase conflicts on `tx.commit` stay with
// their writer. Nothing here fails the merge it observes.
import type { FinError } from "@finisher/contracts";
import { REVIEW_TITLE_MAX } from "@finisher/contracts";
import { Effect } from "effect";
import type { MergeOutcome, ReviewWorkflow } from "./workflow.ts";

/** Conflicts kept in an item's `subject`. */
export const CONFLICTS_KEPT = 200;

/** Who the system's own writes are. */
const SYSTEM = "system:finisherd";

const titleOf = (from: string, into: string): string => [...`Merging ${from} into ${into} conflicts`].slice(0, REVIEW_TITLE_MAX).join("");

/** A merge's outcome: opens, updates or supersedes the pair's item. Never fails. */
export function observeMerge(w: ReviewWorkflow, projectId: string, from: string, into: string, outcome: MergeOutcome, by: string = SYSTEM): Effect.Effect<void> {
  const task = Effect.gen(function* () {
    const source = yield* w.engine.resolveBranch(projectId, from);
    const target = yield* w.engine.resolveBranch(projectId, into);
    const existing = w.store.findOpen(projectId, "mergeConflict", source, target);
    if ("report" in outcome) {
      if ((outcome.report as { dryRun?: unknown } | null)?.dryRun === true) return;
      if (existing !== undefined) w.store.setState(existing.id, "superseded", by);
      return;
    }
    if (outcome.error.code !== "CONFLICT") return;
    const list = (outcome.error.details as { conflicts?: unknown } | null)?.conflicts;
    const subject = { conflicts: Array.isArray(list) ? list.slice(0, CONFLICTS_KEPT) : [] };
    if (existing !== undefined) {
      w.store.setSubject(existing.id, subject);
      return;
    }
    const names = new Map((yield* w.engine.branches(projectId)).map((b) => [b.id, b.name]));
    w.store.open({
      projectId,
      kind: "mergeConflict",
      source,
      target,
      title: titleOf(names.get(source) ?? source, names.get(target) ?? target),
      requestedBy: { principal: by },
      subject,
    });
  });
  return Effect.ignore(task.pipe(Effect.catchDefect(() => Effect.void)));
}

/** The source of `ref` was archived: its open conflict items are superseded. Never fails. */
export function observeArchived(w: ReviewWorkflow, projectId: string, ref: string, by: string = SYSTEM): Effect.Effect<void> {
  const task = Effect.gen(function* () {
    const branch = (yield* w.engine.branches(projectId)).find((b) => b.id === ref || b.name === ref || b.display === ref);
    if (branch === undefined) return;
    for (const item of w.store.list(projectId, { kind: "mergeConflict" })) {
      if (item.source === branch.id && (item.state === "open" || item.state === "changesRequested")) w.store.setState(item.id, "superseded", by);
    }
  });
  return Effect.ignore(task.pipe(Effect.catchDefect(() => Effect.void)));
}

/** `history.merge` observed (a dry run wrote nothing and is not). */
export const observingMerge =
  (w: ReviewWorkflow, by: string, p: { readonly projectId: string; readonly from: string; readonly into?: string | undefined; readonly dryRun?: boolean | undefined }) =>
  <A>(self: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
    p.dryRun === true || p.into === undefined
      ? self
      : self.pipe(
          Effect.tap((report) => w.observeMerge(p.projectId, p.from, p.into!, { report }, by)),
          Effect.tapError((error) => w.observeMerge(p.projectId, p.from, p.into!, { error }, by)),
        );

/** `branch.set` observed: archiving supersedes the branch's conflict items. */
export const observingArchive =
  (w: ReviewWorkflow, by: string, p: { readonly projectId: string; readonly branch: string; readonly archived?: boolean | undefined; readonly dryRun?: boolean | undefined }) =>
  <A>(self: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
    p.archived !== true || p.dryRun === true ? self : Effect.tap(self, () => observeArchived(w, p.projectId, p.branch, by));
