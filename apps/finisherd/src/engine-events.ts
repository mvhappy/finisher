// Routing one engine event (`Engine.onEvent`) to the registry's hubs (`projects.ts`).
import type { EngineEvent } from "@finisher/contracts";
import type { Hubs } from "./hubs.ts";
import { historyRewound } from "./op-stream.ts";
import type { OpenProject } from "./open-project.ts";

/**
 * Imports go to the op hubs (deduped by rev against commits), rewinds and rebuilds end the
 * subscribers that saw the lost history, status and the rest go to `project.events` (with the
 * engine's `epoch` and the event's `seq`, so a subscriber drops what its first status already
 * covers); a fatal error evicts the engine. `ENGINE_BUSY` never arrives here: it is a call's
 * answer, passed through to the client. The caller checks `p` is still the registered engine.
 */
export function routeEngineEvent(hubs: Hubs, p: OpenProject, json: string, seq: number, evict: (p: OpenProject) => void): void {
  const id = p.engine.projectId;
  let e: EngineEvent;
  try {
    e = JSON.parse(json) as EngineEvent;
  } catch {
    return;
  }
  const publish = (event: Parameters<Hubs["publishEvent"]>[1]["event"]) => hubs.publishEvent(id, { event, source: p.epoch, seq });
  switch (e.type) {
    case "applied":
      hubs.publishTx(id, e.tx);
      break;
    case "status":
      publish({ type: "status", status: e.status });
      break;
    case "rewound": {
      const generation = { incarnation: e.incarnation, n: e.generation };
      hubs.rewind(id, e.branch, e.fromRev, historyRewound({ branch: e.branch, rev: e.fromRev, generation }), generation);
      publish(e);
      break;
    }
    case "rebuilt":
      // `rev: 0` as on fin-store's own `HISTORY_REWOUND` (`REBUILT`): read from the start.
      hubs.endAll(id, 1, (branch) => historyRewound({ branch, rev: 0, reason: "REBUILT", incarnation: e.incarnation }), {
        incarnation: e.incarnation,
      });
      publish(e);
      break;
    case "lagged":
      // Events were dropped before registration (a rewind or rebuild among them, maybe).
      hubs.endAll(id, 1, (branch) => historyRewound({ branch, rev: 0, reason: "EVENTS_LAGGED" }));
      break;
    case "error":
      publish({ type: "error", error: e.error, fatal: e.fatal });
      if (e.fatal) evict(p);
      break;
    case "collect":
      break;
  }
}
