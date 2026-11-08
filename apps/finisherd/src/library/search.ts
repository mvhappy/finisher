// `library.search`, main-thread side (plan 5D Task 12, Decisions 43, 46): checks the request,
// asks the reader thread for one page and shapes the hits. The ranking, the snapshots and the
// keyset live in the reader thread (`catalog/core/{search,snapshot}.ts`); nothing here opens a
// catalog or runs a statement.
//
// Query semantics: the query is taken in NFC and split as SQLite's `unicode61` splits names
// (runs of letters, numbers, marks and private-use characters; everything else separates), so
// `interview_take3.final.mov` is `interview`, `take3`, `final`, `mov`. Every term is a prefix
// term (`"interv"*`) over the file name and its directory path; all terms must match, in
// either column. Case and diacritics are ignored (`cafe` finds `Café`, composed or
// decomposed). Words are not segmented in scripts without spaces: a Han or kana name is one
// term, found by its prefix only. A query without terms lists by `lastChange`, newest first.
import { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import type { ReaderClient } from "./catalog/client.ts";
import type { Hit } from "./catalog/core/search.ts";
import { MAX_PAGE, SNAPSHOT_HITS } from "./catalog/core/snapshot.ts";

export { MAX_PAGE, SNAPSHOT_HITS };
export const DEFAULT_PAGE = 50;

export interface Range {
  readonly min?: number;
  readonly max?: number;
}

export interface SearchRequest {
  readonly query?: string;
  /** The libraries to search: the caller has resolved reach and defaults. */
  readonly libraries: ReadonlyArray<string>;
  readonly kind?: string;
  readonly duration?: Range;
  readonly width?: Range;
  readonly height?: Range;
  /** Bytes. */
  readonly size?: Range;
  /** `lastChange ≥ since` (catalog time, ms). */
  readonly since?: number;
  /** One file state; without it `missing` rows are left out. */
  readonly state?: string;
  readonly cursor?: string;
  /** 1–200, default 50. */
  readonly limit?: number;
}

/** `SearchHit` of the frozen interfaces (`state` is a `FileState`; `score` is bm25, lower is better). */
export interface SearchHit {
  readonly library: string;
  readonly fileId: number;
  readonly relKey: string;
  readonly display: string;
  readonly kind?: string;
  readonly size: number;
  readonly durationMs?: number;
  readonly width?: number;
  readonly height?: number;
  readonly fp?: string;
  readonly state: string;
  readonly collision: boolean;
  readonly score: number;
  readonly offline: boolean;
}

export interface SearchResult {
  readonly hits: ReadonlyArray<SearchHit>;
  readonly next?: string;
  /** More than 2,000 files matched; only the best 2,000 can be paged. */
  readonly truncated: boolean;
}

const invalid = (field: string, why: string): FinError => new FinError({ code: "INVALID_REQUEST", message: `${field}: ${why}`, details: { field } });
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);

function checkRange(field: string, r: Range | undefined): Effect.Effect<void, FinError> {
  if (r === undefined) return Effect.void;
  if ((r.min !== undefined && !isInt(r.min)) || (r.max !== undefined && !isInt(r.max))) return Effect.fail(invalid(field, "min and max are integers"));
  if (r.min !== undefined && r.max !== undefined && r.min > r.max) return Effect.fail(invalid(field, "min is above max"));
  return Effect.void;
}

const shape = ({ lastChange: _t, ...hit }: Hit): SearchHit => hit;

/** One page of a name search across `req.libraries` (Decision 43). */
export function searchLibraries(reader: ReaderClient, req: SearchRequest): Effect.Effect<SearchResult, FinError> {
  return Effect.gen(function* () {
    const limit = req.limit ?? DEFAULT_PAGE;
    if (!isInt(limit) || limit < 1 || limit > MAX_PAGE) return yield* Effect.fail(invalid("limit", `1–${MAX_PAGE}`));
    if (req.libraries.length === 0) return yield* Effect.fail(invalid("libraries", "name at least one library"));
    if (req.since !== undefined && !isInt(req.since)) return yield* Effect.fail(invalid("since", "an integer"));
    for (const [field, r] of [["duration", req.duration], ["width", req.width], ["height", req.height], ["size", req.size]] as const) yield* checkRange(field, r);
    const page = yield* reader.searchPage({
      libraries: req.libraries,
      limit,
      ...(req.cursor === undefined ? {} : { cursor: req.cursor }),
      args: {
        ...(req.query === undefined ? {} : { query: req.query }),
        ...(req.kind === undefined ? {} : { kind: req.kind }),
        ...(req.state === undefined ? {} : { state: req.state }),
        ...(req.since === undefined ? {} : { since: req.since }),
        ...(req.duration === undefined ? {} : { durationMs: req.duration }),
        ...(req.width === undefined ? {} : { width: req.width }),
        ...(req.height === undefined ? {} : { height: req.height }),
        ...(req.size === undefined ? {} : { size: req.size }),
      },
    });
    return { hits: page.hits.map(shape), truncated: page.truncated, ...(page.next === undefined ? {} : { next: page.next }) };
  });
}
