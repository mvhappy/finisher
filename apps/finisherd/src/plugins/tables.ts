// The `plugins` chain's tables (8A Task 9a, spec §5.1): the only module that touches
// `registries`, `plugins`, `plugin_grants` and `plugin_plans`. Importing it registers the chain
// (`main.ts` imports it before it opens the control store). Statements are prepared once.
import type { StatementSync } from "node:sqlite";
import type { InstalledRow, RegistryEntry } from "@finisher/contracts";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { plugins0001 } from "../control/migrations/plugins/0001.ts";

registerChain("plugins", [plugins0001], { formatVersion: 1, minReaderVersion: 1 });

/** `<kind>:<id>` of a principal, as stored in `addedBy`, `installedBy` and `lastRefreshBy`. */
export const actorOf = (p: { readonly kind: string; readonly id: string }): string => `${p.kind}:${p.id}`;

/** What `control.db` adds to a registry's status (the rest is fin-registry's). */
export interface RegistryRow {
  readonly id: string;
  readonly kind: string;
  readonly rootSha256: string;
  readonly freshness: string;
  readonly addedBy: string;
  readonly addedAt: number;
  readonly lastRefresh?: RegistryEntry["lastRefresh"];
}

export interface RefreshRecord {
  readonly at: number;
  readonly by: string;
  readonly byHuman: boolean;
  readonly ok: boolean;
  readonly code?: string | undefined;
}

/** A `plugins` row to insert (an install, Task 9b; tests seed rows with it). */
export interface NewPluginRow {
  readonly id: string;
  readonly version: string;
  readonly source: InstalledRow["source"];
  readonly registryId?: string;
  readonly targetSha256?: string;
  readonly treeHash: string;
  readonly dir: string;
  readonly kind: InstalledRow["kind"];
  readonly enabled?: boolean;
  readonly state?: InstalledRow["state"];
  readonly autoUpdate?: boolean;
  readonly installedBy: string;
  readonly installedAt: number;
}

interface RegistryDbRow {
  id: string;
  kind: string;
  rootSha256: string;
  freshness: string;
  addedBy: string;
  addedAt: number;
  lastRefreshAt: number | null;
  lastRefreshBy: string | null;
  lastRefreshByHuman: number | null;
  lastRefreshOk: number | null;
  lastRefreshCode: string | null;
}

interface PluginDbRow {
  id: string;
  version: string;
  source: InstalledRow["source"];
  registryId: string | null;
  kind: InstalledRow["kind"];
  enabled: number;
  state: InstalledRow["state"];
  autoUpdate: number;
  installedBy: string;
  installedAt: number;
}

function toRegistryRow(r: RegistryDbRow): RegistryRow {
  const base = { id: r.id, kind: r.kind, rootSha256: r.rootSha256, freshness: r.freshness, addedBy: r.addedBy, addedAt: r.addedAt };
  if (r.lastRefreshAt === null) return base;
  return {
    ...base,
    lastRefresh: {
      at: r.lastRefreshAt,
      by: r.lastRefreshBy ?? "",
      byHuman: r.lastRefreshByHuman === 1,
      ok: r.lastRefreshOk === 1,
      ...(r.lastRefreshCode === null ? {} : { code: r.lastRefreshCode }),
    },
  };
}

function toInstalled(r: PluginDbRow): InstalledRow {
  return {
    id: r.id,
    version: r.version,
    source: r.source,
    ...(r.registryId === null ? {} : { registryId: r.registryId }),
    kind: r.kind,
    enabled: r.enabled === 1,
    state: r.state,
    autoUpdate: r.autoUpdate === 1,
    installedBy: r.installedBy,
    installedAt: r.installedAt,
  };
}

export class PluginTables {
  readonly store: ControlStore;
  readonly #s: Record<string, StatementSync>;

  constructor(store: ControlStore) {
    this.store = store;
    const p = (sql: string): StatementSync => store.db.prepare(sql);
    const REG = "id, kind, rootSha256, freshness, addedBy, addedAt, lastRefreshAt, lastRefreshBy, lastRefreshByHuman, lastRefreshOk, lastRefreshCode";
    const PLG = "id, version, source, registryId, kind, enabled, state, autoUpdate, installedBy, installedAt";
    this.#s = {
      registries: p(`SELECT ${REG} FROM registries ORDER BY id`),
      registry: p(`SELECT ${REG} FROM registries WHERE id = ?`),
      addRegistry: p("INSERT INTO registries (id, kind, rootSha256, freshness, addedBy, addedAt) VALUES (?, ?, ?, ?, ?, ?)"),
      removeRegistry: p("DELETE FROM registries WHERE id = ?"),
      refreshed: p(
        "UPDATE registries SET lastRefreshAt = ?, lastRefreshBy = ?, lastRefreshByHuman = ?, lastRefreshOk = ?, lastRefreshCode = ? WHERE id = ?",
      ),
      plugins: p(`SELECT ${PLG} FROM plugins ORDER BY id`),
      plugin: p(`SELECT ${PLG} FROM plugins WHERE id = ?`),
      addPlugin: p(
        "INSERT INTO plugins (id, version, source, registryId, targetSha256, treeHash, dir, kind, enabled, state, autoUpdate, installedBy, installedAt, updatedAt) " +
          "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      ),
    };
  }

  registries(): RegistryRow[] {
    return (this.#s.registries!.all() as unknown as RegistryDbRow[]).map(toRegistryRow);
  }

  registry(id: string): RegistryRow | undefined {
    const r = this.#s.registry!.get(id) as RegistryDbRow | undefined;
    return r === undefined ? undefined : toRegistryRow(r);
  }

  addRegistry(r: Omit<RegistryRow, "lastRefresh">): void {
    this.#s.addRegistry!.run(r.id, r.kind, r.rootSha256, r.freshness, r.addedBy, r.addedAt);
  }

  removeRegistry(id: string): boolean {
    return Number(this.#s.removeRegistry!.run(id).changes) > 0;
  }

  recordRefresh(id: string, r: RefreshRecord): void {
    this.#s.refreshed!.run(r.at, r.by, r.byHuman ? 1 : 0, r.ok ? 1 : 0, r.code ?? null, id);
  }

  installed(): InstalledRow[] {
    return (this.#s.plugins!.all() as unknown as PluginDbRow[]).map(toInstalled);
  }

  installedOne(id: string): InstalledRow | undefined {
    const r = this.#s.plugin!.get(id) as PluginDbRow | undefined;
    return r === undefined ? undefined : toInstalled(r);
  }

  addPlugin(r: NewPluginRow): void {
    this.#s.addPlugin!.run(
      r.id,
      r.version,
      r.source,
      r.registryId ?? null,
      r.targetSha256 ?? null,
      r.treeHash,
      r.dir,
      r.kind,
      r.enabled === true ? 1 : 0,
      r.state ?? "active",
      r.autoUpdate === true ? 1 : 0,
      r.installedBy,
      r.installedAt,
      r.installedAt,
    );
  }
}
