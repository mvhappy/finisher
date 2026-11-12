// A chain over recording handlers with several bound clients (5E Task 6): client ids map to
// principals, every write RPC records what reached it, and `real` replaces handlers.
import { type FinError, type Principal, WRITE_RPCS } from "@finisher/contracts";
import { Effect, Exit, type Fiber } from "effect";
import { issueChallenge } from "../../src/auth/challenges.ts";
import { type ChainDeps, withChain } from "../../src/chain/chain.ts";
import type { ChainProjects, ChainReviews } from "../../src/chain/resolve.ts";
import { Connections } from "../../src/connections.ts";
import type { Audit, AuditInput } from "../../src/control/audit.ts";

export interface Seen {
  readonly tag: string;
  readonly payload: unknown;
  readonly options: Record<string, unknown>;
}

export interface Harness {
  readonly seen: Seen[];
  readonly audited: AuditInput[];
  readonly audit: Audit;
  call(client: number, tag: string, payload: unknown): Promise<Exit.Exit<unknown, FinError>>;
  /** The call as a running fiber (to interrupt it). */
  fork(client: number, tag: string, payload: unknown): Fiber.Fiber<unknown, FinError>;
  /** A fresh `console.challenge` code of `client`. */
  challenge(client: number): string;
}

export function chainHarness(
  principals: Record<number, Principal>,
  deps: { readonly projects: ChainProjects; readonly reviews?: ChainReviews } & Partial<ChainDeps>,
  real: (h: { readonly audit: Audit }) => Record<string, unknown> = () => ({}),
): Harness {
  const seen: Seen[] = [];
  const audited: AuditInput[] = [];
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  for (const [id, p] of Object.entries(principals)) {
    connections.attach(Number(id), { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(Number(id))!.principal = p;
  }
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const handlers: Record<string, unknown> = {};
  for (const t of [...WRITE_RPCS, "project.acknowledgeRewind", "review.policy", "review.policy.get"]) {
    handlers[t] = (payload: unknown, options: Record<string, unknown>) =>
      Effect.sync(() => {
        seen.push({ tag: t, payload, options });
        return { ok: true };
      });
  }
  Object.assign(handlers, real({ audit }));
  const chained = withChain(handlers, { connections, audit, ...deps }) as unknown as Record<string, (p: unknown, o: unknown) => unknown>;
  return {
    seen,
    audited,
    audit,
    call: (client, tag, payload) => Effect.runPromiseExit(chained[tag]!(payload, { client: { id: client }, headers: {} }) as Effect.Effect<unknown, FinError>),
    fork: (client, tag, payload) => Effect.runFork(chained[tag]!(payload, { client: { id: client }, headers: {} }) as Effect.Effect<unknown, FinError>),
    challenge: (client) => issueChallenge(connections.get(client)!).code,
  };
}

export const errorOf = (exit: Exit.Exit<unknown, FinError>): FinError | undefined =>
  Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value;

export const codeOf = (exit: Exit.Exit<unknown, FinError>): string | undefined => errorOf(exit)?.code;
