// The crash loop (sp8 spec §3.3; sp8b plan Decision 15): three `PLUGIN_CRASHED` job ends of one
// plugin, any key, within ten minutes set `state: crashLoop` and `problem: PLUGIN_CRASH_LOOP`,
// stop its processes and audit `plugin.crashLoop`. Its queued jobs then fail
// `PLUGIN_CRASH_LOOP` when they are dispatched (`program.ts` `unusable`, checked at warm
// dispatch and at every spawn). Only the owner's `plugin.enable` clears it.
import type { JobView } from "@finisher/contracts";
import type { Audit } from "../../control/audit.ts";
import type { PluginTables } from "../tables.ts";
import { splitVia } from "./program.ts";

export const CRASH_LOOP_CRASHES = 3;
export const CRASH_LOOP_WINDOW_MS = 10 * 60_000;

export interface CrashLoopDeps {
  readonly tables: PluginTables;
  readonly clock: () => number;
  readonly audit?: Audit;
  /** Stops every process of the plugin. */
  readonly stop: (id: string) => void;
}

/** `onDone` for the job engine: counts the plugin family's `PLUGIN_CRASHED` ends. */
export function makeCrashLoop(d: CrashLoopDeps): { onDone(job: JobView): void } {
  const crashes = new Map<string, number[]>();
  return {
    onDone(job) {
      if (job.family !== "plugin" || job.error?.code !== "PLUGIN_CRASHED" || job.via === undefined) return;
      let id: string;
      try {
        id = splitVia(job.via.plugin).id;
      } catch {
        return;
      }
      const now = d.clock();
      const recent = [...(crashes.get(id) ?? []).filter((t) => now - t < CRASH_LOOP_WINDOW_MS), now];
      if (recent.length < CRASH_LOOP_CRASHES) {
        crashes.set(id, recent);
        return;
      }
      crashes.delete(id);
      const row = d.tables.installedOne(id);
      if (row === undefined || row.state !== "active") return;
      d.tables.store.tx(() => {
        d.tables.setState(id, row.enabled, "crashLoop", now);
        d.tables.setProblem(id, "PLUGIN_CRASH_LOOP");
      });
      d.audit?.append({
        kind: "plugin.crashLoop",
        principal: { kind: "system", id: "plugins" },
        outcome: "ok",
        code: "PLUGIN_CRASH_LOOP",
        details: { pluginId: id, version: row.version, count: CRASH_LOOP_CRASHES, code: "PLUGIN_CRASH_LOOP" },
      });
      d.stop(id);
    },
  };
}
