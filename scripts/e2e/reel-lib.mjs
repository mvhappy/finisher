// Helpers of the "reel" e2e scenario: goldens (compare or bless), plan digests, folder hashes.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { log } from "./lib.mjs";

export const GOLDEN_DIR = fileURLToPath(new URL("./reel-golden", import.meta.url));

/**
 * Whether goldens are rewritten instead of compared: `E2E_REEL_BLESS=1`, refused under `CI`
 * (a blessed golden is reviewed in a commit, never made on the build machine).
 */
export function blessing(env = process.env) {
  if (env.E2E_REEL_BLESS !== "1") return false;
  if (env.CI) throw new Error("E2E_REEL_BLESS is refused under CI: bless locally and commit the goldens");
  return true;
}

/**
 * Compares `actual` with the golden `name` (`reel-golden/<name>`), or writes it when blessing.
 * A mismatch names the first differing entry and leaves the actual value beside the golden
 * (`<name>.actual`, git-ignored by its suffix) for a diff.
 */
export function checkGolden(name, actual, { dir = GOLDEN_DIR, bless = blessing() } = {}) {
  const path = join(dir, name);
  const text = `${JSON.stringify(actual, null, 2)}\n`;
  if (bless) {
    mkdirSync(dir, { recursive: true });
    writeFileSync(path, text);
    log(`blessed ${name}`);
    return;
  }
  if (!existsSync(path)) throw new Error(`no golden ${name} (run with E2E_REEL_BLESS=1 to write it)`);
  const expected = JSON.parse(readFileSync(path, "utf8"));
  if (isDeepStrictEqual(actual, expected)) return;
  writeFileSync(`${path}.actual`, text);
  throw new Error(`${name} differs from its golden: ${firstDifference(actual, expected)} (actual in ${name}.actual)`);
}

/** The first path where `a` and `b` differ, with both values. */
export function firstDifference(a, b, path = "$") {
  if (isDeepStrictEqual(a, b)) return undefined;
  if (a && b && typeof a === "object" && typeof b === "object" && Array.isArray(a) === Array.isArray(b)) {
    const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])];
    for (const k of keys) {
      const d = firstDifference(a[k], b[k], Array.isArray(a) ? `${path}[${k}]` : `${path}.${k}`);
      if (d) return d;
    }
  }
  return `${path}: got ${JSON.stringify(a)}, expected ${JSON.stringify(b)}`;
}

/** SHA-256 of the compact JSON of `value` (a frame or audio plan). */
export const digest = (value) => createHash("sha256").update(JSON.stringify(value)).digest("hex");

/** A readable line per layer of a frame plan, beside its digest in the goldens. */
export function layerSummary(plan) {
  return plan.layers.map((l) => {
    const parts = [l.item, l.kind, l.track];
    if (l.source) parts.push(`src ${l.source.time}`);
    if (l.opacity !== 1) parts.push(`opacity ${l.opacity}`);
    if (l.transition) parts.push(`${l.transition.id} ${l.transition.role} ${l.transition.progress}`);
    const cap = l.caption;
    if (cap?.layout) parts.push(`${cap.layout.lines.length} lines, ${cap.layout.faces.length} faces, ${cap.layout.width}x${cap.layout.height}/64 px`);
    if (cap?.unmeasured) parts.push(`unmeasured ${cap.unmeasured.reason}`);
    return parts.join(" ");
  });
}

/** A readable line per item of an audio plan: extent, speed, and the gain shapes in order. */
export function audioSummary(plan) {
  return plan.items.map((i) => {
    const shapes = i.gain.flatMap((g) => (g.shapes ?? []).map((s) => `${s.kind}@${g.from}`));
    const db = [...new Set(i.gain.flatMap((g) => g.db))];
    return `${i.item} ${i.track} ${i.from}..${i.to} x${i.speed} ${db.join("/")}dB ${shapes.join(" ")}`.trim();
  });
}

/** `{relative path: sha256}` of every file under `dir` whose relative path `keep` accepts. */
export function hashTree(dir, keep = () => true) {
  const out = {};
  const walk = (d) => {
    for (const name of readdirSync(d).sort()) {
      const p = join(d, name);
      if (statSync(p).isDirectory()) walk(p);
      else {
        const rel = relative(dir, p).split("\\").join("/");
        if (keep(rel)) out[rel] = createHash("sha256").update(readFileSync(p)).digest("hex");
      }
    }
  };
  walk(dir);
  return out;
}
