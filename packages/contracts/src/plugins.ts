// Plugins and registries over RPC (sub-project 8A Task 9a; spec §3.7, §5). The reads and the
// registry administration; plans, installs and the lifecycle are `plugin-lifecycle.ts` (9b).
//
// What the Rust side computes (registry status, listings, package info, refresh reports) is a
// generated type of `@finisher/core-types`; the schemas below are the payloads and the rows
// `control.db` owns (installed plugins).
import type { Listing, PluginInfo, RefreshReport, RegistryList, RegistryStatus } from "@finisher/core-types";
import { Schema } from "effect";
import type { RefreshApplied } from "./plugin-lifecycle.ts";

export type { Listing, PluginInfo, RefreshReport, RegistryList, RegistryStatus } from "@finisher/core-types";

/** `[a-z0-9][a-z0-9-]{0,63}` (8A Decision 14's registry ids). */
export const RegistryIdSchema = Schema.String.check(Schema.isPattern(/^[a-z0-9][a-z0-9-]{0,63}$/));
const Sha256Schema = Schema.String.check(Schema.isPattern(/^[0-9a-fA-F]{64}$/));

export const RegistryIdPayload = Schema.Struct({ id: RegistryIdSchema });

/**
 * `registry.add`: the folder (`path`, absolute) and the root it is trusted through (Decision
 * 16): a root file of the folder (`file`, absolute path) or its SHA-256 (`sha256`: typed, or
 * shown by the console and typed back). Without one of them no registry is added.
 */
export const RegistryAddPayload = Schema.Struct({
  cfg: Schema.Struct({ id: RegistryIdSchema, path: Schema.String }),
  root: Schema.Union([
    Schema.Struct({ kind: Schema.Literal("file"), path: Schema.String }),
    Schema.Struct({ kind: Schema.Literal("sha256"), sha256: Sha256Schema }),
  ]),
});
export type RegistryAddRequest = typeof RegistryAddPayload.Type;

export const RegistryForgetPayload = Schema.Struct({ rootSha256: Sha256Schema });

/** `registry.refresh`: one registry, or every added one without `id`. */
export const RegistryRefreshPayload = Schema.Struct({ id: Schema.optional(RegistryIdSchema) });

/**
 * `RegistryStatus` of fin-registry (the generated type) plus what `control.db` knows of it. The
 * folder's `path` is shown to people only (`console`, `local`): an `agent` or `mcp` principal
 * holding `plugin.read` never learns where a registry lives.
 */
export const RegistryEntrySchema = Schema.Unknown;
export type RegistryEntry = Omit<RegistryStatus, "path"> & {
  readonly path?: string;
  readonly addedBy: string;
  readonly addedAt: number;
  /** The last refresh this server made of it, whoever asked. */
  readonly lastRefresh?: {
    readonly at: number;
    readonly by: string;
    readonly byHuman: boolean;
    readonly ok: boolean;
    readonly code?: string;
  };
};
export interface RegistryListResult {
  readonly registries: ReadonlyArray<RegistryEntry>;
  readonly problems: RegistryList["problems"];
}
export const RegistryListSuccess = Schema.Unknown;
export const RegistryStatusSuccess = RegistryEntrySchema;

/** One registry's refresh: its status now and what the refresh found, or why it failed. */
export type RefreshResult =
  | { readonly id: string; readonly ok: true; readonly status: RegistryEntry; readonly report: RefreshReport }
  | { readonly id: string; readonly ok: false; readonly error: { readonly code: string; readonly message: string; readonly details: unknown; readonly fix?: string } };
export interface RegistryRefreshResult {
  /** A person asked: only then may anything install from what was found (Task 9b). */
  readonly byHuman: boolean;
  readonly results: ReadonlyArray<RefreshResult>;
  /** What the refreshes changed in the installed plugins. */
  readonly applied: RefreshApplied;
}
export const RegistryRefreshSuccess = Schema.Unknown;

export const PluginSourceSchema = Schema.Literals(["bundled", "registry", "dev"]);
export const PluginStateSchema = Schema.Literals(["active", "disabled", "revoked", "crashLoop"]);
export const PluginKindSchema = Schema.Literals(["content", "code"]);

/** An installed plugin (a `plugins` row; spec §5.1). */
export const InstalledRowSchema = Schema.Struct({
  id: Schema.String,
  version: Schema.String,
  source: PluginSourceSchema,
  registryId: Schema.optional(Schema.String),
  kind: PluginKindSchema,
  enabled: Schema.Boolean,
  state: PluginStateSchema,
  autoUpdate: Schema.Boolean,
  installedBy: Schema.String,
  installedAt: Schema.Number,
  /** Whole days its registry's metadata is past its expiry; absent while fresh or for other sources. */
  registryStaleDays: Schema.optional(Schema.Number),
});
export type InstalledRow = typeof InstalledRowSchema.Type;

export const PluginListPayload = Schema.Struct({});
export const PluginListSuccess = Schema.Struct({
  plugins: Schema.Array(InstalledRowSchema),
  /**
   * The plugins that come with the app (8A Task 10): whether their files match the hashes built
   * into it (`ok`; else `code: PLUGIN_BUNDLED_MISMATCH`), and how far that is trusted:
   * `hashesOnly` until sub-project 11 verifies the app's code signature (`signed`).
   */
  bundled: Schema.optional(
    Schema.Struct({
      integrity: Schema.Literals(["hashesOnly", "signed"]),
      plugins: Schema.Array(Schema.Struct({ id: Schema.String, version: Schema.String, ok: Schema.Boolean, code: Schema.optional(Schema.String) })),
    }),
  ),
  /**
   * Installed rows whose package folder is missing from the store (`storeMissing`, disabled at
   * start), and content packs whose files no longer validate (`contentInvalid`) or whose font
   * catalogs the server left out (`fontCatalogRejected`: an entry id taken, a file listed
   * twice; `code` says which), so their contributions are not in use.
   */
  problems: Schema.optional(
    Schema.Array(Schema.Struct({ id: Schema.String, version: Schema.String, reason: Schema.Literals(["storeMissing", "contentInvalid", "fontCatalogRejected"]), code: Schema.optional(Schema.String) })),
  ),
});

/** `plugin.search`: plugins the registries list; `registry` limits it to one. */
export const PluginSearchPayload = Schema.Struct({
  text: Schema.optional(Schema.String),
  kind: Schema.optional(PluginKindSchema),
  registry: Schema.optional(RegistryIdSchema),
});
export type PluginSearchRequest = typeof PluginSearchPayload.Type;
export const PluginSearchSuccess = Schema.Unknown;
export type PluginSearchResult = ReadonlyArray<Listing>;

export const PluginInfoPayload = Schema.Struct({ id: Schema.String });
/** What each registry signs about the plugin, and the installed row, if any. */
export interface PluginInfoResult {
  readonly id: string;
  readonly installed?: InstalledRow;
  readonly registries: ReadonlyArray<PluginInfo>;
}
export const PluginInfoSuccess = Schema.Unknown;
