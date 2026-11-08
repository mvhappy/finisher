// What the library service tells others (plan 5D Decision 50, D34): `library.scan` journal
// events, and file events for sub-projects 6 and 8 (`Libraries.onFile`).
//
// **Journal.** One `library.scan` per full scan, targeted scan and watch rescan, and per poll
// that found changes or did not succeed; a poll that found nothing is only counted
// (`library.status`), so a 60 s poll does not write 1,440 events a day. The payload is closed
// and names nothing: no library id, name, root or path; library sizes are log₂ buckets
// (`@finisher/core-types` `LibraryScan`, checked by the journal itself). A refused event is
// logged, never thrown.
import type { LibraryScan, ScanTrigger } from "@finisher/core-types";
import type { Journal } from "@finisher/fin-node";
import type { FileEvent } from "./api.ts";
import type { CatalogCounts, ProbedApplied } from "./catalog/client.ts";
import type { FileChanges } from "./catalog/core/file-events.ts";
import type { ApplyCounts } from "./catalog/core/apply.ts";
import type { LibraryStorage } from "./registry.ts";

export type JournalLike = Pick<Journal, "enabled" | "record">;
export type ScanMode = "full" | "dirs" | "poll";
export type ScanOutcome = LibraryScan["outcome"];

/** `n` as a log₂ bucket: 0 for 0, else `floor(log2(n)) + 1` (as `fin_core`'s `log2_bucket`). */
export function log2Bucket(n: number): number {
  let b = 0;
  for (let v = Math.max(0, Math.floor(n)); v > 0; v = Math.floor(v / 2)) b++;
  return b;
}

/** The rows a scan changed (what makes a poll worth a journal event). */
export function changesOf(a: ApplyCounts): number {
  return a.added + a.changed + a.missing + a.renamed + a.reappeared;
}

/** Decision 50: every full, targeted and watch scan; a poll only when it changed something or did not succeed. */
export function journaled(mode: ScanMode, outcome: ScanOutcome, applied: ApplyCounts): boolean {
  return mode !== "poll" || outcome !== "succeeded" || changesOf(applied) > 0;
}

const live = (by: Record<string, number>): number => Object.entries(by).reduce((n, [state, k]) => (state === "missing" ? n : n + k), 0);

export interface ScanSummary {
  readonly storage: LibraryStorage;
  readonly mode: ScanMode;
  readonly trigger: ScanTrigger;
  readonly outcome: ScanOutcome;
  readonly code?: string;
  readonly applied: ApplyCounts;
  /** The catalog after the scan (absent when it could not be read). */
  readonly counts?: CatalogCounts;
  readonly micros: number;
}

const CODE = /^[A-Z][A-Z0-9_]{1,63}$/;
const U32 = 0xffff_ffff;
const u32 = (n: number): number => Math.min(U32, Math.max(0, Math.floor(n)));

/** The `library.scan` payload of a finished scan (`kv` 1). */
export function libraryScanPayload(s: ScanSummary): LibraryScan {
  const files = s.counts === undefined ? 0 : live(s.counts.files);
  const dirs = s.counts === undefined ? 0 : live(s.counts.dirs);
  const placeholders = s.counts === undefined ? 0 : (s.counts.files.cloudOnly ?? 0) + (s.counts.dirs.cloudOnly ?? 0);
  return {
    storage: s.storage,
    full: s.mode === "full",
    trigger: s.trigger,
    outcome: s.outcome,
    ...(s.code !== undefined && CODE.test(s.code) ? { code: s.code } : {}),
    files: log2Bucket(files),
    dirs: log2Bucket(dirs),
    added: u32(s.applied.added),
    changed: u32(s.applied.changed),
    missing: u32(s.applied.missing),
    moved: u32(s.applied.renamed),
    placeholders: u32(placeholders),
    micros: Math.max(0, Math.floor(s.micros)),
  };
}

/** Records `library.scan` (source `finisherd`) when the journal is on; a refusal is logged. */
export function recordScan(journal: JournalLike | undefined, payload: LibraryScan, log: (message: string) => void): void {
  if (journal === undefined || !journal.enabled) return;
  try {
    journal.record("library.scan", 1, null, JSON.stringify(payload), "finisherd");
  } catch (e) {
    log(`library service: the journal refused a library.scan: ${(e as Error).message}`);
  }
}

/** The file events of one landed scan batch file (Task 11): one per changed row. */
export function batchEvents(lib: string, files: FileChanges | undefined): FileEvent[] {
  if (files === undefined) return [];
  const out: FileEvent[] = [];
  for (const type of ["added", "changed", "missing", "renamed"] as const) for (const fileId of files[type]) out.push({ type, library: lib, fileId });
  return out;
}

/** The file events of landed probe outcomes: `moved` for a row merged into its missing twin (by the id it kept), else `probed`. */
export function probedEvents(lib: string, applied: ProbedApplied): FileEvent[] {
  const moved = new Set(applied.moved.map((m) => m.to));
  return (applied.probed ?? []).map((p) => ({ type: moved.has(p.fileId) ? "moved" : "probed", library: lib, fileId: p.fileId, ...(p.fp === null ? {} : { fp: p.fp }) }));
}

/** `Libraries.onFile` listeners: each is called in turn; one that throws is logged, never fatal. */
export class FileEvents {
  readonly #listeners = new Set<(e: FileEvent) => void>();
  readonly #log: (message: string) => void;

  constructor(log: (message: string) => void) {
    this.#log = log;
  }

  on(listener: (e: FileEvent) => void): () => void {
    const own = (e: FileEvent) => listener(e);
    this.#listeners.add(own);
    return () => void this.#listeners.delete(own);
  }

  emit(e: FileEvent): void {
    for (const l of [...this.#listeners]) {
      try {
        l(e);
      } catch (err) {
        this.#log(`library service: a file event listener threw: ${(err as Error).message}`);
      }
    }
  }
}
