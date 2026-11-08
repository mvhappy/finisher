// Types of `registry.js`: the plugin store and registries class (re-exported by `index.d.ts`).

/**
 * Makes the `Registry` class over the loader's `native` binding and error adapter; `index.js`
 * exports the result as `Registry`.
 */
export declare function defineRegistry(deps: {
  native: unknown;
  call: (p: Promise<unknown>) => Promise<unknown>;
  toFinError: (e: unknown) => unknown;
}): typeof Registry;

/**
 * A server's plugin store (`<serverData>/plugins`): install plans, commits, removals and the
 * staging sweep. Every call runs on the registry's worker thread, one at a time. Values are
 * JSON strings of the generated `@finisher/core-types` types. Nothing a package ships runs.
 */
export declare class Registry {
  private constructor();
  static open(serverData: string): Registry;
  /**
   * Makes a plan from `{ id, source: { kind: "registry", registry, version? }, update?,
   * allowDowngrade?, installed?: InstalledPlugin, knownIds? }` JSON; resolves to the
   * `InstallPlan` JSON. A registry source is resolved in the registry's trusted metadata and
   * fetched with its signed length and SHA-256 checked (`REGISTRY_NOT_FOUND`,
   * `PLUGIN_NOT_FOUND`, `TUF_TARGET_MISMATCH`); `{ kind: "bundled", root }` plans the
   * compiled-in plugin `id` from `<root>/<name>` (the app's resources) against the hash built
   * into this binary (`PLUGIN_BUNDLED_MISMATCH`, `PLUGIN_NOT_FOUND`) and installs only the
   * compiled-in version; test builds also take `{ kind: "package",
   * archive, registryId, staleDays?, yanked? }` and `testAgentFacing`.
   */
  plan(reqJson: string): Promise<string>;
  /**
   * Installs the plan with its digest (`Accept` JSON: `{ copyleft?, thirdPartyCode? }`;
   * `checksJson`: `{ installedDirs? }`, the installed plugins' package directories, whose font
   * catalog ids must still be free); resolves to the `InstalledPlugin` JSON or rejects with
   * `PLAN_EXPIRED`, `PLAN_MISMATCH` (`reason: installed` when an id was taken since),
   * `COPYLEFT_NOT_ACCEPTED` or `THIRD_PARTY_CODE_NOT_ACCEPTED`.
   */
  commit(planId: string, digest: string, acceptJson?: string, checksJson?: string): Promise<string>;
  /** The staged `InstallPlan` JSON; rejects with `PLAN_EXPIRED` once it is gone. */
  stagedPlan(planId: string): Promise<string>;
  /** Deletes a plan and its staged files; resolves to whether there was one. */
  discardPlan(planId: string): Promise<boolean>;
  /**
   * The installed versions (`[{ id, version }]` JSON) a `disable` advisory of registry `id`
   * applies to: `[{ id, version, advisory }]` JSON.
   */
  revocations(id: string, installedJson: string): Promise<string>;
  /** Deletes a version from the store (and the plugin's data with `purgeData`). */
  removePlugin(id: string, version: string, purgeData: boolean): Promise<void>;
  /** Deletes expired and broken plans; resolves to how many. */
  sweep(): Promise<number>;
  /**
   * Every version directory in the store, readable or not: `[{ id, version, dir }]` JSON, with
   * `dir` as `control.db` records it (`store/@<publisher>/<name>/<version>-<hash12>`).
   */
  installedDirs(): Promise<string>;
  /**
   * The contributions of the installed content pack in `dir` (as `control.db` records it):
   * `{ id, version, fontCatalogs: [text], captionStyles: [{ name, title?, style }] }` JSON,
   * every file validated again as at install (`PLUGIN_MANIFEST_INVALID {reason:
   * contentInvalid}` when one no longer is).
   */
  packContent(dir: string): Promise<string>;
  /**
   * One caption style of the installed content pack in `dir`: `{ id, version, available:
   * [name], found?: { name, title?, style } }` JSON. Only that style's file is read and
   * validated (`plugin.content.get` serves one style, not the pack).
   */
  packStyle(dir: string, name: string): Promise<string>;
  /** The bundled plugins compiled into this binary: `[{ id, version, treeHash }]` JSON. */
  bundledList(): string;
  /**
   * Re-hashes each bundled plugin's tree under `root` and compares it with the compiled-in
   * hash: `[{ id, version, treeHash, ok, code?, reason? }]` JSON (`code` is
   * `PLUGIN_BUNDLED_MISMATCH`; `reason` is `missing`, `unreadable` or `treeHash`).
   */
  bundledVerify(root: string): Promise<string>;
  /** Test builds only: replaces the compiled-in list for this `Registry`; `null` restores it. */
  setBundledForTest(listJson: string | null): void;
  /**
   * Every added registry's `RegistryStatus` as a `RegistryList` JSON (`{ registries,
   * problems }`): a registry that cannot be opened is a problem and hides no other.
   */
  registryList(): Promise<string>;
  /** One registry's `RegistryStatus` JSON; rejects with `REGISTRY_NOT_FOUND`. */
  registryStatus(id: string): Promise<string>;
  /**
   * Adds the local-folder registry `{ id, path }` (JSON) trusted through `rootJson`:
   * `{ kind: "file", path }` (a root file of the folder) or `{ kind: "sha256", sha256 }`.
   * Resolves to its `RegistryStatus` JSON; rejects with `REGISTRY_INVALID`,
   * `REGISTRY_EXISTS` or `REGISTRY_UNTRUSTED` (the root is none of the folder's; nothing is
   * written). Nothing is fetched: refresh next.
   */
  registryAdd(cfgJson: string, rootJson: string): Promise<string>;
  /** Deletes the registry's config; its trusted metadata stays (Decision 14). */
  registryRemove(id: string): Promise<void>;
  /** Deletes the trusted metadata of a root no registry uses. */
  registryForget(rootSha256: string): Promise<void>;
  /**
   * Refreshes the registry from its folder; resolves to `{ status, report }` JSON
   * (`RegistryStatus`, `RefreshReport`) or rejects with `TUF_*`, `CLOCK_SUSPECT`,
   * `REGISTRY_INVALID`. Journals `registry.refresh` with `byHuman`.
   */
  registryRefresh(id: string, byHuman: boolean): Promise<string>;
  /**
   * Plugins the registries list: `{ text?, kind?, registry? }` JSON; resolves to a `Listing`
   * array JSON.
   */
  search(queryJson: string): Promise<string>;
  /** What each registry signs about plugin `id`: a `PluginInfo` array JSON (empty when none). */
  info(id: string): Promise<string>;
}
