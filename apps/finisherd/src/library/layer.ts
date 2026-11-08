// Wiring the library service into `finisherd` (plan 5D Tasks 9–10). The `library` kinds must
// be registered **before** the engine starts (a restart reconciles the jobs it finds against
// the registry), but the service needs the engine: `libraryWiring` gives the engine its
// `register` and forwards the kinds' events to the service once `start` has built it.
// Teardown runs the other way: the service closes before the engine, the catalog threads after
// it (jobs' hooks call the indexer until the engine has stopped them).
import { sep } from "node:path";
import { Context } from "effect";
import type { Audit } from "../control/audit.ts";
import type { ControlStore } from "../control/db.ts";
import type { JobEngine } from "../jobs/api.ts";
import type { KindRegistry } from "../jobs/kinds.ts";
import type { Catalogs } from "./catalog/client.ts";
import type { JournalLike } from "./events.ts";
import { rootHealth } from "./health.ts";
import { type LibraryKindEvents, type LibraryKinds, registerLibraryKinds } from "./kinds.ts";
import { finNodeRootTools, Registry, type RootTools } from "./registry.ts";
import { LibraryService, type LibraryServiceDeps } from "./service.ts";

export class LibraryServiceTag extends Context.Service<LibraryServiceTag, LibraryService>()("finisherd/LibraryService") {}

export interface LibraryWiringOptions {
  readonly serverData: string;
  /** `<server-data>/jobs`, the engine's root. */
  readonly jobsRoot: string;
  readonly control: ControlStore;
  readonly catalogs: Catalogs;
  readonly audit?: Audit;
  readonly journal?: JournalLike;
  readonly tools?: RootTools;
  readonly log?: LibraryServiceDeps["log"];
  readonly service?: Partial<Pick<LibraryServiceDeps, "clock" | "timers" | "timing">>;
}

export interface LibraryWiring {
  readonly registry: Registry;
  /** For `JobEngineOptions.register`. */
  register(kinds: KindRegistry): void;
  /** Builds the service over the started engine and loads every library. */
  start(engine: LibraryServiceDeps["engine"] & Pick<JobEngine, "onDone">): Promise<LibraryService>;
}

/** True when `path` is `root` or lies under it (plain string prefix on separators). */
export function underRoot(root: string, path: string): boolean {
  if (path === root) return true;
  const base = root.endsWith(sep) || root.endsWith("/") ? root : `${root}${sep}`;
  return path.startsWith(base) || (sep !== "/" && path.startsWith(`${root}/`));
}

/** The ids of the libraries whose root or an alias holds one of `paths` (a project's assets). */
export function librariesHolding(registry: Registry, paths: ReadonlyArray<string>): string[] {
  const out: string[] = [];
  for (const lib of registry.list()) {
    const roots = [lib.root, ...lib.aliases];
    if (paths.some((p) => roots.some((r) => underRoot(r, p)))) out.push(lib.id);
  }
  return out;
}

export function libraryWiring(opts: LibraryWiringOptions): LibraryWiring {
  const tools = opts.tools ?? finNodeRootTools;
  const registry = new Registry(opts.control, { serverData: opts.serverData, tools, ...(opts.audit === undefined ? {} : { audit: opts.audit }) });
  const health = rootHealth({ library: (id) => registry.get(id), probeRoot: tools.probeRoot });
  let service: LibraryService | undefined;
  let kinds: LibraryKinds | undefined;
  const events: LibraryKindEvents = {
    batchApplied: (lib, job, applied) => service?.kindEvents.batchApplied?.(lib, job, applied),
    probed: (lib, job, applied) => service?.kindEvents.probed?.(lib, job, applied),
    rescan: (lib, dirs) => service?.kindEvents.rescan?.(lib, dirs),
    offline: (lib, job) => service?.kindEvents.offline?.(lib, job),
  };
  return {
    registry,
    register(k) {
      kinds = registerLibraryKinds(k, {
        catalogs: opts.catalogs,
        jobsRoot: opts.jobsRoot,
        library: (id) => registry.get(id),
        online: (id) => (service === undefined ? health.online(id) : service.online(id)),
        events,
      });
    },
    async start(engine) {
      engine.onDone((job) => kinds?.jobDone(job));
      service = new LibraryService({
        registry,
        catalogs: opts.catalogs,
        engine,
        health,
        ...(opts.journal === undefined ? {} : { journal: opts.journal }),
        ...(opts.log === undefined ? {} : { log: opts.log }),
        ...opts.service,
      });
      await service.load();
      return service;
    },
  };
}
