// The `plugin.command` v1 kind (sp8b plan Decision 7, Task 9): a command run is a job of the
// `plugin` family, warm, interactive, never retried (it may have reached an external API), with
// no files and no resources of a pool; the RPC that runs it (`plugins/host/command-run.ts`)
// cancels it at the command's own timeout. Its `start.extra` is the job's `PluginContext`
// (Decision 2). Never submittable by RPC (Decision 6).
import type { CommandRun, PluginContext } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { toFinError } from "../../errors.ts";
import type { KindSpec } from "../kinds.ts";

export const COMMAND_KIND = "plugin.command";

export interface CommandKindDeps {
  /** `PluginHost.contextOf`. */
  readonly contextOf: (job: JobView, opts: { readonly timeoutS: number }) => PluginContext;
  /** The command's timeout in seconds (`CommandRegistry.timeoutOf`). */
  readonly timeoutOf: (plugin: string, command: string) => number;
}

export function pluginCommandKind(d: CommandKindDeps): KindSpec<CommandRun> {
  return {
    kind: COMMAND_KIND,
    version: 1,
    family: "plugin",
    scopes: [],
    requiresProject: true,
    resources: [],
    warm: true,
    defaultPriority: "interactive",
    qos: "normal",
    // The manifest maximum: the RPC cancels earlier at the command's own (Decision 7).
    timeoutS: 60,
    maxAttempts: 1,
    prepare: (ctx) =>
      Effect.try({
        try: () => ({ extra: d.contextOf(ctx.job, { timeoutS: d.timeoutOf(ctx.job.via?.plugin ?? "", ctx.job.params.command) }) }),
        catch: toFinError,
      }),
  };
}
