// The agent RPCs' handlers (sub-project 7 §3.5; plan 7A Tasks 2, 7 and 12) over the runtime
// `main.ts` builds (`wiring.ts`). The chain has checked scopes, kinds (`agent.session.*` driving
// calls are a person's in 7A) and the human-only challenge code before any of these runs.
//
// - **Reach:** a payload that names a session or an approval names no project, so every session
//   and approval handler (`approvals-rpc.ts`) resolves it through `sessionInReach`/`approvalInReach` (out of reach
//   answers exactly what a missing one does: `SESSION_NOT_FOUND`, `APPROVAL_NOT_FOUND`); lists
//   filter by the caller's projects (`projectsFilter`).
// - **Start** passes the call's actor and the chain's guard (`{...actorFor(by, headers), guard}`),
//   so the root branch is created exactly as the starter's own `branch.create` would be. From a
//   session it spawns a child of that session (7B Task 6; a `base` is refused: a child starts
//   from its parent's branch); `agent.session.list` from a session lists its own children.
// - **Watch** takes `fromNode` (a node id) as the seq after it, and a `WATCH_LAGGING` names the
//   last node delivered (`lastNode`) beside its seq, so the client watches again from it.
// - `agent.self` and `agent.self.try` (Task 7, Decision 14a): the calling session's own default
//   branch and tries, which `fin` reads and writes in place of `fin-config.json`.
// - The owner's settings (`agent.key.*`, `agent.local.*`, `training.closedTargets.*`) are
//   `admin.ts`'s; no answer holds a key.
import type { FinError, Principal, Session } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import { chainOf } from "../chain/chain.ts";
import { actorFor, type CallMeta } from "../diagnostics.ts";
import { finError, toFinError } from "../errors.ts";
import { approvalHandlers } from "./approvals-rpc.ts";
import type { BranchReach } from "./reach.ts";
import { projectsFilter, sessionInReach } from "./session-access.ts";
import type { AgentStore } from "./store.ts";
import type { AgentRuntimeParts } from "./wiring.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
type Options = { readonly headers: unknown; readonly client: { readonly id: number } };

export interface AgentRpcDeps {
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
  readonly store: AgentStore;
  readonly reach: BranchReach;
  readonly agents: AgentRuntimeParts;
  /** The principal the chain bound to the call. */
  readonly principalOf: (options: object) => Principal;
}

/** The session the calling credential is (its id); `SESSION_NOT_FOUND` for any other. */
function selfOf(store: AgentStore, options: object): Effect.Effect<Session, FinError> {
  const p = chainOf(options)?.principal;
  const s = p?.kind === "agent" ? store.get(p.id) : undefined;
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
const promised = <A>(f: () => Promise<A>) => Effect.tryPromise({ try: f, catch: toFinError });

const inReachOf = (p: Principal) => (projectId: string) => p.projects === "*" || p.projects.includes(projectId);

/** `WATCH_LAGGING` with the last delivered node's id beside its seq (`fromNode` for the next watch). */
function lagging(deps: AgentRpcDeps, sessionId: string, e: FinError): FinError {
  if (e.code !== "WATCH_LAGGING") return e;
  const details = (e.details ?? {}) as { lastSeq?: unknown };
  const lastNode = typeof details.lastSeq === "number" && details.lastSeq > 0 ? deps.agents.lookup.nodeIdAt(sessionId, details.lastSeq) : undefined;
  return finError(e.code, e.message, { ...details, ...(lastNode === undefined ? {} : { lastNode }) }, "watch again with fromNode set to details.lastNode");
}

export function agentHandlers(deps: AgentRpcDeps) {
  const { guard, store, agents } = deps;
  const { runtime, admin } = agents;
  const by = deps.principalOf;
  /** `f` on the session `sessionId` when it is in the caller's reach. */
  const onSession = <A>(tag: string, options: object, sessionId: string, f: (s: Session, p: Principal) => Effect.Effect<A, FinError>) =>
    guard(tag, Effect.suspend(() => {
      const p = by(options);
      return Effect.flatMap(sessionInReach(store, p, sessionId), (s) => f(s, p));
    }));
  const metaOf = (options: Options): CallMeta => {
    const c = chainOf(options);
    return { ...actorFor(by(options), options.headers), ...(c === undefined ? {} : { guard: c.guard }) };
  };

  return {
    "agent.session.start": (req: Parameters<typeof runtime.start>[0], options: Options) =>
      guard("agent.session.start", Effect.suspend(() => {
        const p = by(options);
        const parent = p.kind === "agent" ? store.get(p.id) : undefined;
        if (parent === undefined) return runtime.start(req, p, metaOf(options));
        if (req.base !== undefined) return Effect.fail(finError("INVALID_REQUEST", "a child starts from its parent's branch", { field: "base", reason: "subagent" }, "spawn it without base"));
        if (req.prompt === undefined || req.prompt.trim() === "") return Effect.fail(finError("INVALID_REQUEST", "a child starts with a brief", { field: "prompt", reason: "subagent" }, "spawn it with a brief (`fin agent spawn --brief`)"));
        // Reach keeps a session to its project; a child is of its parent's in any case.
        if (req.projectId !== parent.projectId) return Effect.fail(finError("PROJECT_NOT_FOUND", "no such project", { projectId: req.projectId }, "spawn children in your own project"));
        return runtime.spawnChild(parent, req, p, metaOf(options));
      })),
    "agent.session.wait": ({ ids, any, timeoutMs }: { ids: ReadonlyArray<string>; any?: boolean; timeoutMs: number }, options: Options) =>
      guard("agent.session.wait", Effect.map(runtime.waitChildren(by(options), ids, any === true, timeoutMs), (children) => ({ children }))),
    "agent.session.prompt": ({ sessionId, text, attachments }: { sessionId: string; text: string; attachments?: ReadonlyArray<unknown> }, options: Options) =>
      onSession("agent.session.prompt", options, sessionId, (s, p) => Effect.map(runtime.prompt(s.id, text, p, attachments), (run) => ({ run }))),
    "agent.session.steer": ({ sessionId, text }: { sessionId: string; text: string }, options: Options) =>
      onSession("agent.session.steer", options, sessionId, (s, p) => runtime.steer(s.id, text, p)),
    "agent.session.interrupt": ({ sessionId }: { sessionId: string }, options: Options) =>
      onSession("agent.session.interrupt", options, sessionId, (s, p) => Effect.map(runtime.interrupt(s.id, p), (session) => ({ session }))),
    "agent.session.resume": ({ sessionId }: { sessionId: string }, options: Options) =>
      onSession("agent.session.resume", options, sessionId, (s, p) => Effect.map(runtime.resume(s.id, p), (r) => ({ session: r.session }))),
    "agent.session.end": ({ sessionId }: { sessionId: string }, options: Options) =>
      onSession("agent.session.end", options, sessionId, (s, p) => Effect.map(runtime.end(s.id, p), (session) => ({ session }))),
    "agent.session.list": (q: { projectId?: string; status?: Session["status"]; cursor?: string; limit?: number }, options: Options) =>
      guard("agent.session.list", sync(() => {
        const p = by(options);
        const projects = projectsFilter(p);
        // A session sees its own children only (7B Task 6: `fin agent ls`).
        const own = p.kind === "agent" && store.get(p.id) !== undefined ? { parent: p.id } : {};
        return store.list({ ...q, ...(projects === undefined ? {} : { projects }), ...own });
      })),
    "agent.session.get": ({ sessionId }: { sessionId: string }, options: Options) =>
      onSession("agent.session.get", options, sessionId, (s) =>
        // `projectRules`: whether the project's `finisher.rules.json` applies (7B Task 2: ignored while untrusted or changed).
        sync(() => ({ session: s, runs: store.runs(s.id), approvals: store.approvals(s.id).filter((a) => a.status === "open"), projectRules: agents.rules.project(s.projectId).status })),
      ),
    // The owner's (`agent.admin`, challenge code): ends it, abandons its unmerged transactions (Task 11), removes it.
    "agent.session.delete": ({ sessionId }: { sessionId: string }, options: Options) =>
      onSession("agent.session.delete", options, sessionId, (s, p) => Effect.as(runtime.delete(s.id, p), { deleted: true as const })),
    "agent.watch": ({ sessionId, fromNode }: { sessionId: string; fromNode?: string }, options: Options) =>
      Stream.unwrap(
        Effect.map(
          Effect.flatMap(sessionInReach(store, by(options), sessionId), (s) => {
            if (fromNode === undefined) return Effect.succeed({ s, fromSeq: 1 });
            const seq = agents.lookup.nodeSeq(s.id, fromNode);
            return seq === undefined
              ? Effect.fail(finError("NOT_FOUND", "the session has no such node", { sessionId, fromNode }, "watch from a node id this session's watch gave you, or from the start"))
              : Effect.succeed({ s, fromSeq: seq + 1 });
          }),
          ({ s, fromSeq }) => runtime.watch(s.id, fromSeq).pipe(Stream.mapError((e) => lagging(deps, s.id, e))),
        ),
      ).pipe(Stream.catchDefect((defect) => Stream.fail(toFinError(defect)))),
    // `agent.approval.list|wait|answer` (7B Task 5: waits, answers by kind, `remember`).
    ...approvalHandlers({ guard, store, lookup: agents.lookup, runtime, approvals: agents.approvals, principalOf: by }),
    "agent.providers": (_: unknown, options: Options) =>
      guard("agent.providers", Effect.suspend(() => admin.providers(agents.adapters, inReachOf(by(options))))),
    "agent.self": (_: unknown, options: Options) => guard("agent.self", Effect.flatMap(selfOf(store, options), (s) => sync(() => selfView(store, s)))),
    /**
     * `record` a try (its branch must be in the session's subtree), make one (or the root) the
     * default, or `clear` one (restoring the default it replaced). Branches are kept by id.
     */
    "agent.self.try": ({ action, branch }: { readonly action: "record" | "setDefault" | "clear"; readonly branch: string }, options: Options) =>
      guard(
        "agent.self.try",
        Effect.gen(function* () {
          const s = yield* selfOf(store, options);
          const id = yield* deps.reach.inSubtree(s, branch);
          yield* sync(() => {
            if (action === "record") store.recordTry(s.id, id);
            else if (action === "setDefault") store.setDefaultBranch(s.id, id);
            else store.clearTry(s.id, id);
          });
          return yield* sync(() => selfView(store, s));
        }),
      ),
    // The owner's settings (`admin.ts`).
    "agent.key.set": (req: { provider: string; key: string; label?: string; projectId?: string }, options: Options) =>
      guard("agent.key.set", promised(async () => ({ key: await admin.keySet(by(options), req) }))),
    "agent.key.list": (_req: unknown, options: Options) => guard("agent.key.list", sync(() => ({ keys: admin.keyList(by(options)) }))),
    "agent.key.remove": ({ id }: { id: string }, options: Options) =>
      guard("agent.key.remove", promised(async () => ({ removed: await admin.keyRemove(by(options), id) }))),
    "agent.local.set": ({ endpoint }: { endpoint: string }, options: Options) =>
      guard("agent.local.set", sync(() => {
        const { preset, warnings } = admin.localSet(by(options), endpoint);
        return { preset, ...(warnings.length === 0 ? {} : { warnings }) };
      })),
    "agent.local.show": () => guard("agent.local.show", sync(() => admin.localShow())),
    "agent.local.test": () => guard("agent.local.test", promised(() => admin.localTest())),
    "agent.local.remove": (_: unknown, options: Options) => guard("agent.local.remove", sync(() => ({ removed: admin.localRemove(by(options)) }))),
    "training.closedTargets.get": () => guard("training.closedTargets.get", sync(() => ({ state: admin.closedTargets() }))),
    "training.closedTargets.set": ({ state }: { state: "pendingCounsel" | "cleared" | "excluded" }, options: Options) =>
      guard("training.closedTargets.set", sync(() => ({ state: admin.setClosedTargets(by(options), state) }))),
  };
}
