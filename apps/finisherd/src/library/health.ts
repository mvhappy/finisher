// Whether a library's root is usable now, for the library kinds' `resourceHealthy` (plan 5D
// Decision 33): the cached root state, probed again (≤ 5 s, on fin-node's per-root thread)
// when it is older than 60 s. A probe that fails or times out answers `false`, so a crash on
// a dying mount is a resource fault and no file is poisoned on a doubt. The service (Task 10)
// feeds it the states it learns elsewhere (`note`).
import type { RootState } from "@finisher/core-types";
import { Effect } from "effect";
import type { LibraryRow, RootTools } from "./registry.ts";

export const HEALTH_MAX_AGE_MS = 60_000;
export const HEALTH_PROBE_MS = 5_000;

export interface RootHealth {
  /** `true` when the root answers `online` (cached up to 60 s). */
  online(id: string): Effect.Effect<boolean>;
  /** Records a state learnt elsewhere (a scan's first stat, the service's re-probe). */
  note(id: string, state: RootState["state"]): void;
  forget(id: string): void;
}

export function rootHealth(opts: {
  readonly library: (id: string) => Pick<LibraryRow, "root" | "volume" | "state"> | undefined;
  readonly probeRoot: RootTools["probeRoot"];
  readonly clock?: () => number;
}): RootHealth {
  const clock = opts.clock ?? Date.now;
  const cache = new Map<string, { state: RootState["state"]; at: number }>();
  return {
    online: (id) =>
      Effect.promise(async () => {
        const lib = opts.library(id);
        if (lib === undefined) return false;
        const hit = cache.get(id);
        if (hit !== undefined && clock() - hit.at < HEALTH_MAX_AGE_MS) return hit.state === "online";
        let state: RootState["state"];
        try {
          state = (await opts.probeRoot(lib.root, lib.volume, HEALTH_PROBE_MS)).state;
        } catch {
          state = "unreachable";
        }
        cache.set(id, { state, at: clock() });
        return state === "online";
      }),
    note(id, state) {
      cache.set(id, { state, at: clock() });
    },
    forget(id) {
      cache.delete(id);
    },
  };
}
