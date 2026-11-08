// How `files.probe` is stored (plan 5D Task 7a): Task 7a's 100k-file measurement put a typical
// MP4 probe at ~540 bytes of JSON, which alone took the catalog past the 1 KiB-per-file budget
// (the FTS `dir` column costs ~50). So the column holds a BLOB: a format byte, then the
// deflate-raw of the probe's JSON with the fields the row already holds (`fingerprint` = `fp`,
// `size`, `kind`) left out when they are equal to it. Both threads run this, never the main one.
// Format 1 is frozen with the schema; a better encoding gets the next format byte.
import { deflateRawSync, inflateRawSync } from "node:zlib";

const FORMAT_DEFLATE_JSON = 1;

export interface ProbeRowFields {
  readonly fp: string | null;
  readonly size: number;
  readonly kind: string | null;
}

/** Encodes a probe for its row; `null` stays `null`. */
export function encodeProbe(probe: unknown, row: ProbeRowFields): Uint8Array | null {
  if (probe === undefined || probe === null) return null;
  let value = probe;
  if (typeof probe === "object" && !Array.isArray(probe)) {
    const p = { ...(probe as Record<string, unknown>) };
    if (p.fingerprint === row.fp && row.fp !== null) delete p.fingerprint;
    if (p.size === row.size) delete p.size;
    if (p.kind === row.kind && row.kind !== null) delete p.kind;
    value = p;
  }
  const body = deflateRawSync(Buffer.from(JSON.stringify(value), "utf8"));
  const out = new Uint8Array(body.length + 1);
  out[0] = FORMAT_DEFLATE_JSON;
  out.set(body, 1);
  return out;
}

/** Decodes a stored probe, restoring the fields its row holds. */
export function decodeProbe(stored: Uint8Array | null, row: ProbeRowFields): unknown {
  if (stored === null) return null;
  if (stored[0] !== FORMAT_DEFLATE_JSON) throw new Error(`unknown probe format ${stored[0]}`);
  const value = JSON.parse(inflateRawSync(stored.subarray(1)).toString("utf8")) as unknown;
  if (value === null || typeof value !== "object" || Array.isArray(value)) return value;
  const p = value as Record<string, unknown>;
  return {
    ...(row.kind !== null && !("kind" in p) ? { kind: row.kind } : {}),
    ...(row.fp !== null && !("fingerprint" in p) ? { fingerprint: row.fp } : {}),
    ...(!("size" in p) ? { size: row.size } : {}),
    ...p,
  };
}
