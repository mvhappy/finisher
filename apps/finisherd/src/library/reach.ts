// Who reaches which library, and which library files an `asset.probe` may read (plan 5D Task
// 13, Decision 47).
//
// **Library reach** in v1: a principal holding `library.read` reaches every library of this
// server (credentials carry projects, not libraries); `libraryReach` is the one function every
// caller uses, so a later per-library grant changes one place. A library a principal does not
// reach is answered exactly like one that does not exist (`LIBRARY_NOT_FOUND`).
//
// **Asset reach to a library file** is granted by a catalogued row, never by a path prefix and
// never through an alias: the path must lie under a library's own root and find exactly one
// live row in that catalog (several: `PATH_AMBIGUOUS`; none: no reach). This is deliberately
// not `Libraries.locate`, which also resolves aliases, fingerprints and hashes. The file is
// then read through `open_for_read` with that row's raw path and `expect` (`probe.ts`).
import { FinError, type Principal } from "@finisher/contracts";
import { Effect } from "effect";
import type { ExpectWire, FileState, RawRelWire } from "./api.ts";
import { relUnder } from "./locate.ts";
import { relKeyOf } from "./paths.ts";
import type { LibraryRow } from "./registry.ts";
import type { LibraryService } from "./service.ts";
import { exec } from "./states.ts";

/** Whether `principal` reaches library `_id` (v1: it holds `library.read`). */
export function libraryReach(principal: Pick<Principal, "scopes">, _id: string): boolean {
  return principal.scopes.includes("library.read");
}

/** The libraries `principal` reaches, in creation order. */
export function reachable(svc: Pick<LibraryService, "registry">, principal: Pick<Principal, "scopes">): LibraryRow[] {
  return svc.registry.list().filter((l) => libraryReach(principal, l.id));
}

const notFound = (ref: string): FinError =>
  new FinError({ code: "LIBRARY_NOT_FOUND", message: "there is no such library", details: { id: ref }, fix: "Check the library's id or name." });

/** A library by id or name (`LIBRARY_NOT_FOUND` when unknown or out of reach). */
export function libraryOf(svc: Pick<LibraryService, "registry">, principal: Pick<Principal, "scopes">, ref: string): Effect.Effect<LibraryRow, FinError> {
  return Effect.suspend(() => {
    const row = svc.registry.resolve(ref);
    return row === undefined || !libraryReach(principal, row.id) ? Effect.fail(notFound(ref)) : Effect.succeed(row);
  });
}

/** A library by id or name for a call that needs no read reach (its scopes were checked). */
export function libraryAny(svc: Pick<LibraryService, "registry">, ref: string): Effect.Effect<LibraryRow, FinError> {
  return Effect.suspend(() => {
    const row = svc.registry.resolve(ref);
    return row === undefined ? Effect.fail(notFound(ref)) : Effect.succeed(row);
  });
}

/** One catalogued file, as `asset.probe` opens it. */
export interface CataloguedFile {
  readonly library: string;
  readonly root: string;
  readonly fileId: number;
  readonly rel: RawRelWire;
  readonly display: string;
  readonly expect: ExpectWire;
  readonly hydrate: boolean;
  readonly state: FileState;
  readonly offline: boolean;
}

/**
 * The one live catalogued row at the absolute `path` under a library's own root (not an
 * alias), or `undefined`; several rows → `PATH_AMBIGUOUS`. A library whose catalog is not open
 * has no rows. `path` is canonical (the chain resolved it).
 */
export function cataloguedAt(svc: LibraryService, path: string): Effect.Effect<CataloguedFile | undefined, FinError> {
  return Effect.tryPromise({
    try: async () => {
      let hit: { lib: LibraryRow; parts: string[] } | undefined;
      for (const lib of svc.registry.list()) {
        const parts = relUnder(lib.root, path);
        if (parts !== undefined && (hit === undefined || lib.root.length > hit.lib.root.length)) hit = { lib, parts };
      }
      if (hit === undefined) return undefined;
      const { lib } = hit;
      const key = relKeyOf(lib, hit.parts.map((c) => c.normalize("NFC")));
      let matches;
      try {
        ({ matches } = await exec(svc.deps.catalogs.reader.file(lib.id, { relKey: key })));
      } catch {
        return undefined;
      }
      const live = matches.filter((m) => m.state !== "missing");
      if (live.length > 1) {
        throw new FinError({ code: "PATH_AMBIGUOUS", message: "several files in the library have this path", details: { library: lib.id, fileIds: live.map((m) => m.id) } });
      }
      const f = live[0];
      if (f === undefined) return undefined;
      return {
        library: lib.id,
        root: lib.root,
        fileId: f.id,
        rel: f.rel,
        display: f.display,
        expect: { size: f.size, mtimeNs: f.mtimeNs, ...(f.fileId === null ? {} : { fileId: f.fileId }), ...(f.dev === null ? {} : { dev: f.dev }) },
        hydrate: lib.hydrate,
        state: f.state as FileState,
        offline: lib.state === "offline" || f.state === "offline",
      };
    },
    catch: (e) => (e instanceof FinError ? e : new FinError({ code: "INTERNAL", message: String(e), details: {} })),
  });
}
