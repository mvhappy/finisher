// The `plugin.hook` v1 kind (7F plan Task 10, Decision 20; sp8b Decision 7's shape): one run of a
// plugin's hook handler is a job of the `plugin` family, warm, interactive, never retried, with no
// files and no resources of a pool; `plugins/host/hooks.ts` submits it and cancels it at the
// handler's own timeout (the manifest's `timeoutS`), so the kind's 60 s is only the ceiling. Its
// `start.extra` is the job's `PluginContext`. Never submittable by RPC (sp8b Decision 6: no
// `rpc`), and its end fires no `JobDone` (`hooks/jobs.ts`).
//
// The kind carries the server's plugin hook source (`source`): the hook engine's plugin layer
// finds it through the job engine's kind registry (`pluginHooksOf`, `startAgentReviews`), so the
// plugin host that registered the kind is the one whose handlers run.
import type { HookRun, PluginContext } from "@finisher/core-types";
import type { JobView } from "@finisher/contracts";
import { Effect } from "effect";
import { toFinError } from "../../errors.ts";
import type { PluginHookSource } from "../../hooks/wiring.ts";
import type { KindRegistry, KindSpec } from "../kinds.ts";

/** `fin_plugin_api::protocol::hook::HOOK_KIND`. */
export const HOOK_KIND = "plugin.hook";
/** `HOOK_KIND_VERSION`. */
export const HOOK_KIND_VERSION = 1;

export interface HookKindDeps {
  /** `PluginHost.contextOf`. */
  readonly contextOf: (job: JobView, opts: { readonly timeoutS: number }) => PluginContext;
  /** The handlers of the enabled plugins and their runs (`plugins/host/hooks.ts`). */
  readonly source: PluginHookSource;
}

export interface PluginHookKind extends KindSpec<HookRun> {
  readonly source: PluginHookSource;
}

export function pluginHookKind(d: HookKindDeps): PluginHookKind {
  return {
    kind: HOOK_KIND,
    version: HOOK_KIND_VERSION,
    family: "plugin",
    scopes: [],
    resources: [],
    warm: true,
    defaultPriority: "interactive",
    qos: "normal",
    // The manifest maximum: the source cancels earlier at the handler's own.
    timeoutS: 60,
    maxAttempts: 1,
    prepare: (ctx) =>
      Effect.try({
        try: () => ({ extra: d.contextOf(ctx.job, { timeoutS: d.source.timeoutOf(ctx.job.via?.plugin ?? "", ctx.job.params.request.event, ctx.job.params.handler) }) }),
        catch: toFinError,
      }),
    source: d.source,
  };
}

/** The plugin hook source of the server whose engine has `kinds`, if a plugin host registered one. */
export function pluginHooksOf(kinds: Pick<KindRegistry, "kind">): PluginHookSource | undefined {
  return (kinds.kind(HOOK_KIND) as Partial<PluginHookKind> | undefined)?.source;
}
