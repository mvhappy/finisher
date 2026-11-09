// ProjectRegistry: open Engines, the `app.db` project index, per-(project, branch) op hubs and
// per-project event hubs fed by each engine's `onEvent`.
import { resolve } from "node:path";
import type { AudioPlan, BranchView, CommittedTx, FontsPlan, FramePlan } from "@finisher/core-types";
import type { EngineEvent, FinError, Generation, ProjectEvent, TxGetResult } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Context, Effect, Layer, PubSub, type Scope, Semaphore, Stream } from "effect";
import { attempt, attemptJson, finError, isEngineDead } from "./errors.ts";
import type { CallMeta } from "./diagnostics.ts";
import { callEngine, type HistoryOp, type HistoryPayload, metaJson, publishResult } from "./history.ts";
import { follower, Hubs, type Stamped } from "./hubs.ts";
import { ProjectIndex } from "./project-index.ts";

/** What `prepareMediaJob` resolves to: the rev written, the plan's BLAKE3 at each time and the files the worker opens. */
export interface MediaJobPrepared {
  readonly rev: number;
  readonly planSamples: ReadonlyArray<{ readonly at: string; readonly blake3: string }>;
  readonly sources?: ReadonlyArray<string>;
}

export interface ProjectRef {
  readonly projectId: string;
  readonly rev: number;
}

export interface OpenExpect {
  readonly projectId?: string | undefined;
  readonly attachOnly?: boolean;
}

/** The folder holds another project than the one checked (a swap between check and open). */
const changed = (dir: string, projectId: string) =>
  finError("PROJECT_NOT_FOUND", `the folder ${dir} no longer holds project ${projectId}`, { dir, projectId, reason: "changed" }, "open the project by its folder again");

/** One open project: its engine and a commit lock. */
interface OpenProject {
  readonly engine: Engine;
  readonly dir: string;
  /** Serializes commit + publish so hubs see revs in commit order. */
  readonly commitLock: Semaphore.Semaphore;
  /** Releases the libraries its assets live in (5D Decision 28). */
  release?: () => void;
}

export interface ProjectRegistryShape {
  /** `idemKey`: a retry with the same key returns the project that create made. */
  readonly create: (dir: string, name: string, rate: string, idemKey?: string) => Effect.Effect<ProjectRef, FinError>;
  /**
   * `expect.projectId`: the project the chain read in the folder: another one there now is
   * `PROJECT_NOT_FOUND {reason: changed}`, checked before it is registered. `attachOnly`: only
   * an engine open here already answers (the caller was not checked for loading one).
   */
  readonly open: (dir: string, relocate?: boolean, expect?: OpenExpect) => Effect.Effect<ProjectRef, FinError>;
  /** With `generation`, refused with `HISTORY_REWOUND` when the branch moved under the caller. */
  readonly commit: (projectId: string, tx: unknown, generation?: Generation, meta?: CallMeta) => Effect.Effect<CommittedTx, FinError>;
  /** `branch`: an id, a name or `<name>#<id>`; resolved to the id first. */
  readonly snapshot: (
    projectId: string,
    branch: string,
  ) => Effect.Effect<{ rev: number; project: unknown; generation?: Generation }, FinError>;
  /** Every branch (`all`: archived ones too). */
  readonly branches: (projectId: string, all: boolean) => Effect.Effect<{ branches: BranchView[] }, FinError>;
  /**
   * A branch or history write (`payload`: the RPC payload; the engine's request is the same
   * less `projectId`). Runs under the commit lock; afterwards every record of the result is
   * published to its own branch's hub, in rev order (nothing for a dry run).
   */
  readonly history: (projectId: string, op: HistoryOp, payload: HistoryPayload, meta?: CallMeta) => Effect.Effect<unknown, FinError>;
  /**
   * `branch` is resolved to its id first (hubs are keyed by id). Replays `rev > fromRev`, then follows live commits and imports: every rev exactly once, in
   * order. Ends with `HISTORY_REWOUND` once history it reported (or started from) is rewound,
   * at once when `generation` (what the caller read `fromRev` at) is not the branch's now,
   * and with `ENGINE_CLOSED` when the engine is evicted (subscribe again to reopen it).
   */
  readonly subscribe: (
    projectId: string,
    branch: string,
    fromRev: number,
    generation?: Generation,
  ) => Stream.Stream<CommittedTx, FinError>;
  /** The status now, then every status change and rewind/rebuild/background error. */
  readonly events: (projectId: string) => Stream.Stream<ProjectEvent, FinError>;
  readonly status: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly takeover: (projectId: string, confirm: boolean) => Effect.Effect<unknown, FinError>;
  readonly verify: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly rebuild: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly compact: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly txGet: (projectId: string, txId: string) => Effect.Effect<TxGetResult, FinError>;
  /** `meta`'s guard names the caller's credential: refused once it is revoked (review of 5A Tasks 8–9, L1). */
  readonly acknowledgeRewind: (projectId: string, branch: string, generation: number, meta?: CallMeta) => Effect.Effect<boolean, FinError>;
  readonly acknowledgeSalvage: (projectId: string, copy: string, meta?: CallMeta) => Effect.Effect<boolean, FinError>;
  /** `eval.frame`: `branch` resolved first; `timeline` absent: the main timeline. */
  readonly evalFrame: (projectId: string, branch: string, timeline: string | undefined, at: string) => Effect.Effect<FramePlan, FinError>;
  readonly evalAudio: (
    projectId: string,
    branch: string,
    timeline: string | undefined,
    from: string,
    to: string,
  ) => Effect.Effect<AudioPlan, FinError>;
  /** `fonts.plan`: the pins to prepend to a tx of `ops` on `branch` (nothing is committed). */
  readonly fontsPlan: (projectId: string, branch: string, ops: ReadonlyArray<unknown>) => Effect.Effect<FontsPlan, FinError>;
  /**
   * A `media.look` job's inputs (plan 6A Task 12): `snapshot.json`, `manifest.json` and, with
   * `paramsJson`, `params.json` written into `inputDir` (the job's existing `in/`), and the rev
   * snapshotted with the plan hash at every time. `branch` is resolved to its id first.
   */
  readonly prepareMediaJob: (
    projectId: string,
    branch: string,
    timeline: string,
    at: ReadonlyArray<string>,
    inputDir: string,
    fontCache: string | undefined,
    paramsJson: string | undefined,
  ) => Effect.Effect<MediaJobPrepared, FinError>;
  /** Fails with `PROJECT_NOT_FOUND` unless the project is known (opened on demand). */
  readonly ensure: (projectId: string) => Effect.Effect<void, FinError>;
  /** The chain's view (5A Task 5): open here now; its folder; a branch reference's id. */
  readonly isOpen: (projectId: string) => boolean;
  readonly dirOf: (projectId: string) => string | undefined;
  /** The folders of the projects open here now (a device's roots, 5B Decision 32). */
  readonly openDirs: () => ReadonlyArray<string>;
  /** The folder `app.db` records for a project (where it would be reopened from). */
  readonly indexedDirOf: (projectId: string) => string | undefined;
  readonly resolveBranch: (projectId: string, branch: string) => Effect.Effect<string, FinError>;
  /** @internal Tests only: how many live fan-out channels exist. */
  readonly hubCount: () => { ops: number; events: number };
  /** @internal Tests only: the engine currently registered for `projectId`. */
  readonly engineOf: (projectId: string) => Engine | undefined;
}

export class ProjectRegistry extends Context.Service<ProjectRegistry, ProjectRegistryShape>()(
  "finisherd/ProjectRegistry",
) {}

export interface RegistryOptions {
  /** Per-hub buffer; a subscriber that falls further behind is caught up from the op log. */
  readonly hubCapacity?: number;
  /** Holds the libraries holding these asset paths in use (5D Decision 28), and the project's media cache entries (6B-1), while it is open. */
  readonly libraryUse?: (assetPaths: ReadonlyArray<string>, projectId: string) => { release(): void };
}

/** Holds the libraries the project's assets (on `main`, at open) live in, until it closes. */
function holdLibraries(p: OpenProject, use: NonNullable<RegistryOptions["libraryUse"]>): void {
  void p.engine.snapshot(MAIN).then(
    (json) => {
      const assets = (JSON.parse(json) as { project?: { assets?: Record<string, { path?: unknown }> } }).project?.assets ?? {};
      const held = use(Object.values(assets).flatMap((a) => (typeof a.path === "string" ? [a.path] : [])), p.engine.projectId);
      if (p.release === undefined) p.release = () => held.release();
      else held.release();
    },
    () => undefined,
  );
}

export const DEFAULT_HUB_CAPACITY = 1024;
const MAIN = "main";

/** A rewound history, as the op stream reports it to a subscriber that saw it. */
function historyRewound(details: Record<string, unknown>): FinError {
  const branch = String(details.branch);
  const message =
    details.reason === undefined
      ? `the history of ${branch} from rev ${String(details.rev)} on was set aside`
      : details.reason === "GENERATION"
        ? `the history of ${branch} was rewound or rebuilt since the caller read it`
        : `the project database was rebuilt (${String(details.reason)}); revs read before may hold something else`;
  return finError("HISTORY_REWOUND", message, details, `read ${branch} again (snapshot.get) and subscribe from that rev`);
}

/** Ends the streams of an evicted engine: their clients reconnect, which reopens it. */
function engineEvicted(projectId: string): FinError {
  return finError(
    "ENGINE_CLOSED",
    "the project's engine stopped; its history may have changed since",
    { projectId, reason: "evicted" },
    "read the project again and subscribe again (that reopens it)",
  );
}


export function makeProjectRegistry(
  serverData: string,
  { hubCapacity = DEFAULT_HUB_CAPACITY, libraryUse }: RegistryOptions = {},
): Effect.Effect<ProjectRegistryShape, never, Scope.Scope> {
  return Effect.gen(function* () {
    const index = yield* Effect.acquireRelease(
      Effect.sync(() => new ProjectIndex(serverData)),
      (idx) => Effect.sync(() => idx.close()),
    );
    const byId = new Map<string, OpenProject>();
    const byDir = new Map<string, string>();
    /** In-flight opens, so concurrent `open(dir)` calls share one Engine. */
    const opening = new Map<string, Promise<OpenProject>>();
    /** In-flight creates by folder: creates of one folder run one after another. */
    const creating = new Map<string, Promise<OpenProject>>();
    /** Creates with an idempotency key this process made, by folder (`create`'s retries). */
    const created = new Map<string, { idemKey: string; name: string; rate: string; projectId: string }>();
    const hubs = new Hubs(hubCapacity);
    /** Set once shutdown starts: engines that finish opening later are closed at once. */
    let closed = false;

    yield* Effect.addFinalizer(() =>
      Effect.suspend(() => {
        closed = true;
        return Effect.forEach(
          [...byId.values()],
          (p) =>
            // Waiting for the commit lock means no commit is in flight when the engine closes.
            p.commitLock.withPermits(1)(
              Effect.promise(() => {
                p.release?.();
                p.release = () => {};
                return p.engine.close().catch(() => undefined);
              }),
            ),
          { discard: true },
        );
      }),
    );

    const shuttingDown = () => finError("SERVER_SHUTTING_DOWN", "finisherd is shutting down", {}, "retry once the server is back");

    /**
     * Routes one engine event: imports to the op hubs (deduped by rev against commits),
     * rewinds and rebuilds end the subscribers that saw the lost history, status and the rest
     * go to `project.events`; a fatal error evicts the engine. `ENGINE_BUSY` never arrives
     * here: it is a call's answer, passed through to the client.
     */
    const onEngineEvent = (p: OpenProject, json: string): void => {
      const id = p.engine.projectId;
      if (byId.get(id) !== p) return;
      let e: EngineEvent;
      try {
        e = JSON.parse(json) as EngineEvent;
      } catch {
        return;
      }
      switch (e.type) {
        case "applied":
          hubs.publishTx(id, e.tx);
          break;
        case "status":
          hubs.publishEvent(id, { type: "status", status: e.status });
          break;
        case "rewound": {
          const generation = { incarnation: e.incarnation, n: e.generation };
          hubs.rewind(id, e.branch, e.fromRev, historyRewound({ branch: e.branch, rev: e.fromRev, generation }), generation);
          hubs.publishEvent(id, e);
          break;
        }
        case "rebuilt":
          // `rev: 0` as on fin-store's own `HISTORY_REWOUND` (`REBUILT`): read from the start.
          hubs.endAll(
            id,
            1,
            (branch) => historyRewound({ branch, rev: 0, reason: "REBUILT", incarnation: e.incarnation }),
            { incarnation: e.incarnation },
          );
          hubs.publishEvent(id, e);
          break;
        case "lagged":
          // Events were dropped before registration (a rewind or rebuild among them, maybe).
          hubs.endAll(id, 1, (branch) => historyRewound({ branch, rev: 0, reason: "EVENTS_LAGGED" }));
          break;
        case "error":
          hubs.publishEvent(id, { type: "error", error: e.error, fatal: e.fatal });
          if (e.fatal) evict(p);
          break;
        case "collect": break;
      }
    };

    const register = (engine: Engine, dir: string): OpenProject => {
      if (closed) {
        void engine.close().catch(() => undefined);
        throw shuttingDown();
      }
      const existing = byId.get(engine.projectId);
      if (existing) {
        // Same project reached through another path string: keep the first engine.
        void engine.close().catch(() => undefined);
        byDir.set(dir, existing.engine.projectId);
        return existing;
      }
      const project: OpenProject = { engine, dir, commitLock: Semaphore.makeUnsafe(1) };
      byId.set(engine.projectId, project);
      byDir.set(dir, engine.projectId);
      index.touch(engine.projectId, dir);
      engine.onEvent((json) => onEngineEvent(project, json));
      if (libraryUse !== undefined) holdLibraries(project, libraryUse);
      return project;
    };

    /**
     * Forgets a dead engine so the next `get`/`open` reopens the project from disk. Its
     * subscribers end with `ENGINE_CLOSED`: nothing would feed them now, and the events it
     * still had queued (a rewind, maybe) are lost, so they are treated like a rebuild.
     */
    const evict = (p: OpenProject): void => {
      const id = p.engine.projectId;
      if (byId.get(id) !== p) return;
      byId.delete(id);
      for (const [dir, pid] of byDir) if (pid === id) byDir.delete(dir);
      hubs.endAll(id, 0, () => engineEvicted(id));
      hubs.endEvents(id, engineEvicted(id));
      p.release?.();
      p.release = () => {};
      void p.engine.close().catch(() => undefined);
    };

    /** An engine call on `p`; a panic or a dead engine evicts `p`. */
    const call = <A>(p: OpenProject, f: (engine: Engine) => Promise<string>): Effect.Effect<A, FinError> =>
      Effect.tapError(attemptJson<A>(() => f(p.engine)), (e) => Effect.sync(() => (isEngineDead(e) ? evict(p) : undefined)));

    const openDir = (dir: string, relocate: boolean, expected?: string): Promise<OpenProject> => {
      if (closed) return Promise.reject(shuttingDown());
      const id = byDir.get(dir);
      const known = id === undefined ? undefined : byId.get(id);
      if (known) {
        if (expected !== undefined && known.engine.projectId !== expected) return Promise.reject(changed(dir, expected));
        index.touch(known.engine.projectId, known.dir);
        return Promise.resolve(known);
      }
      let pending = opening.get(dir);
      if (!pending) {
        // The store checks the folder still holds `expected` before it locks or leases anything.
        pending = Engine.open(serverData, dir, { relocate, ...(expected === undefined ? {} : { expectId: expected }) })
          .then((engine) => {
            // And again before `register`: a project other than the one checked is never taken on.
            if (expected !== undefined && engine.projectId !== expected) {
              void engine.close().catch(() => undefined);
              throw changed(dir, expected);
            }
            return register(engine, dir);
          })
          .finally(() => opening.delete(dir));
        opening.set(dir, pending);
      }
      // A caller joining another's open checks its own expectation.
      return pending.then((p) => {
        if (expected !== undefined && p.engine.projectId !== expected) throw changed(dir, expected);
        return p;
      });
    };

    const ref = (p: OpenProject): Effect.Effect<ProjectRef, FinError> =>
      Effect.map(
        call<number>(p, async (e) => String(await e.head(MAIN))),
        (rev) => ({ projectId: p.engine.projectId, rev }),
      );

    /** An open project by id; after a restart or eviction it is reopened from the `app.db` index. */
    const get = (projectId: string): Effect.Effect<OpenProject, FinError> =>
      Effect.suspend(() => {
        const open = byId.get(projectId);
        if (open) return Effect.succeed(open);
        if (closed) return Effect.fail(shuttingDown());
        const dir = index.dirOf(projectId);
        if (dir === undefined) {
          return Effect.fail(
            finError("PROJECT_NOT_FOUND", `no project with id ${projectId}`, { projectId }, "open the project by its directory first"),
          );
        }
        return Effect.flatMap(attempt(() => openDir(dir, false)), (p) =>
          p.engine.projectId === projectId
            ? Effect.succeed(p)
            : Effect.fail(
                finError("PROJECT_NOT_FOUND", `project ${projectId} is no longer at ${dir}`, { projectId, dir }, "open the project by its directory"),
              ),
        );
      });

    /** `f` on the engine of `projectId`, its JSON answer parsed. */
    const on = <A>(projectId: string, f: (engine: Engine) => Promise<string>): Effect.Effect<A, FinError> =>
      Effect.flatMap(get(projectId), (p) => call<A>(p, f));

    /** The id a branch reference names (`BRANCH_NOT_FOUND` when none does); `main` needs no call. */
    const resolveBranch = (projectId: string, branch: string): Effect.Effect<string, FinError> =>
      branch === MAIN ? Effect.succeed(MAIN) : on<string>(projectId, (e) => e.resolveBranch(branch));

    const opsSince = (projectId: string, branch: string, rev: number) =>
      on<CommittedTx[]>(projectId, (e) => e.opsSince(branch, rev));

    return {
      create: (dir, name, rate, idemKey) =>
        Effect.suspend(() => {
          if (closed) return Effect.fail(shuttingDown());
          const abs = resolve(dir);
          const once = async (): Promise<OpenProject> => {
            // A retry of a create this process made, its engine still open: that engine is the
            // answer (no second engine opened). Anything else goes to fin-store, which opens
            // the project for a matching key or refuses (PROJECT_EXISTS, IDEM_KEY_REUSED).
            const made = created.get(abs);
            const open = made === undefined ? undefined : byId.get(made.projectId);
            if (open && idemKey !== undefined && made?.idemKey === idemKey && made.name === name && made.rate === rate) {
              return open;
            }
            const p = register(await Engine.create(serverData, abs, name, rate, { idemKey }), abs);
            if (idemKey !== undefined) created.set(abs, { idemKey, name, rate, projectId: p.engine.projectId });
            return p;
          };
          // One create per folder at a time, so a concurrent retry finds the first one's engine.
          const next = (creating.get(abs) ?? Promise.resolve()).catch(() => undefined).then(once);
          creating.set(abs, next);
          next
            .finally(() => {
              if (creating.get(abs) === next) creating.delete(abs);
            })
            .catch(() => undefined);
          return Effect.flatMap(
            attempt(() => next),
            ref,
          );
        }),

      open: (dir, relocate = false, expect = {}) =>
        Effect.suspend(() => {
          if (expect.attachOnly === true && expect.projectId !== undefined) {
            const p = byId.get(expect.projectId);
            if (p === undefined) {
              const fix = "open the project by its folder again";
              return Effect.fail(finError("PROJECT_NOT_FOUND", `project ${expect.projectId} is not open here`, { projectId: expect.projectId, dir }, fix));
            }
            return ref(p);
          }
          return Effect.flatMap(attempt(() => openDir(resolve(dir), relocate, expect.projectId)), ref);
        }),

      commit: (projectId, tx, generation, meta) =>
        Effect.flatMap(get(projectId), (p) =>
          p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<CommittedTx>(p, (e) =>
                  e.commit(JSON.stringify(tx), generation === undefined ? undefined : JSON.stringify(generation), metaJson(meta)),
                ),
                (committed) => Effect.sync(() => hubs.publishTx(projectId, committed)),
              ),
            ),
          ),
        ),

      snapshot: (projectId, requested) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) => on(projectId, (e) => e.snapshot(branch))),

      branches: (projectId, all) =>
        Effect.map(on<BranchView[]>(projectId, (e) => e.branches(all)), (branches) => ({ branches })),

      history: (projectId, op, payload, meta) =>
        Effect.flatMap(get(projectId), (p) =>
          p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<unknown>(p, (e) => callEngine(e, op, payload, metaJson(meta))),
                (result) => Effect.sync(() => publishResult(hubs, projectId, result, payload.dryRun === true)),
              ),
            ),
          ),
        ),

      subscribe: (projectId, requested, fromRev, generation) =>
        Stream.unwrap(
          Effect.gen(function* () {
            // Hubs are keyed by id: a name, or `<name>#<id>`, resolves before anything else.
            const branch = yield* resolveBranch(projectId, requested);
            const channel = yield* Effect.acquireRelease(
              Effect.sync(() => hubs.acquireOps(projectId, branch)),
              () => Effect.sync(() => hubs.releaseOps(projectId, branch)),
            );
            // Before subscribing: an ending published from now on is seen, as its marker or
            // through the `seq` of any later item. One whose engine event lands after the
            // backlog below was read at its new generation is skipped (`check` gets that
            // generation): the subscriber already reads the new history.
            const check = follower(channel, branch);
            // Subscribe before reading the backlog: a commit landing in between is then
            // both in the backlog and on the hub, and the rev filter below drops the copy.
            const live = yield* PubSub.subscribe(channel.pubsub);
            // Every engine call re-resolves the engine (`on`): an eviction between calls must
            // not leave this stream talking to a closed engine. The head is read before the
            // backlog; a rewind landing in between is caught by the generation check below.
            const head = yield* on<number>(projectId, async (e) => String(await e.head(branch)));
            const read = yield* on<{ generation: Generation; ops: CommittedTx[] }>(projectId, (e) => e.opsSinceAt(branch, fromRev));
            if (
              generation !== undefined &&
              (generation.incarnation !== read.generation.incarnation || generation.n !== read.generation.n)
            ) {
              return yield* Effect.fail(
                historyRewound({ branch, rev: 0, reason: "GENERATION", generation: read.generation, expected: generation }),
              );
            }
            if (fromRev > head) {
              return yield* Effect.fail(
                finError(
                  "BASE_REV_AHEAD",
                  `fromRev ${fromRev} is ahead of the head of ${branch} (rev ${head})`,
                  { branch, fromRev, head },
                  `subscribe from a rev at most ${head} (snapshot.get reports the head)`,
                ),
              );
            }
            let last = fromRev;
            const emit = (item: Stamped<CommittedTx>): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> => {
              const ended = check(item, last, read.generation);
              if (ended !== undefined) return Effect.fail(ended);
              const c = item.value;
              if (c === undefined || c.rev <= last) return Effect.succeed([]);
              if (c.rev === last + 1) {
                last = c.rev;
                return Effect.succeed([c]);
              }
              // A gap: the sliding hub dropped items for this slow subscriber. Fill from the log.
              return Effect.map(opsSince(projectId, branch, last), (missing) => {
                const out = missing.filter((m) => m.rev > last && m.rev <= c.rev);
                if (out.length > 0) last = out[out.length - 1]!.rev;
                return out;
              });
            };
            const backlog = read.ops.map((tx): Stamped<CommittedTx> => ({ seq: -1, value: tx }));
            return Stream.concat(Stream.fromIterable(backlog), Stream.fromSubscription(live)).pipe(
              Stream.mapEffect(emit),
              Stream.flattenIterable,
            );
          }),
        ),

      events: (projectId) =>
        Stream.unwrap(
          Effect.gen(function* () {
            const channel = yield* Effect.acquireRelease(
              Effect.sync(() => hubs.acquireEvents(projectId)),
              () => Effect.sync(() => hubs.releaseEvents(projectId)),
            );
            const check = follower(channel, "");
            // Subscribe first: a change landing while the status is read is then not missed.
            const live = yield* PubSub.subscribe(channel.pubsub);
            const status = yield* on<unknown>(projectId, (e) => e.status());
            const first: ProjectEvent = { type: "status", status };
            const items = Stream.fromSubscription(live).pipe(
              Stream.mapEffect((item) => {
                const ended = check(item, 0);
                if (ended !== undefined) return Effect.fail(ended);
                return Effect.succeed(item.value === undefined ? [] : [item.value]);
              }),
              Stream.flattenIterable,
            );
            return Stream.concat(Stream.make(first), items);
          }),
        ),
      status: (projectId) => on(projectId, (e) => e.status()),
      takeover: (projectId, confirm) => on(projectId, (e) => e.takeover(confirm)),
      verify: (projectId) => on(projectId, (e) => e.verify()),
      rebuild: (projectId) => on(projectId, (e) => e.rebuild()),
      compact: (projectId) => on(projectId, (e) => e.compact()),
      txGet: (projectId, txId) => on<TxGetResult>(projectId, async (e) => (await e.txGet(txId)) ?? "null"),
      acknowledgeRewind: (projectId, requested, generation, meta) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<boolean>(projectId, async (e) => String(await e.acknowledgeRewind(branch, generation, metaJson(meta)))),
        ),
      acknowledgeSalvage: (projectId, copy, meta) => on<boolean>(projectId, async (e) => String(await e.acknowledgeSalvage(copy, metaJson(meta)))),
      ensure: (projectId) => Effect.asVoid(get(projectId)),
      isOpen: (projectId) => byId.has(projectId),
      dirOf: (projectId) => byId.get(projectId)?.dir,
      openDirs: () => [...byId.values()].map((p) => p.dir),
      indexedDirOf: (projectId) => (closed ? undefined : index.dirOf(projectId)),
      resolveBranch,
      evalFrame: (projectId, requested, timeline, at) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<FramePlan>(projectId, (e) => e.evalFrame(branch, timeline, at)),
        ),
      evalAudio: (projectId, requested, timeline, from, to) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<AudioPlan>(projectId, (e) => e.evalAudio(branch, timeline, from, to)),
        ),
      fontsPlan: (projectId, requested, ops) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<FontsPlan>(projectId, (e) => e.fontsPlan(branch, JSON.stringify(ops))),
        ),

      prepareMediaJob: (projectId, requested, timeline, at, inputDir, fontCache, paramsJson) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<MediaJobPrepared>(projectId, (e) => e.prepareMediaJob(branch, timeline, [...at], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null })),
        ),

      hubCount: () => hubs.size,
      engineOf: (projectId) => byId.get(projectId)?.engine,
    } satisfies ProjectRegistryShape;
  });
}

export function layerProjectRegistry(serverData: string, options?: RegistryOptions): Layer.Layer<ProjectRegistry> {
  return Layer.effect(ProjectRegistry, makeProjectRegistry(serverData, options));
}
