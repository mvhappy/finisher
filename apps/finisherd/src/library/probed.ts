// What a `library.probed` call's outcomes become in the catalog (plan 5D Decisions 34, 37;
// Task 5's states). The outcomes come from a worker and are untrusted: each is checked field
// for field before anything is applied.
//
// - `present`, `probeFailed`, `skipped`: stored (`applyProbed`), with the media columns read
//   from the probe;
// - `unreadable` (the open or a read failed on a root still online): stored `probeFailed`
//   with its code, so it is not retried until its size or mtime changes (as `POISON_ITEM`);
// - `changed`, `missing`, `cloudOnly`: never stored; the file's directory is rescanned (the
//   listing records what is there now);
// - `cancelled`: never sent (the worker drops it); refused like any unknown state.
import type { ProbeOutcome, RawRel } from "@finisher/core-types";
import { FinError } from "@finisher/contracts";
import type { ProbedRow } from "./catalog/client.ts";

/** What the catalog expects of a file a job read: from the attempt's `in/files.ndjson`. */
export interface ExpectedFile {
  readonly size: number;
  readonly mtimeNs: string;
  readonly rel: RawRel;
}

const STORED = new Set(["present", "probeFailed", "skipped"]);
const RESCAN = new Set(["changed", "missing", "cloudOnly"]);
const CODE = /^[A-Z][A-Z0-9_]{0,63}$/;
const FP = /^[A-Za-z0-9:+/=_-]{1,256}$/;
const RATIONAL = /^(-?\d{1,18})(?:\/(\d{1,18}))?$/;

const invalid = (why: string, at: string): FinError =>
  new FinError({ code: "INVALID_REQUEST", message: `library.probed: ${why}`, details: { method: "library.probed", at } });

const isObj = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
const isInt = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v);

/** Milliseconds of a rational number of seconds (`"n"` or `"n/d"`), rounded; `null` if it is not one. */
export function rationalMs(v: unknown): number | null {
  if (typeof v !== "string") return null;
  const m = RATIONAL.exec(v);
  if (m === null) return null;
  const n = Number(m[1]);
  const d = m[2] === undefined ? 1 : Number(m[2]);
  if (d === 0) return null;
  const ms = Math.round((n * 1000) / d);
  return Number.isSafeInteger(ms) && ms >= 0 ? ms : null;
}

const dim = (v: unknown): number | null => (isInt(v) && v > 0 && v <= 1 << 20 ? v : null);

/** The catalog's media columns of a probe (`kind`, `durationMs`, `width`, `height`). */
export function mediaColumns(probe: unknown): Pick<ProbedRow, "kind" | "durationMs" | "width" | "height"> {
  if (!isObj(probe)) return { kind: null, durationMs: null, width: null, height: null };
  const sub = (k: string): Record<string, unknown> | undefined => (isObj(probe[k]) ? (probe[k] as Record<string, unknown>) : undefined);
  const media = sub("media");
  const box = sub("video") ?? sub("image") ?? sub("lottie") ?? (isObj(media?.video) ? (media.video as Record<string, unknown>) : undefined);
  const size = isObj(box?.display) ? (box.display as Record<string, unknown>) : box;
  return {
    kind: typeof probe.kind === "string" && /^[a-zA-Z]{1,32}$/.test(probe.kind) ? probe.kind : null,
    durationMs: rationalMs(probe.duration ?? media?.duration ?? sub("lottie")?.duration),
    width: dim(size?.width),
    height: dim(size?.height),
  };
}

/**
 * The checked outcomes of one `library.probed {results}` call (≤ 32). `probeVersion`: this
 * server's (`libraryProbeVersion()`); a result claiming any other is refused, so a worker can
 * never mark a file probed at a version that keeps it from being probed again.
 */
export function decodeProbed(params: unknown, max: number, probeVersion: number): ProbeOutcome[] {
  if (!isObj(params) || !Array.isArray(params.results)) throw invalid("results must be a list", "results");
  if (params.results.length > max) throw invalid(`at most ${max} results per call`, "results");
  return params.results.map((r, i): ProbeOutcome => {
    const at = `results[${i}]`;
    if (!isObj(r) || !isInt(r.file) || !isInt(r.probeVersion) || r.probeVersion < 0 || typeof r.state !== "string") throw invalid("a malformed result", at);
    if (r.probeVersion !== probeVersion) throw invalid(`probeVersion must be ${probeVersion}`, at);
    if (r.code != null && (typeof r.code !== "string" || !CODE.test(r.code))) throw invalid("a malformed code", at);
    if (r.fingerprint != null && (typeof r.fingerprint !== "string" || !FP.test(r.fingerprint))) throw invalid("a malformed fingerprint", at);
    if (r.probe != null && !isObj(r.probe)) throw invalid("a malformed probe", at);
    return r as unknown as ProbeOutcome;
  });
}

export interface ProbedPlan {
  /** Rows to store. */
  readonly rows: ProbedRow[];
  /** Directories to rescan (raw chains; duplicates removed). */
  readonly rescan: RawRel[];
}

const dirOf = (rel: RawRel): RawRel => ({ components: rel.components.slice(0, -1) });

/**
 * What to store and rescan for `outcomes`. Every outcome must name a file of this attempt
 * (`expected`): its size and mtime are what the worker opened against, so a row that changed
 * since is left alone by `applyProbed`.
 */
export function planProbed(outcomes: ReadonlyArray<ProbeOutcome>, expected: ReadonlyMap<number, ExpectedFile>): ProbedPlan {
  const rows: ProbedRow[] = [];
  const dirs = new Map<string, RawRel>();
  outcomes.forEach((o, i) => {
    const e = expected.get(o.file);
    if (e === undefined) throw invalid("a result for a file this job does not read", `results[${i}]`);
    if (RESCAN.has(o.state)) {
      const d = dirOf(e.rel);
      dirs.set(d.components.join("/"), d);
      return;
    }
    const state = o.state === "unreadable" ? "probeFailed" : o.state;
    if (!STORED.has(state)) throw invalid(`an unknown state ${JSON.stringify(o.state)}`, `results[${i}]`);
    const probe = state === "present" ? (o.probe ?? null) : null;
    rows.push({
      fileId: o.file,
      size: e.size,
      mtimeNs: e.mtimeNs,
      fp: o.fingerprint ?? null,
      probe,
      probeVersion: o.probeVersion,
      state: state as ProbedRow["state"],
      code: state === "present" ? null : (o.code ?? (o.state === "unreadable" ? "ASSET_UNREADABLE" : null)),
      ...mediaColumns(probe),
    });
  });
  return { rows, rescan: [...dirs.values()] };
}
