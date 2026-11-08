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
import { ControlStoreTag } from "../control/db.ts";
import { attempt, attemptJson, finError, toFinError } from "../errors.ts";
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
  /** One registry (its failure fails the call) or every added one (failures are results). */
  readonly refresh: (id: string | undefined, by: Asker) => Effect.Effect<RegistryRefreshResult, FinError>;
  readonly installed: () => Effect.Effect<InstalledRow[], FinError>;
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

export function makePlugins(native: Registry, tables: PluginTables, clock: () => number = Date.now): PluginsShape {
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

  return {
    tables,
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
        if (id !== undefined) {
          const one = yield* refreshOne(id, by);
          if (!one.ok) return yield* Effect.fail(finError(one.error.code, one.error.message, one.error.details, one.error.fix));
          return { byHuman: by.human, results: [one] };
        }
        const ids = (yield* list()).registries.map((r) => r.id);
        const results: RefreshResult[] = [];
        for (const rid of ids) results.push(yield* refreshOne(rid, by));
        return { byHuman: by.human, results };
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
}

export const layerPlugins = (serverData: string, { clock = Date.now }: PluginsOptions = {}): Layer.Layer<PluginsTag, FinError, ControlStoreTag> =>
  Layer.effect(
    PluginsTag,
    Effect.gen(function* () {
      const control = yield* ControlStoreTag;
      const native = yield* Effect.try({ try: () => Registry.open(serverData), catch: toFinError });
      const tables = new PluginTables(control);
      yield* reconcile(native, tables, clock);
      return makePlugins(native, tables, clock);
    }),
  );
