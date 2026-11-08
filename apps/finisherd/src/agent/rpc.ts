// The agent RPCs' handlers (sub-project 7A). Task 2 fixes the contracts, their scopes and the
// chain's refusals; the runtime behind them arrives with Tasks 3b–12, and Task 12 replaces this
// file. Until then every call that passes the chain answers `SERVER_UNAVAILABLE`, so the chain,
// the audit and `server.hello`'s capabilities can already be exercised end to end.
import type { FinError } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import { finError } from "../errors.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface AgentRpcDeps {
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
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
    "agent.self": unavailable("agent.self", deps),
    "agent.self.try": unavailable("agent.self.try", deps),
  };
}
