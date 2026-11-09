// The `plugins` chain's tables (8A Task 9a, spec §5.1): the only module that touches
// `registries`, `plugins`, `plugin_grants`, `plugin_plans` and `plugin_suggestions`. Importing it registers the chain
// (`main.ts` imports it before it opens the control store). Statements are prepared once.
import type { StatementSync } from "node:sqlite";
import type { InstalledRow, RegistryEntry } from "@finisher/contracts";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { plugins0001 } from "../control/migrations/plugins/0001.ts";
import { plugins0002 } from "../control/migrations/plugins/0002.ts";
import { plugins0003 } from "../control/migrations/plugins/0003.ts";
import { plugins0004 } from "../control/migrations/plugins/0004.ts";

registerChain("plugins", [plugins0001, plugins0002, plugins0003, plugins0004], { formatVersion: 4, minReaderVersion: 1 });

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
  /** The root of the registry the version was verified under (M1). */
  readonly rootSha256?: string;
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

/** An installed plugin as a plan is made against it (fin-registry's `InstalledPlugin` fields). */
export interface InstalledFull extends InstalledRow {
  readonly treeHash: string;
  readonly targetSha256?: string;
  /** The root of the registry the version was verified under (M1); absent for bundled and dev. */
  readonly rootSha256?: string;
  readonly dir: string;
}

/** A `plugin_plans` row (Decision 18; review M4: who made it). */
export interface PlanRow {
  readonly id: string;
  readonly digest: string;
  readonly pluginId: string;
  readonly version: string;
  readonly source: string;
  readonly registryId?: string;
  readonly action: string;
  readonly byKind: string;
  readonly byId: string;
  readonly human: boolean;
  /** The installed version and grants the plan was made against (review M2), JSON or null. */
  readonly base: string | null;
  /** The root of the registry the plan's package was verified under (M1). */
  readonly rootSha256?: string;
  /** A non-human principal's plan: its staged files are gone and no approval can use it (M2). */
  readonly unusable: boolean;
  readonly createdAt: number;
  readonly expiresAt: number;
}

export interface SuggestionRow {
  readonly id: string;
  readonly pluginId: string;
  readonly projectId: string;
  readonly reason: string;
  readonly byKind: string;
  readonly byId: string;
  readonly createdAt: number;
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
  problem: string | null;
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
    ...(r.problem === null ? {} : { problem: r.problem }),
  };
}

export class PluginTables {
  readonly store: ControlStore;
  readonly #s: Record<string, StatementSync>;

  constructor(store: ControlStore) {
    this.store = store;
    const p = (sql: string): StatementSync => store.db.prepare(sql);
    const REG = "id, kind, rootSha256, freshness, addedBy, addedAt, lastRefreshAt, lastRefreshBy, lastRefreshByHuman, lastRefreshOk, lastRefreshCode";
    const PLG = "id, version, source, registryId, kind, enabled, state, autoUpdate, installedBy, installedAt, problem";
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
        "INSERT INTO plugins (id, version, source, registryId, targetSha256, rootSha256, treeHash, dir, kind, enabled, state, autoUpdate, installedBy, installedAt, updatedAt) " +
          "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      ),
      full: p(`SELECT ${PLG}, treeHash, targetSha256, rootSha256, dir FROM plugins WHERE id = ?`),
      replacePlugin: p(
        "UPDATE plugins SET version = ?, source = ?, registryId = ?, targetSha256 = ?, rootSha256 = ?, treeHash = ?, dir = ?, kind = ?, enabled = ?, state = ?, autoUpdate = ?, updatedAt = ? WHERE id = ?",
      ),
      setState: p("UPDATE plugins SET enabled = ?, state = ?, updatedAt = ? WHERE id = ?"),
      setProblem: p("UPDATE plugins SET problem = ? WHERE id = ?"),
      deletePlugin: p("DELETE FROM plugins WHERE id = ?"),
      grants: p("SELECT permission FROM plugin_grants WHERE pluginId = ? ORDER BY rowid"),
      deleteGrants: p("DELETE FROM plugin_grants WHERE pluginId = ?"),
      addGrant: p("INSERT INTO plugin_grants (pluginId, version, permission, grantedBy, grantedAt) VALUES (?, ?, ?, ?, ?)"),
      plan: p("SELECT * FROM plugin_plans WHERE id = ?"),
      addPlan: p(
        "INSERT INTO plugin_plans (id, digest, pluginId, version, source, registryId, action, byKind, byId, human, base, createdAt, expiresAt, rootSha256, unusable) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
      ),
      deletePlan: p("DELETE FROM plugin_plans WHERE id = ?"),
      expiredPlans: p("DELETE FROM plugin_plans WHERE expiresAt <= ?"),
      openPlans: p("SELECT COUNT(*) AS n FROM plugin_plans WHERE byKind = ? AND byId = ? AND expiresAt > ?"),
      addSuggestion: p("INSERT INTO plugin_suggestions (id, pluginId, projectId, reason, byKind, byId, createdAt) VALUES (?, ?, ?, ?, ?, ?, ?)"),
      suggestions: p("SELECT * FROM plugin_suggestions ORDER BY createdAt, id"),
      suggested: p("SELECT * FROM plugin_suggestions WHERE byKind = ? AND byId = ? AND projectId = ? ORDER BY createdAt, id"),
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
      r.rootSha256 ?? null,
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

  /** The installed row with its store fields, for a plan's `installed`. */
  installedFull(id: string): InstalledFull | undefined {
    const r = this.#s.full!.get(id) as (PluginDbRow & { treeHash: string; targetSha256: string | null; rootSha256: string | null; dir: string }) | undefined;
    if (r === undefined) return undefined;
    return {
      ...toInstalled(r),
      treeHash: r.treeHash,
      ...(r.targetSha256 === null ? {} : { targetSha256: r.targetSha256 }),
      ...(r.rootSha256 === null ? {} : { rootSha256: r.rootSha256 }),
      dir: r.dir,
    };
  }

  /** The granted permissions of `id`, as stored (canonical JSON), in grant order. */
  grants(id: string): string[] {
    return (this.#s.grants!.all(id) as Array<{ permission: string }>).map((g) => g.permission);
  }

  /** Replaces `id`'s grants with `permissions` (canonical JSON) for `version`. */
  setGrants(id: string, version: string, permissions: ReadonlyArray<string>, by: string, at: number): void {
    this.#s.deleteGrants!.run(id);
    for (const perm of permissions) this.#s.addGrant!.run(id, version, perm, by, at);
  }

  /** Points the row of `r.id` at a new version at `at` (an update), keeping who first installed it. */
  replacePlugin(r: Omit<NewPluginRow, "installedBy" | "installedAt">, at: number): void {
    this.#s.replacePlugin!.run(
      r.version,
      r.source,
      r.registryId ?? null,
      r.targetSha256 ?? null,
      r.rootSha256 ?? null,
      r.treeHash,
      r.dir,
      r.kind,
      r.enabled === true ? 1 : 0,
      r.state ?? "active",
      r.autoUpdate === true ? 1 : 0,
      at,
      r.id,
    );
  }

  setState(id: string, enabled: boolean, state: InstalledRow["state"], at: number): boolean {
    return Number(this.#s.setState!.run(enabled ? 1 : 0, state, at, id).changes) > 0;
  }

  /** Why a code plugin does not run (sp8b Task 5), or `null` once it may again. */
  setProblem(id: string, problem: string | null): void {
    this.#s.setProblem!.run(problem, id);
  }

  deletePlugin(id: string): void {
    this.#s.deletePlugin!.run(id);
    this.#s.deleteGrants!.run(id);
  }

  plan(id: string): PlanRow | undefined {
    const r = this.#s.plan!.get(id) as (Omit<PlanRow, "human" | "registryId" | "rootSha256" | "unusable"> & { human: number; registryId: string | null; rootSha256: string | null; unusable: number }) | undefined;
    if (r === undefined) return undefined;
    const { registryId, rootSha256, human, unusable, ...rest } = r;
    return { ...rest, human: human === 1, unusable: unusable === 1, ...(registryId === null ? {} : { registryId }), ...(rootSha256 === null ? {} : { rootSha256 }) };
  }

  addPlan(r: PlanRow): void {
    this.#s.addPlan!.run(r.id, r.digest, r.pluginId, r.version, r.source, r.registryId ?? null, r.action, r.byKind, r.byId, r.human ? 1 : 0, r.base, r.createdAt, r.expiresAt, r.rootSha256 ?? null, r.unusable ? 1 : 0);
  }

  deletePlan(id: string): void {
    this.#s.deletePlan!.run(id);
  }

  /** Deletes the plans expired at `now`; returns how many. */
  deleteExpiredPlans(now: number): number {
    return Number(this.#s.expiredPlans!.run(now).changes);
  }

  /** How many plans `by` has that have not expired at `now` (usable or not). */
  openPlans(by: { readonly kind: string; readonly id: string }, now: number): number {
    return Number((this.#s.openPlans!.get(by.kind, by.id, now) as { n: number }).n);
  }

  /** The root of registry `id` as `control.db` records it (M1); absent when it is not added. */
  rootOf(registryId: string): string | undefined {
    return this.registry(registryId)?.rootSha256 || undefined;
  }

  /** Whether `row` still comes from the root its registry has now (M1). */
  rootMatches(row: { readonly registryId?: string | undefined; readonly rootSha256?: string | undefined }): boolean {
    if (row.registryId === undefined || row.rootSha256 === undefined) return false;
    return this.rootOf(row.registryId) === row.rootSha256;
  }

  /** Whether the installed `pluginId` was verified under the root its registry has now (M1). */
  installedRootMatches(pluginId: string): boolean {
    const row = this.installedFull(pluginId);
    return row !== undefined && this.rootMatches(row);
  }

  addSuggestion(r: SuggestionRow): void {
    this.#s.addSuggestion!.run(r.id, r.pluginId, r.projectId, r.reason, r.byKind, r.byId, r.createdAt);
  }

  /** The suggestions `by` made for `projectId`, oldest first. */
  suggestedBy(by: { readonly kind: string; readonly id: string }, projectId: string): SuggestionRow[] {
    return this.#s.suggested!.all(by.kind, by.id, projectId) as unknown as SuggestionRow[];
  }

  suggestions(): SuggestionRow[] {
    return this.#s.suggestions!.all() as unknown as SuggestionRow[];
  }
}
