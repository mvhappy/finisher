// The store folders and the `plugins` rows agree at start (8A Task 9b review, L4). A crash
// between placing a version and writing its row, or a removal that stopped half way, leaves
// folders nobody owns; plan time reads the font ids of every folder in the store, so an orphan
// makes a pack's ids look taken (`catalogIdTaken`) for ever. The other way round, a row whose
// folder is gone (someone deleted it) cannot be used: it is disabled and listed as a problem.
//
// Runs once, before anything installs. A folder with the row's own version but another hash
// is left alone (it may be the row's replacement caught half way), as is any folder while its
// plugin has no row but the store is not ours to judge: only versions no row names go.
import { Effect } from "effect";
import type { FinError } from "@finisher/contracts";
import type { Registry } from "@finisher/fin-node";
import { attempt, attemptJson } from "../errors.ts";
import type { PluginTables } from "./tables.ts";

/** A row whose package folder is not in the store. */
export interface StoreProblem {
  readonly id: string;
  readonly version: string;
  readonly reason: "storeMissing";
}

interface StoreDir {
  readonly id: string;
  readonly version: string;
  readonly dir: string;
}

export function reconcileStore(native: Registry, tables: PluginTables, clock: () => number): Effect.Effect<{ readonly removed: StoreDir[]; readonly problems: StoreProblem[] }, FinError> {
  return Effect.gen(function* () {
    const dirs = yield* attemptJson<StoreDir[]>(() => native.installedDirs());
    const rows = tables.installed().flatMap((r) => {
      const full = tables.installedFull(r.id);
      return full === undefined ? [] : [full];
    });
    const removed: StoreDir[] = [];
    for (const d of dirs) {
      const row = rows.find((r) => r.id === d.id);
      if (row !== undefined && (row.dir === d.dir || row.version === d.version)) continue;
      // Removal is by version: a failure leaves the folder for the next start.
      const done = yield* Effect.option(attempt(() => native.removePlugin(d.id, d.version, false)));
      if (done._tag === "Some") removed.push(d);
    }
    const problems: StoreProblem[] = [];
    for (const r of rows) {
      // A developer plugin (sp8b Task 11) is served from its own folder, never the store.
      if (r.source === "dev" || dirs.some((d) => d.dir === r.dir)) continue;
      problems.push({ id: r.id, version: r.version, reason: "storeMissing" });
      if (r.enabled) tables.setState(r.id, false, r.state === "active" ? "disabled" : r.state, clock());
    }
    return { removed, problems };
  });
}
