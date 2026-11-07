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
 * (`<name>.actual`, git-ignored by its suffix) for a diff; with `plans` (the full plans the
 * golden summarises) it also writes them to `<name without .json>.plans.actual.json`, so a
 * difference between hosts can be bisected down to the plan field.
 */
export function checkGolden(name, actual, { dir = GOLDEN_DIR, bless = blessing(), plans } = {}) {
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
  let where = `${name}.actual`;
  if (plans !== undefined) {
    const plansName = `${name.replace(/\.json$/, "")}.plans.actual.json`;
    writeFileSync(join(dir, plansName), `${JSON.stringify(plans, null, 2)}\n`);
    where += ` and ${plansName}`;
  }
  throw new Error(`${name} differs from its golden: ${firstDifference(actual, expected)} (actual in ${where})`);
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

/** The text engine `fin-text` names in a plan that measured captions: its crate and revision, then the versions. */
export const TEXT_ENGINE = /^fin-text\/\S+ rev\/\d+ /;

/** A frame plan without its `textEngine` (crate versions differ between builds), which is checked apart. */
export const withoutEngine = ({ textEngine, ...rest }) => rest;

/**
 * One frame's golden entry: `{t, sha256, captions, layers}`. `sha256` digests the plan without
 * its `textEngine`; `captions` is the measured shape of each caption layer, so a change of
 * layout shows without hashing.
 */
export function frameGolden(t, plan) {
  return { t, sha256: digest(withoutEngine(plan)), captions: captionSummary(plan), layers: layerSummary(plan) };
}

/** The measured shape of each caption layer of a frame plan (`layout` is null when unmeasured). */
export function captionSummary(plan) {
  return plan.layers
    .filter((l) => l.caption)
    .map((l) => {
      const lay = l.caption.layout;
      if (!lay) return { item: l.item, unmeasured: l.caption.unmeasured?.reason ?? null };
      return { item: l.item, lines: lay.lines.length, faces: lay.faces, width: lay.width, height: lay.height, words: lay.words.length, missing: lay.missing };
    });
}

/**
 * Whether `plan` names its text engine as it should: present (and shaped like `TEXT_ENGINE`)
 * exactly when it measured a caption, else absent.
 */
export function engineProblem(plan, label) {
  const measured = plan.layers.some((l) => l.caption?.layout);
  if (!measured) return plan.textEngine === undefined ? undefined : `${label}: textEngine ${plan.textEngine} on a plan that measured nothing`;
  if (typeof plan.textEngine !== "string" || !TEXT_ENGINE.test(plan.textEngine)) return `${label}: textEngine ${JSON.stringify(plan.textEngine)} is not "fin-text/<v> rev/<n> ..."`;
  return undefined;
}

/**
 * `y` of `cubic-bezier(x1, y1, x2, y2)` at `x`, found by bisection on the curve's own
 * parameter (independent of the engine's solver): the reference for eased keyframe values.
 */
export function cubicBezier(x1, y1, x2, y2, x) {
  const at = (a, b, s) => 3 * (1 - s) * (1 - s) * s * a + 3 * (1 - s) * s * s * b + s * s * s;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 60; i++) {
    const mid = (lo + hi) / 2;
    if (at(x1, x2, mid) < x) lo = mid;
    else hi = mid;
  }
  return at(y1, y2, (lo + hi) / 2);
}

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
