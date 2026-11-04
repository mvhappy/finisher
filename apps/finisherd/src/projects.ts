// ProjectRegistry: open Engines, the `app.db` project index and per-(project, branch) op hubs.
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import type { CommittedTx } from "@finisher/core-types";
import type { FinError } from "@finisher/contracts";
import { Engine } from "@finisher/fin-node";
import { Context, Effect, Layer, PubSub, type Scope, Semaphore, Stream } from "effect";
import { attempt, attemptJson, finError } from "./errors.ts";

export interface ProjectRef {
  readonly projectId: string;
  readonly rev: number;
}

/** One open project: its engine, a commit lock and one op hub per branch. */
interface OpenProject {
  readonly engine: Engine;
  readonly dir: string;
  /** Serializes commit + publish so hubs see revs in commit order. */
  readonly commitLock: Semaphore.Semaphore;
  readonly hubs: Map<string, PubSub.PubSub<CommittedTx>>;
}

export interface ProjectRegistryShape {
  readonly create: (dir: string, name: string, rate: string) => Effect.Effect<ProjectRef, FinError>;
  readonly open: (dir: string) => Effect.Effect<ProjectRef, FinError>;
  readonly commit: (projectId: string, tx: unknown) => Effect.Effect<CommittedTx, FinError>;
  readonly snapshot: (projectId: string, branch: string) => Effect.Effect<{ rev: number; project: unknown }, FinError>;
  /** Replays `rev > fromRev`, then follows live commits: every rev exactly once, in order. */
  readonly subscribe: (projectId: string, branch: string, fromRev: number) => Stream.Stream<CommittedTx, FinError>;
}

export class ProjectRegistry extends Context.Service<ProjectRegistry, ProjectRegistryShape>()(
  "finisherd/ProjectRegistry",
) {}

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

export function makeProjectRegistry(serverData: string): Effect.Effect<ProjectRegistryShape, never, Scope.Scope> {
  return Effect.gen(function* () {
    const index = yield* Effect.acquireRelease(
      Effect.sync(() => new ProjectIndex(serverData)),
      (idx) => Effect.sync(() => idx.close()),
    );
    const byId = new Map<string, OpenProject>();
    const byDir = new Map<string, string>();
    /** In-flight opens, so concurrent `open(dir)` calls share one Engine. */
    const opening = new Map<string, Promise<OpenProject>>();

    yield* Effect.addFinalizer(() =>
      Effect.forEach(
        [...byId.values()],
        (p) =>
          // Waiting for the commit lock means no commit is in flight when the engine closes.
          p.commitLock.withPermits(1)(
            Effect.promise(() => p.engine.close().catch(() => undefined)),
          ),
        { discard: true },
      ),
    );

    const register = (engine: Engine, dir: string): OpenProject => {
      const existing = byId.get(engine.projectId);
      if (existing) {
        // Same project reached through another path string: keep the first engine.
        void engine.close().catch(() => undefined);
        byDir.set(dir, existing.engine.projectId);
        return existing;
      }
      const project: OpenProject = { engine, dir, commitLock: Semaphore.makeUnsafe(1), hubs: new Map() };
      byId.set(engine.projectId, project);
      byDir.set(dir, engine.projectId);
      index.touch(engine.projectId, dir);
      return project;
    };

    const openDir = (dir: string): Promise<OpenProject> => {
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
      Effect.map(attempt(() => p.engine.head(MAIN)), (rev) => ({ projectId: p.engine.projectId, rev }));

    /** An open project by id; after a restart it is reopened from the `app.db` index. */
    const get = (projectId: string): Effect.Effect<OpenProject, FinError> =>
      Effect.suspend(() => {
        const open = byId.get(projectId);
        if (open) return Effect.succeed(open);
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

    const hubFor = (p: OpenProject, branch: string): PubSub.PubSub<CommittedTx> => {
      let hub = p.hubs.get(branch);
      if (!hub) {
        hub = Effect.runSync(PubSub.unbounded<CommittedTx>());
        p.hubs.set(branch, hub);
      }
      return hub;
    };

    const opsSince = (p: OpenProject, branch: string, rev: number) =>
      attemptJson<CommittedTx[]>(() => p.engine.opsSince(branch, rev));

    const registry: ProjectRegistryShape = {
      create: (dir, name, rate) =>
        Effect.suspend(() => {
          const abs = resolve(dir);
          return Effect.flatMap(
            attempt(() => Engine.create(serverData, abs, name, rate)),
            (engine) => ref(register(engine, abs)),
          );
        }),

      open: (dir) => Effect.flatMap(attempt(() => openDir(resolve(dir))), ref),

      commit: (projectId, tx) =>
        Effect.flatMap(get(projectId), (p) =>
          p.commitLock.withPermits(1)(
            Effect.uninterruptible(
              Effect.tap(
                attemptJson<CommittedTx>(() => p.engine.commit(JSON.stringify(tx))),
                (committed) => PubSub.publish(hubFor(p, committed.tx.branch), committed),
              ),
            ),
          ),
        ),

      snapshot: (projectId, branch) =>
        Effect.flatMap(get(projectId), (p) => attemptJson<{ rev: number; project: unknown }>(() => p.engine.snapshot(branch))),

      subscribe: (projectId, branch, fromRev) =>
        Stream.unwrap(
          Effect.gen(function* () {
            const p = yield* get(projectId);
            // Subscribe before reading the backlog: a commit landing in between is then
            // both in the backlog and on the hub, and the rev filter below drops the copy.
            const live = yield* PubSub.subscribe(hubFor(p, branch));
            const backlog = yield* opsSince(p, branch, fromRev);
            let last = fromRev;
            const emit = (c: CommittedTx): Effect.Effect<ReadonlyArray<CommittedTx>, FinError> => {
              if (c.rev <= last) return Effect.succeed([]);
              if (c.rev === last + 1) {
                last = c.rev;
                return Effect.succeed([c]);
              }
              // A gap (should not happen with the commit lock): fill it from the log.
              return Effect.map(opsSince(p, branch, last), (missing) => {
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
    };
    return registry;
  });
}

export function layerProjectRegistry(serverData: string): Layer.Layer<ProjectRegistry> {
  return Layer.effect(ProjectRegistry, makeProjectRegistry(serverData));
}
