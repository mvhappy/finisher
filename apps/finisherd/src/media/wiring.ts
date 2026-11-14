// The server's proxy wiring (6B-1 Task 14), kept out of `main.ts`: the capability cache and the
// proxy service, made once the job engine runs; the project registry's hooks (an asset first used
// here, by opening its project or adding it, gets a background 1080 proxy; a commit's added assets
// only; a project opened before the service starts is scanned when it does); the `media.proxy`
// kind's view of the cache and the report; and the `proxy540` stage, registered with the first
// capability answer (which a server with libraries asks for at start). Also the capability
// URLs of Task 17: the tokens (dropped with their principal's tickets) and each listener's
// media route. 6C Task 18: the PCM service, its first-use hook (an asset on an audio track gets a
// background pcm48) and the pcm16m stage, registered at start (it needs no capability report).
import type { CommittedTx } from "@finisher/core-types";
import { testHooks } from "@finisher/fin-node";
import { Effect } from "effect";
import type { Tickets } from "../auth/tickets.ts";
import type { ListenerPolicy } from "../http/policy.ts";
import type { JobEngine } from "../jobs/api.ts";
import { MEDIA_LOOK } from "../jobs/kinds/media.look.ts";
import type { ProxyKindDeps } from "../jobs/kinds/media.proxy.ts";
import { MEDIA_THUMBS } from "../jobs/kinds/media.thumbs.ts";
import type { LibraryService } from "../library/service.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import type { MediaCacheService } from "./cache.ts";
import { CapTokens } from "./cap-tokens.ts";
import { makeMediaHttp, type MediaRequestHandler } from "./http.ts";
import { type CapabilitiesCache, mediaCapabilities } from "./caps.ts";
import { itemsOf, makePcmService, type PcmService, pcm16mStage } from "./pcm.ts";
import { MEDIA_SYSTEM_OWNER, type ProxyService, makeProxyService } from "./proxy.ts";
import { type ThumbsService, makeThumbsService } from "./thumbs.ts";
import { proxy540Stage } from "./stages.ts";

export interface MediaWiring {
  /** The capability URL tokens (Task 17), dropped with their principal. */
  readonly capTokens: CapTokens;
  /** A listener's `/media/` route, under its policy. */
  readonly http: (policy: ListenerPolicy) => MediaRequestHandler;
  readonly onOpened: (projectId: string) => void;
  readonly onCommitted: (projectId: string, committed: CommittedTx) => void;
  /** For `registerMediaKinds`' `proxy`. */
  readonly kindDeps: Pick<ProxyKindDeps, "cache" | "report">;
  /** Makes the cache and the service once the engine and the library service run. */
  readonly start: (
    engine: JobEngine,
    library: Pick<LibraryService, "registerStage" | "registry">,
    projects: Pick<ProjectRegistryShape, "ensure" | "snapshot" | "resolveBranch" | "dirOf">,
  ) => { readonly capabilities: CapabilitiesCache; readonly proxies: ProxyService; readonly thumbs: ThumbsService; readonly pcm: PcmService };
}

export function mediaWiring(mediaCache: MediaCacheService, tickets: Pick<Tickets, "onDrop">, log: (message: string) => void): MediaWiring {
  const capTokens = new CapTokens();
  tickets.onDrop((pred) => capTokens.dropWhere(pred));
  let caps: CapabilitiesCache | undefined;
  let proxies: ProxyService | undefined;
  // 6C Task 18: the pcm48 of an asset first used on an audio track, beside the proxy hook.
  let pcm: PcmService | undefined;
  /** Projects opened (or committed to) before `start`: scanned whole once the service runs. */
  const early = new Set<string>();
  return {
    capTokens,
    http: (policy) => makeMediaHttp({ tokens: capTokens, cache: () => mediaCache.cache }, policy),
    onOpened: (id) => {
      if (proxies === undefined) early.add(id);
      else void proxies.used(id);
      if (pcm !== undefined) void pcm.used(id);
    },
    onCommitted: (id, committed) => {
      const items = itemsOf(committed);
      if (items.length > 0) {
        if (pcm === undefined) early.add(id);
        else void pcm.used(id, items);
      }
      const added = committed.diff.changes.filter((c) => c.kind === "asset" && c.change === "added").map((c) => c.id);
      if (added.length === 0) return;
      if (proxies === undefined) early.add(id);
      else void proxies.used(id, added);
    },
    kindDeps: { cache: () => mediaCache.cache, report: () => caps?.current() },
    start(engine, library, projects) {
      let staged = false;
      const capabilities = mediaCapabilities({
        engine,
        onAnswer: () => {
          if (staged) return;
          staged = true;
          library.registerStage(proxy540Stage({ report: () => caps?.current(), cache: () => mediaCache.cache }));
        },
      });
      caps = capabilities;
      // The pins of a look or thumbnails job on the proxies its manifest lists (6B-1 Tasks 16 and
      // 15) end with the job, however it ends.
      engine.onDone((job) => {
        if (job.kind === MEDIA_LOOK || job.kind === MEDIA_THUMBS) void mediaCache.cache?.release(`job:${job.id}`).catch(() => undefined);
      });
      const delay = testHooks === undefined ? 0 : Number(process.env.FIN_TEST_PROXY_DELAY_MS ?? 0);
      proxies = makeProxyService({ registry: projects, engine, capabilities, cache: () => mediaCache.cache, log, ...(Number.isFinite(delay) && delay > 0 ? { testDelayMs: delay } : {}) });
      if (library.registry.list().length > 0) void Effect.runPromise(Effect.ignore(capabilities(MEDIA_SYSTEM_OWNER)));
      pcm = makePcmService({ registry: projects, engine, cache: () => mediaCache.cache, log });
      library.registerStage(pcm16mStage({ cache: () => mediaCache.cache }));
      for (const id of early) {
        void proxies.used(id);
        void pcm.used(id);
      }
      early.clear();
      return { capabilities, proxies, thumbs: makeThumbsService({ engine, capabilities, proxies, cache: () => mediaCache.cache }), pcm };
    },
  };
}
