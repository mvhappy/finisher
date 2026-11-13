// Chain events (7F plan Task 7; Decisions 1, 14, 15; spec §9.1): the hooks of an RPC call, in
// the chain's stage 5 (policy) and stage 7 (after). `buildHooks` makes them; `handlers.ts`
// appends `policies` after the command rules and wraps its after stage with `wrapAfter`.
//
// - **`PreCommand`** (blocking before): a `PolicyRule` after `commandRules`, so a rule's `deny`
//   or `ask` ends the stage first and a hook never sees a call the rules refuse (nor one waiting
//   on a card). It fires for every call that names a project (its payload's `projectId`, else
//   the session's, else the first project stage 3 resolved); the engine keeps it to `agent`
//   principals unless a hook says `principals: "all"`. `match` is tested against the call's
//   command names (`commandsOf`, a plugin host's commit named by its command too). Payload
//   `{rpc, command, commands, payload}` (the resolved payload; one over 1 MiB reaches the hook
//   as `{payloadOmitted, bytes}`, `process.ts`). A block is `HOOK_BLOCKED` (exit 7; the engine
//   audits `hook.block` as the caller); the chain then gives back what earlier rules took (a
//   one-time grant consumed for the call, `Outcome.undo`). A dry run fires it too. An allow
//   with context is handed to the session port (`SessionNotices.deliver`: steered into the
//   running turn, else held; a call of no session only logs it).
// - **`PreMerge`** (blocking before, fails closed by default): a `PolicyRule` for
//   `history.merge` (`{via: "rpc", rpc, from, into}`, the refs as sent) and `review.merge` (the
//   item read through the reviews workflow: `from`/`into` its source and target, and `item`
//   with its id, kind, title, source, target and snapshot), for **every** principal (a
//   person's merge is policy too). It runs while the call holds its project's policy gate (5E),
//   so a slow hook delays policy writes on that project by at most its timeout.
// - **`PostCommand`** (blocking after) and **`PostMerge`** (observe-only) from stage 7, only
//   after a call that answered, queued off its path (`HookEngine.observe`): nothing they do
//   changes the answer. A `PostCommand` block stops the later `PostCommand` hooks, is audited
//   `hook.block {after: true}` and reaches the session as a notice and as context (the engine
//   and the session port). `PostMerge`'s payload is `PreMerge`'s without the item, with
//   `itemId` and the `mergeId` the call answered.
// - **Who.** The hooks see the caller's principal and, for an agent session's credential, its
//   session. A hook's own `fin` calls fire nothing (the engine's recursion guard). An
//   `anonymous` caller fires nothing (it reaches only the public RPCs).
//
// The `auto` merge path does not pass the chain (5E's internal path merges through `mergeReview`
// directly): its `PreMerge`/`PostMerge` are `review.ts`'s, so a merge fires them once.
import type { FinError, FinRpcTag, Principal, ReviewItem, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { commandsOf } from "../agent/command-of.ts";
import type { ChainHooks } from "../chain/hooks.ts";
import type { Outcome, PolicyInput, PolicyRule } from "../chain/policy.ts";
import { finError } from "../errors.ts";
import { type HookAt, type HookEngine, type HookOutcome, hookBlockedError, type SessionNotices } from "./engine.ts";

/** The reviews workflow's item read (`ReviewWorkflow.item`). */
export interface ReviewItems {
  item(itemId: string): Effect.Effect<ReviewItem, FinError>;
}

export interface ChainHooksDeps {
  readonly engine: Pick<HookEngine, "fire" | "observe">;
  /** An agent principal's session (its id is the session id). */
  readonly sessions: { get(id: string): Session | undefined };
  readonly notices?: SessionNotices;
  /** The reviews workflow, once the reviews have started (`HooksWiring.bindReviews`). */
  readonly reviews: () => ReviewItems | undefined;
  readonly log?: (message: string) => void;
}

export interface ChainHookPoints {
  readonly preCommand: PolicyRule;
  readonly preMerge: PolicyRule;
  /** `hooks` with `PostCommand` and `PostMerge` after its own `after`. */
  readonly withHooksAfter: (hooks: ChainHooks) => ChainHooks;
}

const MERGES: ReadonlySet<string> = new Set(["history.merge", "review.merge"]);

const field = (payload: unknown, key: string): unknown => (payload !== null && typeof payload === "object" ? (payload as Record<string, unknown>)[key] : undefined);
const text = (payload: unknown, key: string): string | undefined => {
  const v = field(payload, key);
  return typeof v === "string" ? v : undefined;
};
const opt = <K extends string>(key: K, v: string | undefined) => (v === undefined ? {} : ({ [key]: v } as Record<K, string>));

export function chainHooks(d: ChainHooksDeps): ChainHookPoints {
  const sessionOf = (p: Principal) => (p.kind === "agent" ? d.sessions.get(p.id) : undefined);

  /** Where a call's hooks run: its project and session; `undefined` when it names no project. */
  const atOf = (principal: Principal, payload: unknown, projectIds?: ReadonlyArray<string>, names?: readonly string[]): HookAt | undefined => {
    if (principal.kind === "anonymous") return undefined;
    const session = sessionOf(principal);
    const projectId = text(payload, "projectId") ?? session?.projectId ?? projectIds?.[0];
    if (projectId === undefined) return undefined;
    return { projectId, principal, ...(session === undefined ? {} : { session }), ...(names === undefined ? {} : { names }) };
  };

  /** A blocking-before outcome: its block refuses the call; its context goes to the session. */
  const decided = (event: "PreCommand" | "PreMerge", at: HookAt, out: HookOutcome): Outcome | undefined => {
    if (out.blocked !== undefined) return { deny: hookBlockedError(event, out.blocked) };
    if (out.context !== undefined) d.notices?.deliver(at, event, out);
    return undefined;
  };

  const preCommand: PolicyRule = (input) =>
    Effect.suspend(() => {
      const commands = commandsOf(input.tag, input.payload, input.host === undefined ? undefined : { via: input.host.via });
      const at = atOf(input.principal, input.payload, input.projectIds, commands);
      if (at === undefined) return Effect.succeed(undefined);
      const payload = { rpc: input.tag, command: commands[0], commands, payload: input.payload };
      return Effect.map(d.engine.fire("PreCommand", at, payload), (out) => decided("PreCommand", at, out));
    });

  /** `PreMerge`'s payload: the refs of a `history.merge`, the item of a `review.merge`. */
  const mergePayload = (input: PolicyInput): Effect.Effect<Record<string, unknown>, FinError> => {
    const base = { via: "rpc", rpc: input.tag };
    if (input.tag === "history.merge") return Effect.succeed({ ...base, ...opt("from", text(input.payload, "from")), ...opt("into", text(input.payload, "into")) });
    const reviews = d.reviews();
    const itemId = text(input.payload, "itemId");
    if (reviews === undefined || itemId === undefined) return Effect.fail(finError("INTERNAL", "a review merge reached the PreMerge hooks without the reviews workflow", { reason: "noReviews" }));
    return Effect.map(reviews.item(itemId), (item) => ({
      ...base,
      ...opt("from", item.source),
      ...opt("into", item.target),
      item: { id: item.id, kind: item.kind, title: item.title, ...opt("source", item.source), ...opt("target", item.target), ...(item.snapshot === undefined ? {} : { snapshot: item.snapshot }) },
    }));
  };

  const preMerge: PolicyRule = (input) =>
    Effect.gen(function* () {
      if (!MERGES.has(input.tag)) return undefined;
      const at = atOf(input.principal, input.payload, input.projectIds);
      if (at === undefined) return undefined;
      const payload = yield* mergePayload(input);
      return decided("PreMerge", at, yield* d.engine.fire("PreMerge", at, payload));
    });

  /** Stage 7, after a call that answered: queued, never failing or delaying it. */
  const observeAfter = (tag: FinRpcTag, principal: Principal, payload: unknown, result: unknown): void => {
    const commands = commandsOf(tag, payload);
    const at = atOf(principal, payload, undefined, commands);
    if (at === undefined) return;
    d.engine.observe("PostCommand", at, { rpc: tag, command: commands[0], commands, payload });
    if (!MERGES.has(tag)) return;
    const { names: _, ...merged } = at;
    const mergeId = text(result, "mergeId") ?? text(field(result, "item"), "mergeId");
    const refs = tag === "history.merge" ? { ...opt("from", text(payload, "from")), ...opt("into", text(payload, "into")) } : { ...opt("itemId", text(payload, "itemId")), ...opt("from", text(payload, "source")), ...opt("into", text(payload, "target")) };
    d.engine.observe("PostMerge", merged, { via: "rpc", rpc: tag, ...refs, ...opt("mergeId", mergeId) });
  };

  const withHooksAfter = (hooks: ChainHooks): ChainHooks => ({
    ...hooks,
    after: (tag, principal, payload, result, headers) =>
      Effect.andThen(
        hooks.after === undefined ? Effect.void : hooks.after(tag, principal, payload, result, headers),
        Effect.sync(() => {
          try {
            observeAfter(tag, principal, payload, result);
          } catch (e) {
            d.log?.(`hooks: ${tag}'s after hooks not queued: ${e instanceof Error ? e.message : String(e)}`);
          }
        }),
      ),
  });

  return { preCommand, preMerge, withHooksAfter };
}
