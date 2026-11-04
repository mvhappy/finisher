// ProjectRegistry: open Engines, the `app.db` project index and per-(project, branch) op hubs.
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { CommittedTx } from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Context, Effect, Layer, PubSub, type Scope, Semaphore, Stream } from "effect";
import { attempt, attemptJson, finError, isEngineDead } from "./errors.ts";

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
  readonly open: (dir: string) => Effect.Effect<ProjectRef, FinError>;
  readonly commit: (projectId: string, tx: unknown) => Effect.Effect<CommittedTx, FinError>;
  readonly snapshot: (projectId: string, branch: string) => Effect.Effect<{ rev: number; project: unknown }, FinError>;
  /** Replays `rev > fromRev`, then follows live commits: every rev exactly once, in order. */
  readonly subscribe: (projectId: string, branch: string, fromRev: number) => Stream.Stream<CommittedTx, FinError>;
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
    /** Keyed by `projectId \0 branch`; outlives engine reopens so live subscribers keep going. */
    const hubs = new Map<string, PubSub.PubSub<CommittedTx>>();
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

    const openDir = (dir: string): Promise<OpenProject> => {
      if (closed) return Promise.reject(shuttingDown());
      const id = byDir.get(dir);
      const known = id === undefined ? undefined : byId.get(id);
      if (known) {
        index.touch(known.engine.projectId, known.dir);
        return Promise.resolve(known);
      }
      let pending = opening.get(dir);
      if (!pending) {
        pending = Engine.open(serverData, dir)
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
        return Effect.flatMap(attempt(() => openDir(dir)), (p) =>
          p.engine.projectId === projectId
            ? Effect.succeed(p)
            : Effect.fail(
                finError("PROJECT_NOT_FOUND", `project ${projectId} is no longer at ${dir}`, { projectId, dir }, "open the project by its directory"),
              ),
        );
      });

    const hubFor = (projectId: string, branch: string): PubSub.PubSub<CommittedTx> => {
      const key = `${projectId}\u0000${branch}`;
      let hub = hubs.get(key);
      if (!hub) {
        // Sliding: a slow subscriber loses the oldest buffered items, never blocks commits;
        // the subscriber notices the rev gap and reads the missing revs from the log.
        hub = Effect.runSync(PubSub.sliding<CommittedTx>(hubCapacity));
        hubs.set(key, hub);
      }
      return hub;
    };

    const opsSince = (projectId: string, branch: string, rev: number) =>
      Effect.flatMap(get(projectId), (p) => call<CommittedTx[]>(p, (e) => e.opsSince(branch, rev)));

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

      open: (dir) => Effect.flatMap(attempt(() => openDir(resolve(dir))), ref),

      commit: (projectId, tx) =>
        Effect.flatMap(get(projectId), (p) =>
          p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                call<CommittedTx>(p, (e) => e.commit(JSON.stringify(tx))),
                (committed) => PubSub.publish(hubFor(projectId, committed.tx.branch), committed),
              ),
            ),
          ),
        ),

      snapshot: (projectId, branch) =>
        Effect.flatMap(get(projectId), (p) => call<{ rev: number; project: unknown }>(p, (e) => e.snapshot(branch))),

      subscribe: (projectId, branch, fromRev) =>
        Stream.unwrap(
          Effect.gen(function* () {
            yield* get(projectId);
            // Subscribe before reading the backlog: a commit landing in between is then
            // both in the backlog and on the hub, and the rev filter below drops the copy.
            const live = yield* PubSub.subscribe(hubFor(projectId, branch));
            const backlog = yield* opsSince(projectId, branch, fromRev);
            let last = fromRev;
            const emit = (c: CommittedTx): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> => {
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
            return Stream.concat(Stream.fromIterable(backlog), Stream.fromSubscription(live)).pipe(
              Stream.mapEffect(emit),
              Stream.flattenIterable,
            );
          }),
        ),

      engineOf: (projectId) => byId.get(projectId)?.engine,
    } satisfies ProjectRegistryShape;
  });
}

export function layerProjectRegistry(serverData: string, options?: RegistryOptions): Layer.Layer<ProjectRegistry> {
  return Layer.effect(ProjectRegistry, makeProjectRegistry(serverData, options));
}
