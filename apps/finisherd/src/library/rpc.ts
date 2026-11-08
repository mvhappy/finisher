// The `library.*` RPCs (plan 5D Task 13): the handlers `handlers.ts` takes as call sites. Every
// one passes the chain first (scopes, human-only and typed confirmation: `scopes.ts`), so these
// only check what the chain cannot: that a library exists and is in the caller's reach (an
// unknown library and one out of reach answer the same `LIBRARY_NOT_FOUND`), that a path
// below a root is a plain relative path, and what each call's state allows.
//
// Reads go to the reader thread through the service (`fresh.ts`: a listing or a search of a
// network or sync library first runs its freshness pass, bounded to 5 s, and says `stale`).
// The changes are `admin.ts`'s; the registry writes their audit rows (`library.add`, `remove`,
// `set` with human-only fields), which carry the library's id and storage kind and never a
// name, root, alias or path. Nothing here journals: the service's `library.scan` events do.
import type {
  FinError,
  JobView,
  LibraryAddRequest,
  LibraryFilePayload,
  LibraryFileResult,
  LibraryFilesPayload,
  LibraryLookupPayload,
  LibraryRemovePayload,
  LibraryScanPayload,
  LibrarySearchPayload,
  LibrarySetRequest,
  LibraryStatusPayload,
  LibraryWatchPayload,
  Principal,
} from "@finisher/contracts";
import { FinError as FinErrorClass, type LibraryFilesResult } from "@finisher/contracts";
import { Effect, Stream } from "effect";
import { chainOf } from "../chain/chain.ts";
import type { JobEngine } from "../jobs/api.ts";
import { ANONYMOUS } from "../principal.ts";
import { toFinError } from "../errors.ts";
import { acceptVolume, addLibrary, removeLibrary, scanLibrary, setLibrary } from "./admin.ts";
import type { FileState } from "./api.ts";
import { filesFresh, searchFresh } from "./fresh.ts";
import { lookupLibraries } from "./lookup.ts";
import { relComponents, relKeyOf } from "./paths.ts";
import { libraryAny, libraryOf, reachable } from "./reach.ts";
import type { LibraryPatch, LibraryRow, NewLibrary } from "./registry.ts";
import type { LibraryService } from "./service.ts";
import { childDisplay, decodeCursor, dirShape, encodeCursor, fileShape, foldStages, publicJob, withDirWarnings } from "./shape.ts";
import { watchLibraries } from "./watch-stream.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface LibraryRpcDeps {
  readonly service: LibraryService;
  readonly engine: Pick<JobEngine, "events">;
  /** `handlers.ts`' `guard`: defects become `INTERNAL`, the call is timed into `rpc.stats`. */
  readonly guard: Guard;
}

/** The chain's principal (`anonymous` outside it: reaches nothing). */
const principalOf = (options: object): Principal => chainOf(options)?.principal ?? ANONYMOUS;

const invalid = (field: string, why: string): FinError => new FinErrorClass({ code: "INVALID_REQUEST", message: `${field}: ${why}`, details: { field } });
const notFound = (what: string, details: unknown): FinError => new FinErrorClass({ code: "FILE_NOT_FOUND", message: `the library has no such ${what}`, details, fix: "Check the path, or let the library scan once more." });

/** `o` without its undefined fields (what `exactOptionalPropertyTypes` wants of an options object). */
const defined = <T extends object>(o: T): { [K in keyof T]?: Exclude<T[K], undefined> } =>
  Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as { [K in keyof T]?: Exclude<T[K], undefined> };

/** The libraries a call covers: the named ones (reach checked), else every one the caller reaches whose catalog is usable. */
function covered(svc: LibraryService, principal: Principal, refs: ReadonlyArray<string> | undefined): Effect.Effect<string[], FinError> {
  if (refs !== undefined) return Effect.map(Effect.forEach(refs, (r) => libraryOf(svc, principal, r)), (rows) => [...new Set(rows.map((l) => l.id))]);
  const usable = (l: LibraryRow) => !(l.state === "offline" && l.offlineReason?.startsWith("CATALOG_"));
  return Effect.succeed(reachable(svc, principal).filter(usable).map((l) => l.id));
}

const rng = (r: { readonly min?: number | undefined; readonly max?: number | undefined } | undefined) => (r === undefined ? undefined : defined(r));

const run = <A>(f: () => Promise<A>): Effect.Effect<A, FinError> => Effect.tryPromise({ try: f, catch: toFinError });

export function libraryHandlers({ service: svc, engine, guard }: LibraryRpcDeps) {
  const reader = svc.deps.catalogs.reader;
  const actorOf = (options: object) => {
    const p = principalOf(options);
    return { kind: p.kind, id: p.id };
  };
  const shown = (row: LibraryRow) => withDirWarnings(svc, svc.view(row));
  /** A directory `dir` (a path below the root) as the catalog's `files`/`dirRel` reference. */
  const dirRef = (lib: LibraryRow, dir: string | number | undefined): Effect.Effect<{ dir: string } | { dirId: number }, FinError> =>
    Effect.try({
      try: () => {
        if (typeof dir === "number") return { dirId: dir };
        const parts = dir === undefined ? [] : relComponents(lib, dir);
        return parts.length === 0 ? { dirId: 1 } : { dir: relKeyOf(lib, parts) };
      },
      catch: toFinError,
    });

  return {
    // The owner's changes (human-only): the registry audits them.
    "library.add": (req: LibraryAddRequest, options: object) =>
      guard(
        "library.add",
        run(async () => {
          const { challenge: _c, ...input } = req;
          const { library, job } = await addLibrary(svc, defined(input) as NewLibrary, actorOf(options));
          return { library: await shown(svc.registry.get(library.id) ?? (library as unknown as LibraryRow)), ...(job === undefined ? {} : { job: publicJob(job) }) };
        }),
      ),
    "library.remove": ({ id }: typeof LibraryRemovePayload.Type, options: object) =>
      guard(
        "library.remove",
        Effect.flatMap(libraryAny(svc, id), (lib) => run(() => removeLibrary(svc, svc.registry.serverData, lib.id, actorOf(options)).then(() => ({})))),
      ),
    "library.set": (req: LibrarySetRequest, options: object) =>
      guard(
        "library.set",
        Effect.flatMap(libraryAny(svc, req.id), (lib) =>
          run(async () => {
            const { id: _i, challenge: _c, acceptVolume: accept, ...fields } = req;
            const patch = defined(fields) as LibraryPatch;
            if (Object.keys(patch).length > 0) await setLibrary(svc, lib.id, patch, actorOf(options));
            if (accept === true) await acceptVolume(svc, lib.id, actorOf(options));
            return { library: await shown(svc.registry.get(lib.id) ?? lib) };
          }),
        ),
      ),
    "library.list": (_: object, options: object) =>
      guard("library.list", run(async () => ({ libraries: await Promise.all(reachable(svc, principalOf(options)).map(shown)) }))),
    "library.status": ({ id }: typeof LibraryStatusPayload.Type, options: object) =>
      guard(
        "library.status",
        Effect.gen(function* () {
          const lib = yield* libraryOf(svc, principalOf(options), id);
          const s = svc.status(lib.id);
          const stages = yield* (reader.stageCounts(lib.id)).pipe(Effect.catch(() => Effect.succeed({ stages: [] })));
          return {
            library: yield* run(() => shown(lib)),
            jobs: s.jobs,
            stages: foldStages(stages.stages),
            watch: s.watch,
            ...(s.watchReason === undefined ? {} : { watchReason: s.watchReason }),
            noChangePolls: s.noChangePolls,
            inUse: s.inUse,
          };
        }),
      ),
    "library.files": (req: typeof LibraryFilesPayload.Type, options: object) =>
      guard(
        "library.files",
        Effect.gen(function* () {
          const lib = yield* libraryOf(svc, principalOf(options), req.id);
          const limit = req.limit ?? 100;
          if (!Number.isSafeInteger(limit) || limit < 1 || limit > 500) return yield* Effect.fail(invalid("limit", "1–500"));
          const after = req.cursor === undefined ? undefined : yield* Effect.try({ try: () => decodeCursor(req.cursor!), catch: toFinError });
          const ref = yield* dirRef(lib, req.dir);
          const a = yield* run(() => filesFresh(svc, lib.id, { ...ref, ...defined({ state: req.state, kind: req.kind, after }), limit }));
          if (a.matches !== undefined && a.matches.length > 1) {
            return yield* Effect.fail(new FinErrorClass({ code: "PATH_AMBIGUOUS", message: "several folders in the library have this path", details: { dirIds: a.matches.map((d) => d.id) } }));
          }
          if (a.dir === undefined) return yield* Effect.fail(notFound("folder", { dir: req.dir ?? null }));
          const here = ((yield* (reader.query(lib.id, "core", "dirDisplay", { dirId: a.dir.id }))) as { display: string }).display;
          const out: LibraryFilesResult = {
            dir: dirShape(a.dir, here),
            dirs: a.dirs.map((d) => dirShape(d, childDisplay(here, d.nameNfc))),
            files: a.files.map((f) => fileShape(f, childDisplay(here, f.nameNfc))),
            ...(a.next === undefined ? {} : { next: encodeCursor(a.next) }),
            stale: a.stale,
            offline: a.offline,
            ...(a.lastPollAt === undefined ? {} : { lastPollAt: a.lastPollAt }),
          };
          return out;
        }),
      ),
    "library.file": (req: typeof LibraryFilePayload.Type, options: object) =>
      guard(
        "library.file",
        Effect.gen(function* () {
          const lib = yield* libraryOf(svc, principalOf(options), req.id);
          if ((req.path === undefined) === (req.fileId === undefined)) return yield* Effect.fail(invalid("file", "give a path or a fileId"));
          const key = req.path === undefined ? undefined : yield* Effect.try({ try: () => relKeyOf(lib, relComponents(lib, req.path!)), catch: toFinError });
          if (req.path !== undefined && key === "") return yield* Effect.fail(invalid("path", "name a file"));
          const found = yield* (reader.file(lib.id, key === undefined ? { fileId: req.fileId! } : { relKey: key }));
          const live = found.matches.filter((m) => m.state !== "missing");
          if (live.length > 1) {
            return yield* Effect.fail(new FinErrorClass({ code: "PATH_AMBIGUOUS", message: "several files in the library have this path", details: { fileIds: live.map((m) => m.id) }, fix: "Ask for the file by its id." }));
          }
          const m = live[0] ?? found.matches[0];
          if (m === undefined) return yield* Effect.fail(notFound("file", {}));
          const file: LibraryFileResult = fileShape(m, m.display);
          return { file, stages: found.matches.length === 1 ? found.stages : [] };
        }),
      ),
    "library.search": (req: typeof LibrarySearchPayload.Type, options: object) =>
      guard(
        "library.search",
        Effect.gen(function* () {
          const libraries = yield* covered(svc, principalOf(options), req.libraries);
          if (libraries.length === 0) return { hits: [], truncated: false, stale: false };
          const r = req;
          const res = yield* run(() => searchFresh(svc, { libraries, ...defined({ query: r.query, kind: r.kind, duration: rng(r.duration), width: rng(r.width), height: rng(r.height), since: r.since, state: r.state, cursor: r.cursor, limit: r.limit }) }));
          return { hits: res.hits as ReadonlyArray<never>, truncated: res.truncated, stale: res.stale, ...(res.next === undefined ? {} : { next: res.next }), ...(res.lastPollAt === undefined ? {} : { lastPollAt: res.lastPollAt }) };
        }),
      ),
    "library.lookup": (req: typeof LibraryLookupPayload.Type, options: object) =>
      guard(
        "library.lookup",
        Effect.gen(function* () {
          const libraries = yield* covered(svc, principalOf(options), req.libraries);
          if (libraries.length === 0) return { matches: [] };
          const res = yield* lookupLibraries(reader, { libraries, ...defined({ fingerprints: req.fingerprints, hashes: req.hashes, hashCandidates: req.hashCandidates }) }, (lib, ids) => svc.hash(lib, ids));
          return {
            matches: res.matches as ReadonlyArray<never>,
            ...(res.pending === undefined ? {} : { pending: { job: res.pending.job, jobs: res.pending.jobs, more: res.pending.more } }),
          };
        }),
      ),
    "library.scan": ({ id, full, dirs }: typeof LibraryScanPayload.Type) =>
      guard(
        "library.scan",
        Effect.gen(function* () {
          const lib = yield* libraryAny(svc, id);
          const rels: Array<{ components: string[] }> = [];
          for (const d of dirs ?? []) {
            const ref = yield* dirRef(lib, d);
            const r = (yield* (reader.query(lib.id, "core", "dirRel", ref))) as { rel?: { components: string[] } };
            if (r.rel === undefined) return yield* Effect.fail(notFound("folder", {}));
            rels.push(r.rel);
          }
          const job: JobView = yield* run(() => scanLibrary(svc, lib.id, { ...(full === true ? { full } : {}), ...(rels.length > 0 ? { dirs: rels } : {}) }));
          return { job: publicJob(job) };
        }),
      ),
    // Errors travel in the stream's error channel. The chain checked `library.read`.
    "library.watch": ({ ids }: typeof LibraryWatchPayload.Type, options: object) =>
      Stream.unwrap(Effect.map(covered(svc, principalOf(options), ids), (libraries) => watchLibraries(svc, engine, libraries))).pipe(
        Stream.catchDefect((defect) => Stream.fail(toFinError(defect))),
      ),
  };
}
