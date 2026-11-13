// `agent.hooks.list {projectId?}` (7F plan Task 2): every layer's hooks as a person sees them
// (`agent.admin`, people only, no code: it changes nothing; the chain checked the project's
// reach). With a project, its `finisher.hooks.json` is listed whether or not it applies, with
// its status and the `hooksHash` that `agent.project.trust` binds (`expectedHooksHash`).
import type { AgentHooksListSuccess, FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { finError, toFinError } from "../errors.ts";
import type { HookConfig } from "./config.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface HooksRpcDeps {
  readonly guard: Guard;
  readonly hooks: HookConfig;
  /** The folder of a project (open, else indexed). */
  readonly dirOf: (projectId: string) => string | undefined;
}

export function hooksHandlers(deps: HooksRpcDeps) {
  return {
    "agent.hooks.list": ({ projectId }: { projectId?: string }) =>
      deps.guard(
        "agent.hooks.list",
        Effect.try({
          try: (): AgentHooksListSuccess => {
            if (projectId !== undefined && deps.dirOf(projectId) === undefined) {
              throw finError("PROJECT_NOT_FOUND", "no such project on this server", { projectId }, "open the project first");
            }
            return deps.hooks.list(projectId);
          },
          catch: toFinError,
        }),
      ),
  };
}
