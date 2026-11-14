// Catalog extensions (plan 5D Decision 24, frozen): the tables of 8C's transcripts, 8E's chunks
// and 5D's own `core` schema live in each library's catalog, and each comes as a **module
// registered by URL**. Only the name and the URL cross a `MessagePort`: the indexer and the
// reader thread both `import()` the module at start, so no function ever crosses one.
//
// A module exports `{migrations, versions?, ops, queries}`:
// - `migrations`: the extension's frozen chain (`control/migrations.ts`' `Migration`), run by
//   the indexer when a catalog opens, `core` first and then the others by name; each chain has
//   its own `formatVersion.<ext>`/`minReaderVersion.<ext>` in the catalog's `meta`, and a
//   catalog whose `minReaderVersion.<ext>` is above this code's is `CATALOG_TOO_NEW`;
// - `ops[name](db, args)`: writes, run in the indexer inside one transaction (keep each one
//   ≤ 2,000 rows, Decision 2, so libraries interleave);
// - `queries[name](db, args)`: reads, run in the reader on its read-only connection.
// `args` and results are structured-clone data.
//
// Registration is a module side effect before the catalog threads start, as `registerChain`
// is before `openControlStore`; registering once they run throws.
import type { DatabaseSync } from "node:sqlite";
import { ChainRegistry, type ChainVersions, type Migration } from "../../control/migrations.ts";

export interface CatalogExtensionModule {
  readonly migrations: ReadonlyArray<Migration>;
  readonly versions?: ChainVersions;
  /** Writes: run in the indexer thread, inside a transaction. */
  readonly ops: Readonly<Record<string, (db: DatabaseSync, args: unknown) => unknown>>;
  /** Reads: run in the reader thread, on a read-only connection. */
  readonly queries: Readonly<Record<string, (db: DatabaseSync, args: unknown) => unknown>>;
}

/** What crosses a port: a name and a `file:` URL, as strings. */
export interface CatalogExtensionRef {
  readonly name: string;
  readonly url: string;
}

/** Extension names are chain names (`[a-z][a-z0-9]*`); op and query names are identifiers. */
export const EXTENSION_NAME = /^[a-z][a-z0-9]*$/;
export const MEMBER_NAME = /^[a-zA-Z][a-zA-Z0-9]{0,63}$/;

/** 5D's own schema, loaded the same way as every other extension. */
export const CORE_EXTENSION: CatalogExtensionRef = { name: "core", url: new URL("./core/index.ts", import.meta.url).href };

/** A set of extensions. The process-wide one is `CATALOG_EXTENSIONS`; tests make their own. */
export class CatalogExtensionRegistry {
  readonly #exts = new Map<string, string>([[CORE_EXTENSION.name, CORE_EXTENSION.url]]);
  #started = false;

  /** Registers `name` at `module`. The same pair again is a no-op; another URL under one name throws. */
  register(ext: { readonly name: string; readonly module: URL }): void {
    if (!EXTENSION_NAME.test(ext.name)) throw new Error(`catalog extension name ${JSON.stringify(ext.name)} must match ${EXTENSION_NAME}`);
    if (!(ext.module instanceof URL) || ext.module.protocol !== "file:") {
      throw new Error(`catalog extension ${ext.name}: module must be a file: URL`);
    }
    const known = this.#exts.get(ext.name);
    if (known === ext.module.href) return;
    if (known !== undefined) throw new Error(`catalog extension ${ext.name} is already registered at another URL`);
    if (this.#started) throw new Error(`catalog extension ${ext.name}: register before the catalog threads start`);
    this.#exts.set(ext.name, ext.module.href);
  }

  /** Marks the registry as in use by running threads and answers its refs, `core` first. */
  start(): ReadonlyArray<CatalogExtensionRef> {
    this.#started = true;
    return this.refs();
  }

  refs(): ReadonlyArray<CatalogExtensionRef> {
    const all = [...this.#exts].map(([name, url]) => ({ name, url }));
    return [...all.filter((e) => e.name === "core"), ...all.filter((e) => e.name !== "core").sort((a, b) => (a.name < b.name ? -1 : 1))];
  }
}

/** The process-wide extensions the catalog threads load unless given another registry. */
export const CATALOG_EXTENSIONS = new CatalogExtensionRegistry();

/** Registers a catalog extension with the process-wide registry (frozen for 8C and 8E). */
export function registerCatalogExtension(ext: { readonly name: string; readonly module: URL }): void {
  CATALOG_EXTENSIONS.register(ext);
}

export interface LoadedExtensions {
  readonly modules: ReadonlyMap<string, CatalogExtensionModule>;
  /** The extensions' migration chains, for `planMigrations`/`applyMigrations`. */
  readonly chains: ChainRegistry;
}

function checkMembers(ext: string, kind: "ops" | "queries", members: unknown): void {
  if (members === null || typeof members !== "object") throw new Error(`catalog extension ${ext}: ${kind} must be an object`);
  for (const [name, f] of Object.entries(members)) {
    if (!MEMBER_NAME.test(name)) throw new Error(`catalog extension ${ext}: ${kind} name ${JSON.stringify(name)} must match ${MEMBER_NAME}`);
    if (typeof f !== "function") throw new Error(`catalog extension ${ext}: ${kind}.${name} is not a function`);
  }
}

/** Imports every extension (in a catalog thread) and checks its exports. */
export async function loadExtensions(refs: ReadonlyArray<CatalogExtensionRef>): Promise<LoadedExtensions> {
  const modules = new Map<string, CatalogExtensionModule>();
  const chains = new ChainRegistry();
  for (const ref of refs) {
    if (!EXTENSION_NAME.test(ref.name)) throw new Error(`catalog extension name ${JSON.stringify(ref.name)} must match ${EXTENSION_NAME}`);
    const url = new URL(ref.url);
    if (url.protocol !== "file:") throw new Error(`catalog extension ${ref.name}: module must be a file: URL`);
    const mod = (await import(url.href)) as Partial<CatalogExtensionModule>;
    if (!Array.isArray(mod.migrations)) throw new Error(`catalog extension ${ref.name}: migrations must be an array`);
    checkMembers(ref.name, "ops", mod.ops);
    checkMembers(ref.name, "queries", mod.queries);
    const ext = mod as CatalogExtensionModule;
    chains.register(ref.name, ext.migrations, ext.versions ?? {});
    modules.set(ref.name, ext);
  }
  if (!modules.has("core")) throw new Error("catalog extensions: core is not loaded");
  return { modules, chains };
}
