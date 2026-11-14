// The chained `jobs.*` handlers over a test engine, one connection per principal (5C-2 tests).
import type { FinError, JobView, JobWatchEvent, Principal, Scope } from "@finisher/contracts";
import { Effect, Exit, type Stream } from "effect";
import { withChain } from "../../src/chain/chain.ts";
import type { ChainJobs, ChainProjects } from "../../src/chain/resolve.ts";
import { Connections } from "../../src/connections.ts";
import type { Audit, AuditInput } from "../../src/control/audit.ts";
import { CallSlots, chainJobs, jobHandlers } from "../../src/jobs/rpc.ts";
import type { TestEngine } from "./jobs.ts";

export const A = "01aaaaaaaaaaaaaaaaaaaaaaaa";
export const B = "01bbbbbbbbbbbbbbbbbbbbbbbb";
export const MINT: Scope[] = ["server.read", "project.read", "project.write", "review.write", "jobs.read", "jobs.submit", "library.read", "library.scan", "diagnostics.read", "diagnostics.record"];
/** An agent limited to project A (unless `more` says otherwise). */
export const agent = (id: string, more: Partial<Principal> = {}): Principal => ({ kind: "agent", id, scopes: MINT, projects: [A], branches: "*", human: false, ...more });

export const projects: ChainProjects = {
  isOpen: () => true,
  dirOf: () => undefined,
  resolveBranch: (_p, ref) => Effect.succeed(ref),
  branches: () => Effect.succeed([]),
};

/** The chained job handlers over `t`; `jobs: undefined` leaves the engine's view out of the chain. */
export function rpc(t: TestEngine, opts: { readonly slots?: CallSlots; readonly jobs?: ChainJobs | undefined } = {}) {
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  const audited: AuditInput[] = [];
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const slots = opts.slots ?? new CallSlots();
  const chained = withChain(jobHandlers({ engine: t.engine, guard: (_, e) => e, slots }), {
    connections,
    projects,
    audit,
    ...("jobs" in opts ? (opts.jobs === undefined ? {} : { jobs: opts.jobs }) : { jobs: chainJobs(t.engine) }),
  }) as unknown as Record<string, (p: unknown, o: unknown) => unknown>;
  let next = 1;
  const as = (principal: Principal) => {
    const id = next++;
    connections.attach(id, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(id)!.principal = principal;
    const options = { client: { id }, headers: {} };
    return {
      call: (tag: string, payload: unknown) => Effect.runPromiseExit(chained[tag]!(payload, options) as Effect.Effect<unknown, FinError>),
      ok: async <A = JobView>(tag: string, payload: unknown): Promise<A> => {
        const exit = await Effect.runPromiseExit(chained[tag]!(payload, options) as Effect.Effect<A, FinError>);
        if (Exit.isFailure(exit)) throw new Error(`${tag}: ${JSON.stringify(Exit.findErrorOption(exit))}`);
        return exit.value;
      },
      stream: (payload: unknown) => chained["jobs.watch"]!(payload, options) as Stream.Stream<JobWatchEvent, FinError>,
    };
  };
  return { as, audited, slots };
}

export const err = (exit: Exit.Exit<unknown, FinError>): FinError | undefined =>
  Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value;
export const code = (exit: Exit.Exit<unknown, FinError>) => err(exit)?.code;

/** `e` with every occurrence of `id` in its message and details replaced by `X`. */
export function normalised(e: FinError | undefined, id: string): unknown {
  if (e === undefined) return undefined;
  const swap = (v: unknown): unknown =>
    typeof v === "string" ? v.split(id).join("X") : Array.isArray(v) ? v.map(swap) : v !== null && typeof v === "object" ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, swap(x)])) : v;
  return { code: e.code, message: swap(e.message), details: swap(e.details), fix: e.fix };
}
