// Answers carry freshness (plan 5D Decisions 28, 46; the freshness part of Task 12). Before a
// search or listing of a network or sync library whose last poll is older than 5 minutes, the
// service submits an `interactive` poll of what the answer touches (a listing: that directory,
// as a targeted scan; a search: the whole library, as a poll) and waits up to 5 s. Past that
// the answer says `stale: true` and the poll goes on. Polls run on the volume's poll pool, so a
// full scan never holds them up. Local roots are watched and answer at once. Every search and
// listing counts as use (Decision 28). Later pages of a search slice its snapshot and run no
// pass.
import type { RawRel } from "@finisher/core-types";
import type { DirRow, FileRow } from "./catalog/core/queries.ts";
import { type SearchHit, type SearchRequest, type SearchResult, searchLibraries } from "./search.ts";
import { type LibraryService, exec } from "./service.ts";

export const FRESH_MS = 5 * 60_000;
export const FRESH_WAIT_MS = 5_000;

export interface Freshness {
  /** A freshness pass did not finish within 5 s (or could not run). */
  readonly stale: boolean;
  /** The oldest last poll (or full scan) of the libraries answered, when any has one. */
  readonly lastPollAt?: number;
}

/**
 * Runs the freshness pass over `ids` (Decision 28). `dir(lib)` names the directory a listing
 * shows (a targeted scan of it); without it a poll of the whole library runs.
 */
export async function freshen(svc: LibraryService, ids: ReadonlyArray<string>, dir?: (lib: string) => Promise<RawRel | undefined>): Promise<Freshness> {
  const jobs: string[] = [];
  let stale = false;
  const libs = [...new Set(ids)];
  for (const id of libs) {
    const row = svc.registry.get(id);
    if (row === undefined) continue;
    svc.usage.touch(id);
    const last = row.lastPollAt ?? row.lastFullAt;
    if (row.storage === "local" || row.state === "offline" || row.state === "paused") continue;
    if (last !== undefined && svc.now() - last < FRESH_MS) continue;
    const rel = dir === undefined ? undefined : await dir(id).catch(() => undefined);
    const job = await svc.submitScan(
      id,
      rel === undefined
        ? { mode: "poll", trigger: "demand", priority: "interactive" }
        : { mode: "dirs", dirs: [rel], trigger: "demand", priority: "interactive" },
    );
    if (job === undefined) stale = true;
    else if (job.state !== "succeeded") jobs.push(job.id);
  }
  if (jobs.length > 0) {
    try {
      const views = await exec(svc.deps.engine.wait(jobs, { timeoutMs: FRESH_WAIT_MS }));
      if (views.some((v) => v.state !== "succeeded")) stale = true;
    } catch {
      stale = true; // WAIT_TIMEOUT: the poll goes on, the answer is what the catalog holds now.
    }
  }
  let lastPollAt: number | undefined;
  for (const id of libs) {
    const row = svc.registry.get(id);
    const last = row?.lastPollAt ?? row?.lastFullAt;
    if (last !== undefined) lastPollAt = Math.min(lastPollAt ?? last, last);
  }
  return { stale, ...(lastPollAt === undefined ? {} : { lastPollAt }) };
}

/** `library.search` with freshness: hits of an offline library say so. */
export async function searchFresh(svc: LibraryService, req: SearchRequest): Promise<SearchResult & Freshness> {
  const fresh = req.cursor === undefined ? await freshen(svc, req.libraries) : await freshen(svc, []);
  const page = await exec(searchLibraries(svc.deps.catalogs.reader, req));
  const offline = new Set(req.libraries.filter((id) => svc.registry.get(id)?.state === "offline"));
  const hits: SearchHit[] = page.hits.map((h) => (offline.has(h.library) && !h.offline ? { ...h, offline: true } : h));
  return { ...page, hits, ...fresh };
}

export interface FilesArgs {
  /** The directory's `relKey` (else `dirId`, default the root). */
  readonly dir?: string;
  readonly dirId?: number;
  readonly state?: string;
  readonly kind?: string;
  readonly after?: { readonly nameNfc: string; readonly id: number };
  readonly limit: number;
}

export interface FilesAnswer extends Freshness {
  readonly dir?: DirRow;
  readonly matches?: DirRow[];
  readonly dirs: DirRow[];
  readonly files: FileRow[];
  readonly next?: { nameNfc: string; id: number };
  readonly offline: boolean;
}

/** `library.files` with freshness: the first page of a listing lists its directory first. */
export async function filesFresh(svc: LibraryService, lib: string, args: FilesArgs): Promise<FilesAnswer> {
  const { reader } = svc.deps.catalogs;
  const ref = args.dir !== undefined ? { dir: args.dir } : { dirId: args.dirId ?? 1 };
  const dir = async (id: string): Promise<RawRel | undefined> => ((await exec(reader.query(id, "core", "dirRel", ref))) as { rel?: RawRel }).rel;
  const fresh = args.after === undefined ? await freshen(svc, [lib], dir) : await freshen(svc, []);
  const answer = await exec(reader.files(lib, { ...args }));
  return { ...answer, ...fresh, offline: svc.registry.get(lib)?.state === "offline" };
}
