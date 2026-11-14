// Synthetic media cache entries for the tests of what serves them (6B-1 Task 17): `meta.json`
// and payload files as a worker would leave them in a job's `out/<key>/`, committed through
// `MediaCache.commit` (no proxy entries exist before Task 14, and no media file is needed).
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { blake3Hex } from "@finisher/fin-node";

/** Canonical JSON (fin-media's `canonical_json`): keys sorted at every level, no whitespace. */
export function canonical(v: unknown): string {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v !== null && typeof v === "object") {
    const o = v as Record<string, unknown>;
    return `{${Object.keys(o)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}

/**
 * Writes `<jobOut>/<key>/` holding `files` and a `meta.json` for a `proxy-1080` of `fingerprint`
 * and answers the key (the key input's `renderVersion` is whatever `meta.json` says).
 */
export function writeEntry(jobOut: string, fingerprint: string, files: Record<string, Buffer | string>): string {
  const input = { fingerprint, recipe: "proxy-1080", recipeVersion: 1, params: { tier: "t1080" }, renderVersion: 1, decoderClass: "software" };
  const key = blake3Hex(Buffer.from(canonical(input)));
  const dir = join(jobOut, key);
  mkdirSync(dir, { recursive: true });
  const listed = Object.entries(files).map(([name, data]) => {
    const bytes = Buffer.from(data);
    writeFileSync(join(dir, name), bytes);
    return { name, bytes: bytes.length, blake3: blake3Hex(bytes) };
  });
  writeFileSync(join(dir, "meta.json"), JSON.stringify({ key, ...input, files: listed, createdAt: 1 }));
  return key;
}

/** `n` bytes whose value at `i` is `(i * 7 + 3) % 251`: any wrong offset or length shows. */
export function pattern(n: number): Buffer {
  const b = Buffer.alloc(n);
  for (let i = 0; i < n; i++) b[i] = (i * 7 + 3) % 251;
  return b;
}
