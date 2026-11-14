// `library.lookup`, main-thread side (plan 5D Task 12, Decision 44): exact matches of
// fingerprints (`fp1:…`, from the probe) or full hashes (`blake3:<hex>`) across the given
// libraries. With `hashCandidates`, the present rows of every library that have the size of a
// match but no full hash are handed to the hash service in **one job per library**
// (`Libraries.hash`, which Task 10/11 provides), so a later lookup by hash finds their twins.
import { FinError, type JobView } from "@finisher/contracts";
import { Effect } from "effect";
import type { ReaderClient } from "./catalog/client.ts";

export const MAX_KEYS = 1000;
/** Rows handed to one library's hash job; `pending.more` says the cap cut the list. */
export const MAX_CANDIDATES = 1000;

export interface LookupRequest {
  /** The libraries to look in: the caller has resolved reach. */
  readonly libraries: ReadonlyArray<string>;
  readonly fingerprints?: ReadonlyArray<string>;
  readonly hashes?: ReadonlyArray<string>;
  readonly hashCandidates?: boolean;
}

export interface LookupMatch {
  /** The requested value that matched. */
  readonly key: string;
  readonly library: string;
  readonly fileId: number;
  readonly relKey: string;
  readonly display: string;
  readonly state: string;
}

export interface LookupResult {
  readonly matches: ReadonlyArray<LookupMatch>;
  /** The hash jobs started for `hashCandidates`: `job` the first, `jobs` one per library. */
  readonly pending?: { readonly job: string; readonly jobs: ReadonlyArray<{ readonly library: string; readonly job: string }>; readonly more: boolean };
}

/** `Libraries.hash` of the frozen interfaces. */
export type HashSubmit = (library: string, fileIds: ReadonlyArray<number>) => Effect.Effect<JobView, FinError>;

const invalid = (field: string, why: string): FinError => new FinError({ code: "INVALID_REQUEST", message: `${field}: ${why}`, details: { field } });
const HASH = /^blake3:[0-9a-f]{64}$/;

export function lookupLibraries(reader: ReaderClient, req: LookupRequest, hash?: HashSubmit): Effect.Effect<LookupResult, FinError> {
  return Effect.gen(function* () {
    if ((req.fingerprints === undefined) === (req.hashes === undefined)) return yield* Effect.fail(invalid("lookup", "give fingerprints or hashes"));
    if (req.libraries.length === 0) return yield* Effect.fail(invalid("libraries", "name at least one library"));
    const field = req.fingerprints === undefined ? "hashes" : "fingerprints";
    const keys = [...new Set(req.fingerprints ?? req.hashes)];
    if (keys.length === 0 || keys.length > MAX_KEYS) return yield* Effect.fail(invalid(field, `1–${MAX_KEYS} values`));
    if (keys.some((k) => typeof k !== "string" || k.length === 0 || k.length > 200 || (field === "hashes" && !HASH.test(k)))) {
      return yield* Effect.fail(invalid(field, field === "hashes" ? "blake3:<64 hex digits>" : "non-empty strings"));
    }
    if (req.hashCandidates === true && hash === undefined) return yield* Effect.fail(invalid("hashCandidates", "the hash service is not available"));
    const order = new Map(keys.map((k, i) => [k, i]));
    const found: Array<LookupMatch & { size: number }> = [];
    for (const library of req.libraries) {
      const r = yield* reader.lookup(library, { [field]: keys });
      for (const m of r.matches) found.push({ key: m.key, library, fileId: m.file.id, relKey: m.file.relKey, display: m.display, state: m.file.state, size: m.file.size });
    }
    found.sort((a, b) => order.get(a.key)! - order.get(b.key)! || req.libraries.indexOf(a.library) - req.libraries.indexOf(b.library) || a.fileId - b.fileId);
    const matches = found.map(({ size: _size, ...m }) => m);
    if (req.hashCandidates !== true || found.length === 0) return { matches };
    const sizes = [...new Set(found.map((m) => m.size))].slice(0, MAX_KEYS);
    const jobs: Array<{ library: string; job: string }> = [];
    let more = false;
    for (const library of req.libraries) {
      const c = yield* reader.hashCandidates(library, { sizes, limit: MAX_CANDIDATES });
      more ||= c.more;
      if (c.fileIds.length === 0) continue;
      jobs.push({ library, job: (yield* hash!(library, c.fileIds)).id });
    }
    return jobs.length === 0 ? { matches } : { matches, pending: { job: jobs[0]!.job, jobs, more } };
  });
}
