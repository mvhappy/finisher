// `agent.savedPrompt.list|run` (sub-project 7B Task 10, spec §6.4). Running one is a person's:
// the chain's kinds (`SESSION_DRIVERS`) refuse an agent principal with `SCOPE_DENIED` before this
// runs, and `fin` has no command for it. A run expands the prompt (`SAVED_PROMPT_UNKNOWN`,
// `AGENT_ARGS`) and starts a session of the prompt's role and mode as the caller, as
// `agent.session.start` would, with the expansion as its first prompt and `trigger: savedPrompt`.
import type { AgentSavedPromptRunPayload, FinError, Principal } from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import { actorFor, type CallMeta } from "../diagnostics.ts";
import { toFinError } from "../errors.ts";
import type { Runtime } from "./runtime.ts";
import { expand, type SavedPrompts } from "./saved-prompts.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
type Options = { readonly headers: unknown; readonly client: { readonly id: number } };

export interface SavedPromptRpcDeps {
  readonly guard: Guard;
  readonly prompts: SavedPrompts;
  readonly runtime: Runtime;
  readonly principalOf: (options: object) => Principal;
}

export function savedPromptHandlers(deps: SavedPromptRpcDeps) {
  const { guard, prompts, runtime } = deps;
  return {
    "agent.savedPrompt.list": ({ projectId }: { projectId?: string }) =>
      guard("agent.savedPrompt.list", Effect.try({ try: () => prompts.list(projectId), catch: toFinError })),
    "agent.savedPrompt.run": (
      req: typeof AgentSavedPromptRunPayload.Type,
      options: Options,
    ) =>
      guard("agent.savedPrompt.run", Effect.suspend(() => {
        const p = deps.principalOf(options);
        const c = chainOf(options);
        const meta: CallMeta = { ...actorFor(p, options.headers), ...(c === undefined ? {} : { guard: c.guard }) };
        return Effect.flatMap(
          Effect.try({
            try: () => {
              const prompt = prompts.find(req.projectId, req.name);
              return { prompt, text: expand(prompt, req.params ?? {}) };
            },
            catch: toFinError,
          }),
          ({ prompt, text }) =>
            runtime.start(
              {
                projectId: req.projectId,
                role: prompt.role,
                mode: prompt.mode,
                prompt: text,
                title: req.title ?? prompt.title,
                ...(prompt.budget === undefined ? {} : { budget: prompt.budget }),
                ...(req.harness === undefined ? {} : { harness: req.harness }),
                ...(req.provider === undefined ? {} : { provider: req.provider }),
                ...(req.model === undefined ? {} : { model: req.model }),
              },
              p,
              meta,
              undefined,
              "savedPrompt",
            ),
        );
      })),
  };
}
