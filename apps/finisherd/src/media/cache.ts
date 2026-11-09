// The media cache (6B-1 plan Task 13; sub-project 6 spec §7.2): fin-node's `MediaCache` at
// `<server-data>/cache/media/`, opened once per server run. finisherd adds:
// - the cap: `media.cacheMaxGiB` in `settings.json` (default `min(50 GiB, 15 % of the volume)`),
//   re-read every 10 minutes, when eviction also runs (fin-node evicts after each commit);
// - project pins: a project open here holds `project:<id>` pins on the entries serving it until
//   it closes (`holdProject`, bound to the project registry's open hook). Pins never outlive a
//   run, so the projects reopened after a start pin again;
// - the RPCs `media.cache.status {}` (`server.read`) and `media.cache.gc {toGiB?}` (`settings.admin`).
// Running jobs' `job:<id>` pins are the kinds' (6B-1 Tasks 14–16), through `cache`.
import type { FinError } from "@finisher/contracts";
import { type CacheGcResult, type CacheStatus, MediaCache } from "@finisher/fin-node";
import { Context, Effect, type Scope } from "effect";
import { readMediaSettings } from "../config.ts";
import { attempt, finError } from "../errors.ts";

export const GIB = 2 ** 30;

/** How often the cap is re-read and the cache evicted down to it. */
export const EVICT_EVERY_MS = 10 * 60_000;

export interface MediaCacheService {
  readonly cache: MediaCache;
  readonly status: () => Effect.Effect<CacheStatus, FinError>;
  /** Evicts down to the cap, or to `toGiB` when lower. */
  readonly gc: (toGiB?: number) => Effect.Effect<CacheGcResult, FinError>;
  /** Pins the entries serving `projectId` while it is open here; `release` when it closes. */
  readonly holdProject: (projectId: string) => { release(): void };
  /** The timer's work: the cap from `settings.json` (kept when the file is unusable), then eviction. */
  readonly tick: () => Promise<void>;
}

export class MediaCacheTag extends Context.Service<MediaCacheTag, MediaCacheService>()("finisherd/MediaCache") {}

/** The cap `settings.json` names, in bytes; `null` for the default; `undefined` when unusable. */
export function capFromSettings(serverData: string): number | null | undefined {
  const read = readMediaSettings(serverData);
  if (read.error !== undefined) return undefined;
  return read.cacheMaxGiB === undefined ? null : Math.floor(read.cacheMaxGiB * GIB);
}

export interface MediaCacheOptions {
  readonly everyMs?: number;
  readonly log?: (message: string) => void;
}

/** Opens the cache with the cap of `settings.json` and starts the eviction timer (stopped with the scope). */
export function makeMediaCache(serverData: string, options: MediaCacheOptions = {}): Effect.Effect<MediaCacheService, FinError, Scope.Scope> {
  const log = options.log ?? (() => {});
  return Effect.gen(function* () {
    const cap = capFromSettings(serverData);
    if (cap === undefined) log("finisherd: media cache: settings.json media.cacheMaxGiB is unusable; using the default cap");
    const cache = yield* attempt(() => MediaCache.open(serverData, cap ?? undefined));
    const report = cache.openReport();
    if (report.adopted + report.deleted + report.dropped > 0 || report.indexReplaced) {
      log(`finisherd: media cache: adopted ${report.adopted}, deleted ${report.deleted}, dropped ${report.dropped}${report.indexReplaced ? ", index rebuilt" : ""}`);
    }
    const tick = async () => {
      const next = capFromSettings(serverData);
      try {
        if (next !== undefined) await cache.setCap(next ?? undefined);
        else await cache.gc();
      } catch (e) {
        log(`finisherd: media cache: eviction failed (${(e as { code?: string }).code ?? String(e)})`);
      }
    };
    yield* Effect.acquireRelease(
      Effect.sync(() => {
        const timer = setInterval(() => void tick(), options.everyMs ?? EVICT_EVERY_MS);
        timer.unref();
        return timer;
      }),
      (timer) => Effect.sync(() => clearInterval(timer)),
    );
    const service: MediaCacheService = {
      cache,
      tick,
      status: () => attempt(() => cache.status()),
      gc: (toGiB) =>
        toGiB !== undefined && !(Number.isFinite(toGiB) && toGiB >= 0)
          ? Effect.fail(finError("INVALID_OP", "toGiB is a number of GiB, 0 or more", { field: "toGiB", reason: "mediaCache" }, "pass --to <GiB>, e.g. --to 10"))
          : attempt(() => cache.gc(toGiB === undefined ? undefined : Math.floor(toGiB * GIB))),
      holdProject: (projectId) => {
        // The cache thread runs calls in order, so a release always follows its pin.
        void cache.pinProject(projectId).catch(() => undefined);
        let held = true;
        return {
          release: () => {
            if (!held) return;
            held = false;
            void cache.unpinProject(projectId).catch(() => undefined);
          },
        };
      },
    };
    return service;
  });
}
