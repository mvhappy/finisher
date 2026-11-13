// `JobDone` from the job engine (7F plan Task 8, Decision 18; spec §9.1) and the render events
// nothing fires (Decision 17). `startAgentReviews` (`agent/wiring.ts`) subscribes the listener to
// `JobEngine.onDone` (5C's bus).
//
// - **One observe-only event per finished job**, queued off the engine's path, in the job's
//   project, as its owner (the live principal where there is one; a `system` owner is
//   `systemInvoker`, an ended credential keeps its identity, so the recursion guard still sees a
//   hook's own job); `match` is tested against the job kind. Payload `{jobId, kind, kindVersion,
//   state, project, owner, via?, error?: {code}}` (no params, no result: a hook asks `fin`).
// - **A job of no project** (a library's, the server's) is server-scoped: only the managed, user
//   and plugin layers see it, its request and payload name no project (Task 7 review). Project
//   hooks see their own project's events only.
// - **Never for a `plugin.hook` job** (a plugin's hook handler, Task 10): its end would fire hooks
//   that run handlers again.
// - **`PreRender`/`PostRender`** parse and are listed `inert: noRenderKind` (`config.ts`); no
//   source fires them until a render job kind exists (6E).
import type { HookEvent, JobOwner, JobView } from "@finisher/contracts";
import { type Invoker, type LiveInvoker, systemInvoker } from "../plugins/host/rights.ts";
import type { HookEngine } from "./engine.ts";

/** The job kind of a plugin's hook handler (Task 10): its end fires no `JobDone`. */
export const PLUGIN_HOOK_KIND = "plugin.hook";

/** Accepted in configuration, fired by nothing until a render job kind exists (Decision 17). */
export const INERT_EVENTS: readonly HookEvent[] = ["PreRender", "PostRender"];

export interface JobDoneDeps {
  readonly engine: Pick<HookEngine, "observe">;
  /** The owner's live principal (`liveInvokers`); absent or ended: its identity alone. */
  readonly invoker?: LiveInvoker;
}

/** `JobDone`'s payload for `job`. */
export function jobDonePayload(job: JobView) {
  return {
    jobId: job.id,
    kind: job.kind,
    kindVersion: job.kindVersion,
    state: job.state,
    ...(job.project === undefined ? {} : { project: job.project }),
    owner: { kind: job.owner.kind, id: job.owner.id },
    ...(job.via === undefined ? {} : { via: job.via }),
    ...(job.error === undefined ? {} : { error: { code: job.error.code } }),
  };
}

const identity = (owner: JobOwner, project: string | undefined): Invoker =>
  owner.kind === "system" ? systemInvoker(owner.id) : { kind: owner.kind, id: owner.id, scopes: [], projects: project === undefined ? [] : [project] };

/** The `onDone` listener. */
export function jobDoneHooks(d: JobDoneDeps): (job: JobView) => void {
  return (job) => {
    if (job.kind === PLUGIN_HOOK_KIND) return;
    const project = job.project;
    const principal = d.invoker?.(job.owner) ?? identity(job.owner, project);
    d.engine.observe("JobDone", { ...(project === undefined ? {} : { projectId: project }), principal, names: [job.kind] }, jobDonePayload(job));
  };
}
