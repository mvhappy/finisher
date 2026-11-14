// The project registry's reads at a branch (`projects.ts`): each resolves the requested branch
// reference (a name, an id or `name#id`) to its id first, then asks the engine at that id.
import type { AudioPlan, FontsPlan, FramePlan } from "@finisher/core-types";
import type { FinError, HistoryLogResult } from "@finisher/contracts";
import type { Engine } from "@finisher/fin-node";
import { Effect } from "effect";
import type { MediaJobPrepared, ProjectRegistryShape } from "./projects.ts";

type Resolve = (projectId: string, branch: string) => Effect.Effect<string, FinError>;
type On = <A>(projectId: string, f: (engine: Engine) => Promise<string>) => Effect.Effect<A, FinError>;

export function branchReads(
  resolveBranch: Resolve,
  on: On,
): Pick<ProjectRegistryShape, "evalFrame" | "evalAudio" | "fontsPlan" | "prepareMediaJob" | "historyLog" | "onBranch"> {
  const at = <A>(projectId: string, requested: string, f: (engine: Engine, branch: string) => Promise<string>) =>
    Effect.flatMap(resolveBranch(projectId, requested), (branch) => on<A>(projectId, (e) => f(e, branch)));
  return {
    evalFrame: (projectId, requested, timeline, t, rev) => at<FramePlan>(projectId, requested, (e, b) => e.evalFrame(b, timeline, t, rev)),
    evalAudio: (projectId, requested, timeline, from, to, rev) =>
      at<AudioPlan>(projectId, requested, (e, b) => e.evalAudio(b, timeline, from, to, rev)),
    fontsPlan: (projectId, requested, ops) => at<FontsPlan>(projectId, requested, (e, b) => e.fontsPlan(b, JSON.stringify(ops))),
    prepareMediaJob: (projectId, requested, timeline, times, inputDir, fontCache, paramsJson) =>
      at<MediaJobPrepared>(projectId, requested, (e, b) =>
        e.prepareMediaJob(b, timeline, [...times], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null }),
      ),
    // Review batch L: resolved like every other branch read.
    historyLog: (projectId, requested, before, limit) => at<HistoryLogResult>(projectId, requested, (e, b) => e.log(b, before, limit)),
    // 6B-1: any other engine read at a resolved branch (`asset.reprobe`'s probe).
    onBranch: at,
  };
}
