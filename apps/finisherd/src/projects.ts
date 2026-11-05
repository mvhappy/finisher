// ProjectRegistry: open Engines, the `app.db` project index, per-(project, branch) op hubs and
// per-project event hubs fed by each engine's `onEvent`.
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { CommittedTx } from "@finisher/core-types";
import type { FinError, Generation, ProjectEvent, TxGetResult } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Context, Effect, Layer, PubSub, type Scope, Semaphore, Stream } from "effect";
import { attempt, attemptJson, finError, isEngineDead } from "./errors.ts";
import { type HubItem, Hubs } from "./hubs.ts";

export interface ProjectRef {
  readonly projectId: string;
  readonly rev: number;
}

/** One open project: its engine and a commit lock. */
interface OpenProject {
  readonly engine: Engine;
  readonly dir: string;
  /** Serializes commit + publish so hubs see revs in commit order. */
  readonly commitLock: Semaphore.Semaphore;
}

export interface ProjectRegistryShape {
  readonly create: (dir: string, name: string, rate: string) => Effect.Effect<ProjectRef, FinError>;
  readonly open: (dir: string, relocate?: boolean) => Effect.Effect<ProjectRef, FinError>;
  /** With `generation`, refused with `HISTORY_REWOUND` when the branch moved under the caller. */
  readonly commit: (projectId: string, tx: unknown, generation?: Generation) => Effect.Effect<CommittedTx, FinError>;
  readonly snapshot: (
    projectId: string,
    branch: string,
  ) => Effect.Effect<{ rev: number; project: unknown; generation?: Generation }, FinError>;
  /**
   * Replays `rev > fromRev`, then follows live commits and imports: every rev exactly once, in
   * order. Ends with `HISTORY_REWOUND` once history it reported (or started from) is rewound.
   */
  readonly subscribe: (projectId: string, branch: string, fromRev: number) => Stream.Stream<CommittedTx, FinError>;
  /** The status now, then every status change and rewind/rebuild/background error. */
  readonly events: (projectId: string) => Stream.Stream<ProjectEvent, FinError>;
  readonly status: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly takeover: (projectId: string, confirm: boolean) => Effect.Effect<unknown, FinError>;
  readonly verify: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly rebuild: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly compact: (projectId: string) => Effect.Effect<unknown, FinError>;
  readonly txGet: (projectId: string, txId: string) => Effect.Effect<TxGetResult, FinError>;
  readonly acknowledgeRewind: (projectId: string, branch: string, generation: number) => Effect.Effect<boolean, FinError>;
  readonly acknowledgeSalvage: (projectId: string, copy: string) => Effect.Effect<boolean, FinError>;
  /** Fails with `PROJECT_NOT_FOUND` unless the project is known (opened on demand). */
  readonly ensure: (projectId: string) => Effect.Effect<void, FinError>;
  /** @internal Tests only: the engine currently registered for `projectId`. */
  readonly engineOf: (projectId: string) => Engine | undefined;
}

export class ProjectRegistry extends Context.Service<ProjectRegistry, ProjectRegistryShape>()(
  "finisherd/ProjectRegistry",
) {}

export interface RegistryOptions {
  /** Per-hub buffer; a subscriber that falls further behind is caught up from the op log. */
  readonly hubCapacity?: number;
}

export const DEFAULT_HUB_CAPACITY = 1024;
const MAIN = "main";

/** `<server-data>/app.db`: `projects(id, dir, last_opened)`. */
class ProjectIndex {
  readonly #db: DatabaseSync;

  constructor(serverData: string) {
    mkdirSync(serverData, { recursive: true });
    this.#db = new DatabaseSync(join(serverData, "app.db"));
    this.#db.exec(
      "CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, dir TEXT NOT NULL, last_opened INTEGER NOT NULL)",
    );
  }

  touch(id: string, dir: string): void {
    this.#db
      .prepare(
        "INSERT INTO projects (id, dir, last_opened) VALUES (?, ?, ?) " +
          "ON CONFLICT(id) DO UPDATE SET dir = excluded.dir, last_opened = excluded.last_opened",
      )
      .run(id, dir, Date.now());
  }

  dirOf(id: string): string | undefined {
    const row = this.#db.prepare("SELECT dir FROM projects WHERE id = ?").get(id) as { dir?: string } | undefined;
    return row?.dir;
  }

  close(): void {
    this.#db.close();
  }
}

/** A rewound history, as the op stream reports it to a subscriber that saw it. */
function historyRewound(details: Readonly<Record<string, unknown>>): FinError {
  const branch = String(details.branch);
  return finError(
    "HISTORY_REWOUND",
    details.reason === undefined
      ? `the history of ${branch} from rev ${String(details.rev)} on was set aside`
      : `the project database was rebuilt (${String(details.reason)}); revs read before may hold something else`,
    details,
    `read ${branch} again (snapshot.get) and subscribe from that rev`,
  );
}

type EngineEvent =
  | { type: "applied"; tx: CommittedTx }
  | { type: "status"; status: unknown }
  | { type: "rewound"; branch: string; fromRev: number; quarantined: number; own: boolean; generation: number; incarnation: string }
  | { type: "rebuilt"; incarnation: string }
  | { type: "error"; error: unknown; fatal: boolean }
  | { type: "lagged"; dropped: number };

export function makeProjectRegistry(
  serverData: string,
  { hubCapacity = DEFAULT_HUB_CAPACITY }: RegistryOptions = {},
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
            p.commitLock.withPermits(1)(Effect.promise(() => p.engine.close().catch(() => undefined))),
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
        case "rewound":
          hubs.rewind(id, e.branch, e.fromRev, { generation: { incarnation: e.incarnation, n: e.generation } });
          hubs.publishEvent(id, e);
          break;
        case "rebuilt":
          hubs.rewindAll(id, { reason: "REBUILT", incarnation: e.incarnation });
          hubs.publishEvent(id, e);
          break;
        case "lagged":
          // Events were dropped before registration (a rewind or rebuild among them, maybe).
          hubs.rewindAll(id, { reason: "EVENTS_LAGGED" });
          break;
        case "error":
          hubs.publishEvent(id, { type: "error", error: e.error, fatal: e.fatal });
          if (e.fatal) evict(p);
          break;
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
      return project;
    };

    /** Forgets a dead engine so the next `get`/`open` reopens the project from disk. */
    const evict = (p: OpenProject): void => {
      const id = p.engine.projectId;
      if (byId.get(id) !== p) return;
      byId.delete(id);
      for (const [dir, pid] of byDir) if (pid === id) byDir.delete(dir);
      void p.engine.close().catch(() => undefined);
    };

    /** An engine call on `p`; a panic or a dead engine evicts `p`. */
    const call = <A>(p: OpenProject, f: (engine: Engine) => Promise<string>): Effect.Effect<A, FinError> =>
      Effect.tapError(attemptJson<A>(() => f(p.engine)), (e) => Effect.sync(() => (isEngineDead(e) ? evict(p) : undefined)));

    const openDir = (dir: string, relocate: boolean): Promise<OpenProject> => {
      if (closed) return Promise.reject(shuttingDown());
      const id = byDir.get(dir);
      const known = id === undefined ? undefined : byId.get(id);
      if (known) {
        index.touch(known.engine.projectId, known.dir);
        return Promise.resolve(known);
      }
      let pending = opening.get(dir);
      if (!pending) {
        pending = Engine.open(serverData, dir, { relocate })
          .then((engine) => register(engine, dir))
          .finally(() => opening.delete(dir));
        opening.set(dir, pending);
      }
      return pending;
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

    const opsSince = (projectId: string, branch: string, rev: number) =>
      on<CommittedTx[]>(projectId, (e) => e.opsSince(branch, rev));

    return {
      create: (dir, name, rate) =>
        Effect.suspend(() => {
          if (closed) return Effect.fail(shuttingDown());
          const abs = resolve(dir);
          return Effect.flatMap(
            attempt(async () => register(await Engine.create(serverData, abs, name, rate), abs)),
            ref,
          );
        }),

      open: (dir, relocate = false) => Effect.flatMap(attempt(() => openDir(resolve(dir), relocate)), ref),

      commit: (projectId, tx, generation) =>
        Effect.flatMap(get(projectId), (p) =>
          p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<CommittedTx>(p, (e) =>
                  e.commit(JSON.stringify(tx), generation === undefined ? undefined : JSON.stringify(generation)),
                ),
                (committed) => Effect.sync(() => hubs.publishTx(projectId, committed)),
              ),
            ),
          ),
        ),

      snapshot: (projectId, branch) => on(projectId, (e) => e.snapshot(branch)),

      subscribe: (projectId, branch, fromRev) =>
        Stream.unwrap(
          Effect.gen(function* () {
            yield* get(projectId);
            // Subscribe before reading the backlog: a commit landing in between is then
            // both in the backlog and on the hub, and the rev filter below drops the copy.
            const live = yield* PubSub.subscribe(hubs.ops(projectId, branch));
            const backlog = yield* opsSince(projectId, branch, fromRev);
            let last = fromRev;
            const emit = (item: HubItem): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> => {
              if (item.kind === "rewound") {
                // Revs this subscriber reported (or started from) are gone: it must re-read.
                return last >= item.fromRev ? Effect.fail(historyRewound(item.details)) : Effect.succeed([]);
              }
              const c = item.tx;
              if (c.rev <= last) return Effect.succeed([]);
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
            const backlogItems = backlog.map((tx): HubItem => ({ kind: "tx", tx }));
            return Stream.concat(Stream.fromIterable(backlogItems), Stream.fromSubscription(live)).pipe(
              Stream.mapEffect(emit),
              Stream.flattenIterable,
            );
          }),
        ),

      events: (projectId) =>
        Stream.unwrap(
          Effect.gen(function* () {
            const p = yield* get(projectId);
            // Subscribe first: a change landing while the status is read is then not missed.
            const live = yield* PubSub.subscribe(hubs.events(projectId));
            const status = yield* call<unknown>(p, (e) => e.status());
            const first: ProjectEvent = { type: "status", status };
            return Stream.concat(Stream.make(first), Stream.fromSubscription(live));
          }),
        ),

      status: (projectId) => on(projectId, (e) => e.status()),
      takeover: (projectId, confirm) => on(projectId, (e) => e.takeover(confirm)),
      verify: (projectId) => on(projectId, (e) => e.verify()),
      rebuild: (projectId) => on(projectId, (e) => e.rebuild()),
      compact: (projectId) => on(projectId, (e) => e.compact()),
      txGet: (projectId, txId) => on<TxGetResult>(projectId, async (e) => (await e.txGet(txId)) ?? "null"),
      acknowledgeRewind: (projectId, branch, generation) =>
        on<boolean>(projectId, async (e) => String(await e.acknowledgeRewind(branch, generation))),
      acknowledgeSalvage: (projectId, copy) => on<boolean>(projectId, async (e) => String(await e.acknowledgeSalvage(copy))),
      ensure: (projectId) => Effect.asVoid(get(projectId)),

      engineOf: (projectId) => byId.get(projectId)?.engine,
    } satisfies ProjectRegistryShape;
  });
}

export function layerProjectRegistry(serverData: string, options?: RegistryOptions): Layer.Layer<ProjectRegistry> {
  return Layer.effect(ProjectRegistry, makeProjectRegistry(serverData, options));
}
