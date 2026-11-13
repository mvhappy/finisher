// ProjectRegistry: open Engines, the `app.db` project index, per-(project, branch) op hubs and
// per-project event hubs fed by each engine's `onEvent`.
import { resolve } from "node:path";
import type { AudioPlan, BranchView, CommittedTx, CompactReport, FontsPlan, FramePlan, ProjectStatusView, RebuildReport, VerifyReport } from "@finisher/core-types";
import type { FinError, Generation, HistoryLogResult, ProjectEvent, TxGetResult } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Context, Effect, Layer, type Scope, Semaphore, type Stream } from "effect";
import { attempt, attemptJson, finError, isEngineDead } from "./errors.ts";
import type { CallMeta } from "./diagnostics.ts";
import { callEngine, type HistoryOp, type HistoryPayload, type HistoryResults, metaJson, publishResult } from "./history.ts";
import { Hubs } from "./hubs.ts";
import { routeEngineEvent } from "./engine-events.ts";
import { engineEvicted, opStream } from "./op-stream.ts";
import { projectIdIn } from "./chain/resolve.ts";
import { holdLibraries, movedFrom, type OpenProject } from "./open-project.ts";
import { projectEvents, type StatusAt } from "./project-events.ts";
import { ProjectIndex } from "./project-index.ts";
import { branchReads } from "./branch-reads.ts";

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
  readonly history: <O extends HistoryOp>(projectId: string, op: O, payload: HistoryPayload, meta?: CallMeta) => Effect.Effect<HistoryResults[O], FinError>;
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
  /**
   * `project.close`: ends its subscribers with `ENGINE_CLOSED {reason: closed}`, waits for the
   * commit lock and closes the engine (in-flight calls finish; the export is flushed and the
   * lease released). `false` when it was not open. The next call reopens it.
   */
  readonly close: (projectId: string) => Effect.Effect<{ closed: boolean }, FinError>;
  /** The status now, then every status change and rewind/rebuild/background error. */
  readonly events: (projectId: string) => Stream.Stream<ProjectEvent, FinError>;
  readonly status: (projectId: string) => Effect.Effect<ProjectStatusView, FinError>;
  readonly takeover: (projectId: string, confirm: boolean) => Effect.Effect<ProjectStatusView, FinError>;
  readonly verify: (projectId: string) => Effect.Effect<VerifyReport, FinError>;
  readonly rebuild: (projectId: string) => Effect.Effect<RebuildReport, FinError>;
  readonly compact: (projectId: string) => Effect.Effect<CompactReport, FinError>;
  readonly txGet: (projectId: string, txId: string) => Effect.Effect<TxGetResult, FinError>;
  /** `history.log`: the head and the last `limit` records of `branch` below `before`, oldest first. */
  readonly historyLog: (projectId: string, branch: string, before: number | undefined, limit: number) => Effect.Effect<HistoryLogResult, FinError>;
  /** `meta`'s guard names the caller's credential: refused once it is revoked (review of 5A Tasks 8–9, L1). */
  readonly acknowledgeRewind: (projectId: string, branch: string, generation: number, meta?: CallMeta) => Effect.Effect<boolean, FinError>;
  readonly acknowledgeSalvage: (projectId: string, copy: string, meta?: CallMeta) => Effect.Effect<boolean, FinError>;
  /** `eval.frame`: `timeline` absent: the main timeline; one gone since `rev` is `CONFLICT` (`TIMELINE_GONE`). */
  readonly evalFrame: (projectId: string, branch: string, timeline: string | undefined, at: string, rev?: number) => Effect.Effect<FramePlan, FinError>;
  readonly evalAudio: (
    projectId: string,
    branch: string,
    timeline: string | undefined,
    from: string,
    to: string,
    rev?: number,
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
  /** `f` on the project's engine (opened on demand) with `branch` resolved to its id; `f` resolves to JSON (6B-1 Task 11: `prepareAssetJob`). */
  readonly onBranch: <A>(projectId: string, branch: string, f: (engine: Engine, branch: string) => Promise<string>) => Effect.Effect<A, FinError>;
  /** Fails with `PROJECT_NOT_FOUND` unless the project is known (opened on demand). */
  readonly ensure: (projectId: string) => Effect.Effect<void, FinError>;
  /** The chain's view (5A Task 5): open here now; its folder; a branch reference's id. A project closed idle or by `project.close` still counts as open here, with its folder, until it reopens
   * (reopening on demand is the registry's housekeeping: no `project.write`; a device's roots and an agent's or a job's folder stay as they were). */
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
  /**
   * An engine with no subscriber and no call in flight (no commit lock held or awaited, no long
   * call) for this long is closed (owner decision on review #11); the next call reopens it.
   * Default `DEFAULT_IDLE_CLOSE_MS`; `0` never closes one.
   */
  readonly idleCloseMs?: number;
  readonly onOpened?: (projectId: string) => void; // 5E Decision 22: a project was loaded (re-derived reviews)
  /** A transaction was committed through `commit` (6B-1 Task 14: assets used for the first time get a 1080 proxy). */
  readonly onCommitted?: (projectId: string, committed: CommittedTx) => void;
}

/** 10 minutes. */
export const DEFAULT_IDLE_CLOSE_MS = 10 * 60_000;

export const DEFAULT_HUB_CAPACITY = 1024;
const MAIN = "main";

export function makeProjectRegistry(
  serverData: string,
  { hubCapacity = DEFAULT_HUB_CAPACITY, libraryUse, idleCloseMs = DEFAULT_IDLE_CLOSE_MS, onOpened, onCommitted }: RegistryOptions = {},
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
    /** Engines still closing, by folder: an open of the folder waits for its old engine to let go. */
    const closingDirs = new Map<string, Promise<void>>();
    /** Projects the registry closed (idle, `project.close`) and their folders, until they reopen. */
    const closedHere = new Map<string, string>();
    /** Set once shutdown starts: engines that finish opening later are closed at once. */
    let closed = false;
    /** Relocations under way, by project (`relocateOnce`). */
    const relocating = new Map<string, Promise<OpenProject>>();
    /** The last engine's `epoch`. */
    let epochs = 0;

    yield* Effect.addFinalizer(() =>
      Effect.suspend(() => {
        closed = true;
        const closes = Effect.forEach(
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
        // And the closes already under way (an idle close, `project.close`, an eviction).
        return Effect.andThen(closes, Effect.promise(() => Promise.all([...closingDirs.values()])));
      }),
    );

    const shuttingDown = () => finError("SERVER_SHUTTING_DOWN", "finisherd is shutting down", {}, "retry once the server is back");

    const onEngineEvent = (p: OpenProject, json: string, seq: number): void => {
      if (byId.get(p.engine.projectId) === p) routeEngineEvent(hubs, p, json, seq, evict);
    };

    const register = (engine: Engine, dir: string): OpenProject => {
      if (closed) {
        void engine.close().catch(() => undefined);
        throw shuttingDown();
      }
      const existing = byId.get(engine.projectId);
      if (existing) {
        // Same project reached through another path string (a link): keep the first engine; the
        // second one only read (LEASE_HELD). A move was handled before opening (`relocation`).
        void engine.close().catch(() => undefined);
        byDir.set(dir, existing.engine.projectId);
        return existing;
      }
      epochs += 1;
      const project: OpenProject = { engine, epoch: epochs, dir, commitLock: Semaphore.makeUnsafe(1), active: 0, lastUsed: Date.now(), retired: false };
      byId.set(engine.projectId, project);
      closedHere.delete(engine.projectId);
      byDir.set(dir, engine.projectId);
      index.touch(engine.projectId, dir);
      engine.onEvent((json, seq) => onEngineEvent(project, json, seq));
      if (libraryUse !== undefined) holdLibraries(project, libraryUse);
      if (onOpened !== undefined) queueMicrotask(() => onOpened(engine.projectId));
      return project;
    };

    /**
     * Forgets a dead engine so the next `get`/`open` reopens the project from disk. Its
     * subscribers end with `ENGINE_CLOSED`: nothing would feed them now, and the events it
     * still had queued (a rewind, maybe) are lost, so they are treated like a rebuild.
     */
    const evict = (p: OpenProject): void => {
      void forget(p, "evicted").catch(() => undefined);
    };

    /**
     * Unregisters `p`, ends its streams and closes its engine (after its in-flight calls).
     * `moved`: its folder is elsewhere now and reopens there at once, so its streams go on.
     */
    const forget = (p: OpenProject, reason: "evicted" | "closed" | "moved"): Promise<void> => {
      const id = p.engine.projectId;
      if (byId.get(id) !== p) return Promise.resolve();
      byId.delete(id);
      p.retired = reason !== "evicted";
      if (reason === "closed") closedHere.set(id, p.dir);
      const dirs = new Set([p.dir]);
      for (const [dir, pid] of byDir) {
        if (pid !== id) continue;
        byDir.delete(dir);
        dirs.add(dir);
      }
      if (reason !== "moved") {
        hubs.endAll(id, 0, () => engineEvicted(id, reason));
        hubs.endEvents(id, engineEvicted(id, reason));
      }
      p.release?.();
      p.release = () => {};
      const closing = p.engine.close();
      const settled = closing.then(
        () => undefined,
        () => undefined,
      );
      for (const dir of dirs) closingDirs.set(dir, settled);
      void settled.then(() => {
        for (const dir of dirs) if (closingDirs.get(dir) === settled) closingDirs.delete(dir);
      });
      return closing;
    };

    /** `project.close` and the idle close: under the commit lock, so no commit is in flight. */
    const closeProject = (p: OpenProject, onlyIfIdle: boolean): Effect.Effect<boolean, FinError> =>
      p.commitLock.withPermits(1)(
        Effect.suspend(() => {
          if (byId.get(p.engine.projectId) !== p) return Effect.succeed(false);
          if (onlyIfIdle && !idle(p, Date.now())) return Effect.succeed(false);
          return Effect.as(attempt(() => forget(p, "closed")), true);
        }),
      );

    const idle = (p: OpenProject, now: number): boolean =>
      p.active === 0 && !hubs.holds(p.engine.projectId) && now - p.lastUsed >= idleCloseMs;

    /** Marks `p` used now, e.g. when its last subscriber left. */
    const touch = (projectId: string): void => {
      const p = byId.get(projectId);
      if (p !== undefined) p.lastUsed = Date.now();
    };

    if (idleCloseMs > 0) {
      const sweep = setInterval(() => {
        const now = Date.now();
        for (const p of byId.values()) if (idle(p, now)) Effect.runFork(Effect.ignore(closeProject(p, true)));
      }, Math.min(idleCloseMs, 60_000));
      sweep.unref();
      yield* Effect.addFinalizer(() => Effect.sync(() => clearInterval(sweep)));
    }

    /** `effect` counted as a call in flight on `p` (the idle close waits for it). */
    const using = <A>(p: OpenProject, effect: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
      Effect.ensuring(
        Effect.andThen(Effect.sync(() => void (p.active += 1)), effect),
        Effect.sync(() => void ((p.active -= 1), (p.lastUsed = Date.now()))),
      );

    /** An engine call on `p`; a panic or a dead engine evicts `p`. */
    const call = <A>(p: OpenProject, f: (engine: Engine) => Promise<string>): Effect.Effect<A, FinError> =>
      using(
        p,
        Effect.tapError(attemptJson<A>(() => f(p.engine)), (e) => Effect.sync(() => (isEngineDead(e) ? evict(p) : undefined))),
      );

    /** What a closed engine answers every call (`ENGINE_CLOSED {reason: closed}`). */
    const closedUnder = (e: FinError): boolean =>
      e.code === "ENGINE_CLOSED" && (e.details as { reason?: unknown } | null | undefined)?.reason === "closed";

    /**
     * `f` on the open project; when the registry closed the engine under it (between `get` and
     * the call: a closed engine refuses a call before doing anything), once more on a reopened one.
     */
    const withOpen = <A>(projectId: string, f: (p: OpenProject) => Effect.Effect<A, FinError>): Effect.Effect<A, FinError> =>
      Effect.flatMap(get(projectId), (p) =>
        Effect.catchIf(f(p), (e) => p.retired && closedUnder(e), () => Effect.flatMap(get(projectId), f)),
      );

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
        // The store checks the folder still holds `expected` before it locks or leases anything;
        // a closing engine of the folder lets go of its lease and database first.
        pending = (closingDirs.get(dir) ?? Promise.resolve())
          .then(() => relocation(dir, relocate, expected))
          .then(async (moved) => {
            if (moved !== undefined) return moved;
            const engine = await Engine.open(serverData, dir, { relocate, ...(expected === undefined ? {} : { expectId: expected }) });
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

    /**
     * The project `dir` holds, when it is open here at another folder it was moved from (review
     * #32), taken on at `dir`; `undefined` otherwise. Decided before any engine opens at `dir`:
     * a second engine of the project, open while the first lets go, would take its lease. One
     * relocation per project at a time: another path to the new folder joins it.
     */
    const relocation = async (dir: string, relocate: boolean, expected: string | undefined): Promise<OpenProject | undefined> => {
      const id = await projectIdIn(dir);
      if (id === undefined || (expected !== undefined && id !== expected)) return undefined;
      const old = byId.get(id);
      if (relocating.has(id) || old === undefined) return relocating.get(id);
      if (!(await movedFrom(old.dir, dir, id))) return relocating.get(id);
      if (relocating.has(id) || byId.get(id) !== old) return relocating.get(id);
      const next = relocated(old, dir, relocate).finally(() => relocating.delete(id));
      relocating.set(id, next);
      return next;
    };

    /**
     * The old folder's engine has lost its lease (its check reads the old path). The index
     * records `dir` first (a call in between waits for this open), the old engine closes after
     * its in-flight calls (streams go on: same project, same history) and the project reopens
     * at `dir`, writable. A reopen that
     * fails ends the streams (`ENGINE_CLOSED {reason: closed}`) and leaves the project closed
     * here, at `dir`: the next call reopens it.
     */
    const relocated = async (old: OpenProject, dir: string, relocate: boolean): Promise<OpenProject> => {
      const id = old.engine.projectId;
      index.touch(id, dir);
      for (const [folder, made] of created) if (made.projectId === id) created.delete(folder);
      await Effect.runPromise(old.commitLock.withPermits(1)(Effect.promise(() => forget(old, "moved").catch(() => undefined))));
      // An eviction that raced this one closes without the lock: its lease must be let go too.
      await (closingDirs.get(old.dir) ?? Promise.resolve());
      try {
        if (closed) throw shuttingDown();
        return register(await Engine.open(serverData, dir, { relocate, expectId: id }), dir);
      } catch (e) {
        if (!closed && !byId.has(id)) {
          closedHere.set(id, dir);
          hubs.endAll(id, 0, () => engineEvicted(id, "closed"));
          hubs.endEvents(id, engineEvicted(id, "closed"));
        }
        throw e;
      }
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
      withOpen(projectId, (p) => call<A>(p, f));

    /** The status and the last event it covers, from the engine that read it (`project.events`). */
    const statusAt = (projectId: string): Effect.Effect<StatusAt, FinError> =>
      withOpen(projectId, (p) =>
        Effect.map(call<{ status: ProjectStatusView; seq: number }>(p, (e) => e.statusAt()), (r) => ({ status: r.status, seq: r.seq, source: p.epoch })),
      );

    /** The id a branch reference names (`BRANCH_NOT_FOUND` when none does); `main` needs no call. */
    const resolveBranch = (projectId: string, branch: string): Effect.Effect<string, FinError> =>
      branch === MAIN ? Effect.succeed(MAIN) : on<string>(projectId, (e) => e.resolveBranch(branch));

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
            // Closed here by the registry: attaching reopens it, as any call would.
            if (p === undefined && closedHere.has(expect.projectId)) return Effect.flatMap(get(expect.projectId), ref);
            if (p === undefined) {
              const fix = "open the project by its folder again";
              return Effect.fail(finError("PROJECT_NOT_FOUND", `project ${expect.projectId} is not open here`, { projectId: expect.projectId, dir }, fix));
            }
            return ref(p);
          }
          return Effect.flatMap(attempt(() => openDir(resolve(dir), relocate, expect.projectId)), ref);
        }),

      commit: (projectId, tx, generation, meta) =>
        withOpen(projectId, (p) =>
          using(p, p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<CommittedTx>(p, (e) =>
                  e.commit(JSON.stringify(tx), generation === undefined ? undefined : JSON.stringify(generation), metaJson(meta)),
                ),
                (committed) =>
                  Effect.sync(() => {
                    hubs.publishTx(projectId, committed);
                    onCommitted?.(projectId, committed);
                  }),
              ),
            ),
          )),
        ),

      snapshot: (projectId, requested) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) => on(projectId, (e) => e.snapshot(branch))),

      branches: (projectId, all) =>
        Effect.map(on<BranchView[]>(projectId, (e) => e.branches(all)), (branches) => ({ branches })),

      history: <O extends HistoryOp>(projectId: string, op: O, payload: HistoryPayload, meta?: CallMeta) =>
        withOpen(projectId, (p) =>
          using(p, p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<HistoryResults[O]>(p, (e) => callEngine(e, op, payload, metaJson(meta))),
                (result) => Effect.sync(() => publishResult(hubs, projectId, result, payload.dryRun === true)),
              ),
            ),
          )),
        ),

      subscribe: (projectId, requested, fromRev, generation) =>
        opStream({ hubs, resolveBranch, on, released: () => touch(projectId) }, projectId, requested, fromRev, generation),

      events: (projectId) => projectEvents({ hubs, capacity: hubCapacity, statusAt, released: () => touch(projectId) }, projectId),
      close: (projectId) =>
        Effect.suspend(() => {
          const p = byId.get(projectId);
          return Effect.map(p === undefined ? Effect.succeed(false) : closeProject(p, false), (closed) => ({ closed }));
        }),
      status: (projectId) => on<ProjectStatusView>(projectId, (e) => e.status()),
      takeover: (projectId, confirm) => on<ProjectStatusView>(projectId, (e) => e.takeover(confirm)),
      verify: (projectId) => on<VerifyReport>(projectId, (e) => e.verify()),
      rebuild: (projectId) => on<RebuildReport>(projectId, (e) => e.rebuild()),
      compact: (projectId) => on<CompactReport>(projectId, (e) => e.compact()),
      txGet: (projectId, txId) => on<TxGetResult>(projectId, async (e) => (await e.txGet(txId)) ?? "null"),
      acknowledgeRewind: (projectId, requested, generation, meta) =>
        Effect.flatMap(resolveBranch(projectId, requested), (branch) =>
          on<boolean>(projectId, async (e) => String(await e.acknowledgeRewind(branch, generation, metaJson(meta)))),
        ),
      acknowledgeSalvage: (projectId, copy, meta) => on<boolean>(projectId, async (e) => String(await e.acknowledgeSalvage(copy, metaJson(meta)))),
      ensure: (projectId) => Effect.asVoid(get(projectId)),
      isOpen: (projectId) => byId.has(projectId) || closedHere.has(projectId),
      dirOf: (projectId) => byId.get(projectId)?.dir ?? closedHere.get(projectId),
      openDirs: () => [...byId.values()].map((p) => p.dir).concat([...closedHere.values()]),
      indexedDirOf: (projectId) => (closed ? undefined : index.dirOf(projectId)),
      resolveBranch,
      ...branchReads(resolveBranch, on),

      hubCount: () => hubs.size,
      engineOf: (projectId) => byId.get(projectId)?.engine,
    } satisfies ProjectRegistryShape;
  });
}

export function layerProjectRegistry(serverData: string, options?: RegistryOptions): Layer.Layer<ProjectRegistry> {
  return Layer.effect(ProjectRegistry, makeProjectRegistry(serverData, options));
}
