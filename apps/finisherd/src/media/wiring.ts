// The server's proxy wiring (6B-1 Task 14), kept out of `main.ts`: the capability cache and the
// proxy service, made once the job engine runs; the project registry's hooks (an asset first used
// here, by opening its project or adding it, gets a background 1080 proxy); the `media.proxy`
// kind's view of the cache and the report; and the `proxy540` stage, registered with the first
// capability answer (which a server with libraries asks for at start).
import type { CommittedTx } from "@finisher/core-types";
import { Effect } from "effect";
import type { JobEngine } from "../jobs/api.ts";
import type { ProxyKindDeps } from "../jobs/kinds/media.proxy.ts";
import type { LibraryService } from "../library/service.ts";
import type { ProjectRegistryShape } from "../projects.ts";
import type { MediaCacheService } from "./cache.ts";
import { type CapabilitiesCache, mediaCapabilities } from "./caps.ts";
import { MEDIA_SYSTEM_OWNER, type ProxyService, makeProxyService } from "./proxy.ts";
import { proxy540Stage } from "./stages.ts";

export interface MediaWiring {
  readonly onOpened: (projectId: string) => void;
  readonly onCommitted: (projectId: string, committed: CommittedTx) => void;
  /** For `registerMediaKinds`' `proxy`. */
  readonly kindDeps: Pick<ProxyKindDeps, "cache" | "report">;
  /** Makes the cache and the service once the engine and the library service run. */
  readonly start: (
    engine: JobEngine,
    library: Pick<LibraryService, "registerStage" | "registry">,
    projects: Pick<ProjectRegistryShape, "ensure" | "snapshot" | "resolveBranch" | "dirOf">,
  ) => { readonly capabilities: CapabilitiesCache; readonly proxies: ProxyService };
}

export function mediaWiring(mediaCache: MediaCacheService, log: (message: string) => void): MediaWiring {
  let caps: CapabilitiesCache | undefined;
  let proxies: ProxyService | undefined;
  return {
    onOpened: (id) => proxies?.used(id),
    onCommitted: (id, committed) => {
      if (committed.tx.ops.some((o) => (o as { op?: unknown }).op === "asset.add")) proxies?.used(id);
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
      proxies = makeProxyService({ registry: projects, engine, capabilities, cache: () => mediaCache.cache, log });
      if (library.registry.list().length > 0) void Effect.runPromise(Effect.ignore(capabilities(MEDIA_SYSTEM_OWNER)));
      return { capabilities, proxies };
    },
  };
}
