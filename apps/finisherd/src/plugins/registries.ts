// The server's registries and what they offer (8A Task 9a): fin-node's `Registry` for the
// folders and their TUF trust, and the `plugins` chain of `control.db` for who added each
// registry, what the last refresh found and which plugins are installed. fin-registry owns
// the trust; this module never decides it.
//
// - Adding needs a root (a root file of the folder or its SHA-256): without one nothing is added.
// - A refresh by anyone updates the trusted metadata and the registry's row; whether anything
//   may install from it depends on who asked (`byHuman`, Task 9b).
// - `control.db`'s `registries` rows and fin-registry's folders are reconciled at start (a
//   crash between the two writes leaves one without the other).
import { Registry } from "@finisher/fin-node";
import type { FinError, InstalledRow, PluginInfoResult, PluginSearchRequest, RefreshResult, RegistryAddRequest, RegistryEntry, RegistryListResult, RegistryRefreshResult } from "@finisher/contracts";
import type { Listing, PluginInfo, RefreshReport, RegistryList, RegistryStatus } from "@finisher/core-types";
import { Context, Effect, Layer } from "effect";
import { type Audit, AuditTag } from "../control/audit.ts";
import { ControlStoreTag } from "../control/db.ts";
import { attempt, attemptJson, finError, toFinError } from "../errors.ts";
import { Fonts } from "../fonts.ts";
import { type Installed, type Lifecycle, makeLifecycle } from "./lifecycle.ts";
import { type Bundled, type BundledStatus, defaultBundledRoot, makeBundled } from "./bundled.ts";
import { type Content, type ContentProblem, type FontSink, makeContent } from "./content.ts";
import { reconcileStore, type StoreProblem } from "./store-sync.ts";
import { actorOf, PluginTables, type RegistryRow } from "./tables.ts";

export { actorOf, PluginTables } from "./tables.ts";

/** Who asked, as the registries record it. */
export interface Asker {
  readonly kind: string;
  readonly id: string;
  readonly human: boolean;
}

export interface PluginsShape {
  readonly tables: PluginTables;
  readonly registries: () => Effect.Effect<RegistryListResult, FinError>;
  readonly status: (id: string) => Effect.Effect<RegistryEntry, FinError>;
  readonly add: (req: RegistryAddRequest, by: Asker) => Effect.Effect<RegistryEntry, FinError>;
  readonly remove: (id: string) => Effect.Effect<void, FinError>;
  readonly forget: (rootSha256: string) => Effect.Effect<void, FinError>;
  /**
   * One registry (its failure fails the call) or every added one (failures are results). A
   * refresh that succeeds applies the registry's `disable` advisories; a person's also updates
   * content packs with automatic updates on (`updates`: what they did, for the audit log).
   */
  readonly refresh: (id: string | undefined, by: Asker) => Effect.Effect<RegistryRefreshResult & { readonly updates: ReadonlyArray<Installed> }, FinError>;
  /** Plans, approvals and the lifecycle (Task 9b). */
  readonly lifecycle: Lifecycle;
  readonly installed: () => Effect.Effect<InstalledRow[], FinError>;
  /**
   * Rows whose package folder is missing from the store, as the last start found them (L4),
   * and packs whose contributions are not in use (Task 12a).
   */
  readonly problems: () => ReadonlyArray<StoreProblem | ContentProblem>;
  /** The content packs' contributions: `plugin.content.get`, and the font catalogs' sync. */
  readonly content: Content;
  /** The bundled plugins' integrity, as the last start found it (Task 10). */
  readonly bundled: () => BundledStatus;
  /** Installs, updates and checks the plugins that come with the app (Task 10); runs at start. */
  readonly syncBundled: () => Effect.Effect<BundledStatus, FinError>;
  /** Aligns the store's folders with the rows: orphans are deleted, rows without a folder disabled. */
  readonly reconcileStore: () => Effect.Effect<void, FinError>;
  readonly search: (q: PluginSearchRequest) => Effect.Effect<Listing[], FinError>;
  readonly info: (id: string) => Effect.Effect<PluginInfoResult, FinError>;
}

export class PluginsTag extends Context.Service<PluginsTag, PluginsShape>()("finisherd/Plugins") {}

/** The codes that mean a registry's signed data did not verify (audited as `registry.verifyFail`). */
export function isVerifyFailure(code: string): boolean {
  return code.startsWith("TUF_") || code === "CLOCK_SUSPECT" || code === "REGISTRY_INVALID" || code === "REGISTRY_UNTRUSTED";
}

function entryOf(status: RegistryStatus, row: RegistryRow | undefined): RegistryEntry {
  return {
    ...status,
    addedBy: row?.addedBy ?? "unknown",
    addedAt: row?.addedAt ?? 0,
    ...(row?.lastRefresh === undefined ? {} : { lastRefresh: row.lastRefresh }),
  };
}

export interface MakePluginsOptions {
  /** Where the app's bundled plugin folders are (default `defaultBundledRoot()`). */
  readonly bundledRoot?: string;
  /** Audits the bundled installs (the layer passes the server's; tests without one skip it). */
  readonly audit?: Audit;
  /** Where the enabled packs' font catalogs go (the layer passes the server's font manager). */
  readonly fonts?: FontSink;
  /** Test builds only: plans for these ids are marked agent-facing (Decision 23). */
  readonly testAgentFacing?: (id: string) => boolean;
}

/** An audit log that records nothing, for services made without one. */
const NO_AUDIT = { append: () => undefined } as unknown as Audit;

export function makePlugins(native: Registry, tables: PluginTables, clock: () => number = Date.now, options: MakePluginsOptions = {}): PluginsShape {
  const list = () => attemptJson<RegistryList>(() => native.registryList());
  const row = (id: string) => tables.registry(id);

  const registries = () =>
    Effect.map(list(), (l) => ({ registries: l.registries.map((s) => entryOf(s, row(s.id))), problems: l.problems }));

  const status = (id: string) =>
    Effect.map(attemptJson<RegistryStatus>(() => native.registryStatus(id)), (s) => entryOf(s, row(s.id)));

  const refreshOne = (id: string, by: Asker): Effect.Effect<RefreshResult> =>
    Effect.matchEffect(attemptJson<{ status: RegistryStatus; report: RefreshReport }>(() => native.registryRefresh(id, by.human)), {
      onFailure: (error) =>
        Effect.sync(() => {
          tables.recordRefresh(id, { at: clock(), by: actorOf(by), byHuman: by.human, ok: false, code: error.code });
          return { id, ok: false as const, error: { code: error.code, message: error.message, details: error.details, ...(error.fix === undefined ? {} : { fix: error.fix }) } };
        }),
      onSuccess: (r) =>
        Effect.sync(() => {
          tables.recordRefresh(id, { at: clock(), by: actorOf(by), byHuman: by.human, ok: true });
          return { id, ok: true as const, status: entryOf(r.status, row(id)), report: r.report };
        }),
    });

  let problems: ReadonlyArray<StoreProblem> = [];
  const content = makeContent({ native, tables, fonts: options.fonts });
  const failed = (r: RefreshResult) => (r.ok ? undefined : finError(r.error.code, r.error.message, r.error.details, r.error.fix));
  const lifecycle = makeLifecycle({
    native,
    tables,
    clock,
    changed: content.sync(),
    ...(options.testAgentFacing === undefined ? {} : { testAgentFacing: options.testAgentFacing }),
    refreshOne: (id, by) => Effect.flatMap(refreshOne(id, by), (r) => (r.ok ? Effect.void : Effect.fail(failed(r)!))),
  });

  const bundled: Bundled = makeBundled({ native, tables, lifecycle, audit: options.audit ?? NO_AUDIT, root: options.bundledRoot ?? defaultBundledRoot() });

  /** One registry's refresh and what it changed in the installed plugins. */
  const refreshApplying = (id: string, by: Asker) =>
    Effect.gen(function* () {
      const result = yield* refreshOne(id, by);
      if (!result.ok) return { result, revoked: [], updates: [] };
      const applied = yield* lifecycle.afterRefresh(id, by);
      return { result, revoked: applied.revoked, updates: applied.updated };
    });

  return {
    tables,
    lifecycle,
    registries,
    status,
    add: (req, by) =>
      Effect.gen(function* () {
        const added = yield* attemptJson<RegistryStatus>(() => native.registryAdd(JSON.stringify(req.cfg), JSON.stringify(req.root)));
        // The folder is trusted now; the row follows. If it cannot be written the registry is
        // taken back, so neither half is left behind.
        yield* Effect.try({
          try: () => tables.addRegistry({ id: added.id, kind: added.kind, rootSha256: added.rootSha256, freshness: added.freshness, addedBy: actorOf(by), addedAt: clock() }),
          catch: toFinError,
        }).pipe(Effect.tapError(() => Effect.ignore(attempt(() => native.registryRemove(added.id)))));
        return entryOf(added, row(added.id));
      }),
    remove: (id) =>
      Effect.gen(function* () {
        yield* attempt(() => native.registryRemove(id));
        tables.removeRegistry(id);
      }),
    forget: (rootSha256) => attempt(() => native.registryForget(rootSha256)),
    refresh: (id, by) =>
      Effect.gen(function* () {
        const ids = id !== undefined ? [id] : (yield* list()).registries.map((r) => r.id);
        const results: RefreshResult[] = [];
        const revoked: Array<RegistryRefreshResult["applied"]["revoked"][number]> = [];
        const updates: Installed[] = [];
        for (const rid of ids) {
          const one = yield* refreshApplying(rid, by);
          if (id !== undefined && !one.result.ok) return yield* Effect.fail(failed(one.result)!);
          results.push(one.result);
          revoked.push(...one.revoked);
          updates.push(...one.updates);
        }
        const updated = updates.map((u) => ({ id: u.plugin.id, from: u.plan.from ?? "", to: u.plugin.version }));
        return { byHuman: by.human, results, applied: { revoked, updated }, updates };
      }),
    problems: () => [...problems, ...content.problems()],
    content,
    bundled: bundled.status,
    syncBundled: () => bundled.sync(),
    reconcileStore: () =>
      Effect.map(reconcileStore(native, tables, clock), (r) => {
        problems = r.problems;
      }),
    installed: () =>
      Effect.gen(function* () {
        const stale = new Map((yield* list()).registries.flatMap((r) => (r.staleDays === undefined ? [] : [[r.id, r.staleDays] as const])));
        return tables.installed().map((p) => {
          const days = p.registryId === undefined ? undefined : stale.get(p.registryId);
          return days === undefined ? p : { ...p, registryStaleDays: days };
        });
      }),
    search: (q) => attemptJson<Listing[]>(() => native.search(JSON.stringify(q))),
    info: (id) =>
      Effect.gen(function* () {
        const registries = yield* attemptJson<PluginInfo[]>(() => native.info(id));
        const installed = tables.installedOne(id);
        if (registries.length === 0 && installed === undefined) {
          return yield* Effect.fail(finError("PLUGIN_NOT_FOUND", `no registry lists ${id} and it is not installed`, { id }, "Check the plugin's id, or add the registry that offers it."));
        }
        return { id, ...(installed === undefined ? {} : { installed }), registries };
      }),
  };
}

/** Aligns the `registries` rows with the folders fin-registry has (see the file comment). */
export function reconcile(native: Registry, tables: PluginTables, clock: () => number): Effect.Effect<void, FinError> {
  return Effect.gen(function* () {
    const l = yield* attemptJson<RegistryList>(() => native.registryList());
    const real = new Map<string, RegistryStatus | undefined>([...l.registries.map((s) => [s.id, s] as const), ...l.problems.map((p) => [p.id, undefined] as const)]);
    for (const r of tables.registries()) if (!real.has(r.id)) tables.removeRegistry(r.id);
    for (const [id, s] of real) {
      if (tables.registry(id) !== undefined) continue;
      tables.addRegistry({ id, kind: s?.kind ?? "local", rootSha256: s?.rootSha256 ?? "", freshness: s?.freshness ?? "relaxed", addedBy: "system:reconcile", addedAt: clock() });
    }
  });
}

export interface PluginsOptions {
  readonly clock?: () => number;
  /** Where the app's bundled plugin folders are (default `defaultBundledRoot()`). */
  readonly bundledRoot?: string;
  /** How often expired plans are swept (Decision 18: every minute, and at start). */
  readonly sweepMs?: number;
}

export const layerPlugins = (serverData: string, { clock = Date.now, sweepMs = 60_000, bundledRoot }: PluginsOptions = {}): Layer.Layer<PluginsTag, FinError, ControlStoreTag | AuditTag | Fonts> =>
  Layer.effect(
    PluginsTag,
    Effect.gen(function* () {
      const control = yield* ControlStoreTag;
      const native = yield* Effect.try({ try: () => Registry.open(serverData), catch: toFinError });
      const tables = new PluginTables(control);
      yield* reconcile(native, tables, clock);
      const audit = yield* AuditTag;
      const fonts = yield* Fonts;
      const plugins = makePlugins(native, tables, clock, { audit, fonts: fonts.setExtraCatalogs, ...(bundledRoot === undefined ? {} : { bundledRoot }) });
      yield* plugins.reconcileStore();
      // The plugins that come with the app: installed, updated or revoked before anything is served.
      yield* Effect.catch(plugins.syncBundled(), (e) => Effect.logWarning(`finisherd: bundled plugins could not be checked: ${e.code}`));
      // The enabled packs' font catalogs join the server's before anything is served.
      yield* plugins.content.sync();
      yield* Effect.ignore(plugins.lifecycle.sweep());
      yield* Effect.acquireRelease(
        Effect.sync(() => {
          const timer = setInterval(() => void Effect.runPromise(Effect.ignore(plugins.lifecycle.sweep())), sweepMs);
          timer.unref();
          return timer;
        }),
        (timer) => Effect.sync(() => clearInterval(timer)),
      );
      return plugins;
    }),
  );
