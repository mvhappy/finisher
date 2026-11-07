// The "reel" scenario's real-font pass (plan 4C Task 27 step 9): a second project on the
// built-in catalog, offline, its font cache seeded from the warmed `FIN_FONT_CACHE`, with
// captions in Latin, Arabic, CJK and emoji; the frame plans must equal `frames-real.json`.
// Without `FIN_FONT_CACHE` the pass is skipped with a notice, unless `FIN_REQUIRE_FONTS=1`
// (CI), which makes it fail instead.
import { copyFileSync, existsSync, linkSync, mkdirSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { isDeepStrictEqual } from "node:util";
import { assertEqual, fin, log, startServer, stopServer, waitFor } from "./lib.mjs";
import { checkGolden, digest, layerSummary } from "./reel-lib.mjs";

/** One caption block per script: Latin (the face), then Arabic, CJK and emoji (fallbacks). */
export const REAL_WORDS = [
  { text: "Hello", start: "0", end: "0.5" },
  { text: "world", start: "0.5", end: "1" },
  { text: "مرحبا", start: "1.5", end: "2" },
  { text: "بالعالم", start: "2", end: "2.5" },
  { text: "你好", start: "3", end: "3.5" },
  { text: "世界", start: "3.5", end: "4" },
  { text: "🎉", start: "4.5", end: "5" },
  { text: "✨", start: "5", end: "5.5" },
];

export const REAL_FRAME_TIMES = ["0.2s", "1.8s", "3.2s", "4.8s"];

/** The built-in catalog's pins the import plans: the face, then the fallbacks for its scripts. */
const REAL_PINS = [
  ["Inter|700|normal", "inter"],
  ["Noto Sans Arabic|400|normal", "noto-sans-arabic"],
  ["Noto Sans CJK|400|normal", "noto-sans-cjk"],
  ["Noto Color Emoji|400|normal", "noto-color-emoji"],
];

/** Whether the pass can run: the warmed cache, else a skip (or a failure under `FIN_REQUIRE_FONTS=1`). */
export function realCache(env = process.env) {
  const cache = env.FIN_FONT_CACHE;
  if (cache && existsSync(cache)) return cache;
  const why = cache ? `FIN_FONT_CACHE ${cache} does not exist` : "FIN_FONT_CACHE is not set";
  if (env.FIN_REQUIRE_FONTS === "1") throw new Error(`real fonts are required (FIN_REQUIRE_FONTS=1) but ${why}`);
  log(`real fonts: skipped (${why}; warm one with cargo run -p fin-fonts --example warm_cache -- <dir>)`);
  return undefined;
}

/** Links (else copies) every cache file `<sha256>` of `from` into `to`. */
function seed(from, to) {
  mkdirSync(to, { recursive: true });
  const files = readdirSync(from).filter((n) => /^[0-9a-f]{64}$/.test(n));
  for (const name of files) {
    try {
      linkSync(join(from, name), join(to, name));
    } catch {
      copyFileSync(join(from, name), join(to, name));
    }
  }
  return files.length;
}

/** Runs the pass under `tmp`; returns whether it ran. */
export async function realFonts(tmp) {
  const cache = realCache();
  if (!cache) return false;
  const sd = join(tmp, "server-real");
  const project = join(tmp, "reel-real");
  log(`real fonts: seeded ${seed(cache, join(sd, "fonts"))} cache files from ${cache}`);
  // The built-in catalog (no FIN_FONT_CATALOG), and nothing fetched.
  const server = await startServer(sd, { FIN_FONTS_OFFLINE: "1" });
  try {
    const f = (args) => fin(sd, project, args);
    fin(sd, tmp, ["project", "create", project, "--name", "Reel real fonts"]);
    f(["track", "add", "--kind", "caption"]);
    writeFileSync(join(project, "words.json"), JSON.stringify(REAL_WORDS));
    const imported = f(["caption", "import", "--track", "t3", "--words", "words.json", "--style", "preset:bold"]);
    assertEqual(imported.resolved.fonts.planned.map((p) => [p.key, p.source.entry]), REAL_PINS, "real pins");
    // Every catalog file is listed; a pinned one names its keys on the row of its style (Inter's
    // variable file serves 400 and 700 from one file).
    const pinnedRows = (st) => st.files.filter((x) => x.keys.length > 0).map((x) => [x.keys.join(), x.entry, x.style, x.state.state]);
    const want = REAL_PINS.map(([key, entry]) => [key, entry, entry === "inter" ? "bold" : "regular", "cached"]);
    await waitFor(
      "every pinned real font cached",
      () => isDeepStrictEqual(pinnedRows(f(["fonts", "status"])), want) || undefined,
      15_000,
      () => `: ${JSON.stringify(pinnedRows(f(["fonts", "status"])))}`,
      200,
    );
    const plans = REAL_FRAME_TIMES.map((t) => ({ t, plan: f(["eval", "frame", "--at", t]).plan }));
    for (const { t, plan } of plans) {
      const caps = plan.layers.filter((l) => l.caption);
      if (caps.length !== 1 || !caps[0].caption.layout || caps[0].caption.layout.missing.length > 0) {
        throw new Error(`real fonts: the caption at ${t} should be laid out with nothing missing: ${JSON.stringify(caps)}`);
      }
    }
    checkGolden("frames-real.json", plans.map(({ t, plan }) => ({ t, sha256: digest(plan), layers: layerSummary(plan) })));
    return true;
  } finally {
    await stopServer(server);
  }
}
