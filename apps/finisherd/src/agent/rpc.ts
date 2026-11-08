// The agent RPCs' handlers (sub-project 7A). Task 2 fixes the contracts, their scopes and the
// chain's refusals; the runtime behind them arrives with Tasks 3b–12, and Task 12 replaces this
// file. Until then every other call that passes the chain answers `SERVER_UNAVAILABLE`, so the chain,
// the audit and `server.hello`'s capabilities can already be exercised end to end.
//
// `agent.self` and `agent.self.try` (Task 7, Decision 14a) are served now: the calling session's
// own default branch and tries, which `fin` reads and writes in place of `fin-config.json`.
import type { FinError, Session } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import { chainOf } from "../chain/chain.ts";
import { finError, toFinError } from "../errors.ts";
import type { BranchReach } from "./reach.ts";
import type { AgentStore } from "./store.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface AgentRpcDeps {
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
  readonly store?: AgentStore;
  readonly reach?: BranchReach;
}

/** The session the calling credential is (its id); `SESSION_NOT_FOUND` for any other. */
function selfOf(deps: AgentRpcDeps, options: object): Effect.Effect<Session, FinError> {
  const p = chainOf(options)?.principal;
  const s = p?.kind === "agent" ? deps.store?.get(p.id) : undefined;
  return s === undefined
    ? Effect.fail(finError("SESSION_NOT_FOUND", "this credential is not an agent session's", {}, "Run this from inside an agent session."))
    : Effect.succeed(s);
}

function selfView(store: AgentStore, s: Session) {
  const st = store.state(s.id);
  return {
    sessionId: s.id,
    projectId: s.projectId,
    ...(s.rootBranch === undefined ? {} : { rootBranch: s.rootBranch }),
    defaultBranch: st.defaultBranch,
    tries: st.tries.map((t) => ({ branch: t.branch, previousDefault: t.previousDefault })),
  };
}

const sync = <A>(f: () => A) => Effect.try({ try: f, catch: toFinError });

function agentSelf(deps: AgentRpcDeps) {
  return (_: unknown, options: object) =>
    deps.guard("agent.self", Effect.flatMap(selfOf(deps, options), (s) => sync(() => selfView(deps.store!, s))));
}

/**
 * `record` a try (its branch must be in the session's subtree), make one (or the root) the
 * default, or `clear` one (restoring the default it replaced). Branches are kept by id.
 */
function agentSelfTry(deps: AgentRpcDeps) {
  return ({ action, branch }: { readonly action: "record" | "setDefault" | "clear"; readonly branch: string }, options: object) =>
    deps.guard(
      "agent.self.try",
      Effect.gen(function* () {
        const s = yield* selfOf(deps, options);
        const store = deps.store!;
        const id = yield* deps.reach!.inSubtree(s, branch);
        yield* sync(() => {
          if (action === "record") store.recordTry(s.id, id);
          else if (action === "setDefault") store.setDefaultBranch(s.id, id);
          else store.clearTry(s.id, id);
        });
        return yield* sync(() => selfView(store, s));
      }),
    );
}

const unwired = (tag: string): FinError =>
  finError(
    "SERVER_UNAVAILABLE",
    `${tag}: the agent runtime is not running on this server yet`,
    { reason: "agentRuntimeNotWired" },
    "This version of Finisher has the agent calls defined but not switched on. Update Finisher.",
  );

/** One handler failing as unwired, under the call's timing. */
const unavailable = (tag: string, deps: AgentRpcDeps) => () => deps.guard(tag, Effect.fail(unwired(tag)));

export function agentHandlers(deps: AgentRpcDeps) {
  return {
    "agent.session.start": unavailable("agent.session.start", deps),
    "agent.session.prompt": unavailable("agent.session.prompt", deps),
    "agent.session.steer": unavailable("agent.session.steer", deps),
    "agent.session.interrupt": unavailable("agent.session.interrupt", deps),
    "agent.session.resume": unavailable("agent.session.resume", deps),
    "agent.session.end": unavailable("agent.session.end", deps),
    "agent.session.list": unavailable("agent.session.list", deps),
    "agent.session.get": unavailable("agent.session.get", deps),
    "agent.session.delete": unavailable("agent.session.delete", deps),
    "agent.watch": () => Stream.fail(unwired("agent.watch")),
    "agent.approval.list": unavailable("agent.approval.list", deps),
    "agent.approval.answer": unavailable("agent.approval.answer", deps),
    "agent.providers": unavailable("agent.providers", deps),
    "agent.self": deps.store === undefined ? unavailable("agent.self", deps) : agentSelf(deps),
    "agent.self.try": deps.store === undefined || deps.reach === undefined ? unavailable("agent.self.try", deps) : agentSelfTry(deps),
  };
}
