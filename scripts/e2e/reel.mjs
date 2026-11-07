// The "reel" scenario (plan 4C Task 27, spec §5.9): a 30 s vertical reel built by `fin` only.
// finisherd runs on a test font catalog served from a loopback server (test-hooks build) and
// starts with background fetching off; captions are unmeasured until it is switched on, then
// the pinned fonts arrive by themselves and eval lays the captions out, while the project's
// records stay byte-identical. View, frame and audio plans are checked against goldens
// (`reel-golden/`, `E2E_REEL_BLESS=1` rewrites them). An optional pass measures real catalog
// fonts from a warmed cache (`FIN_FONT_CACHE`; `FIN_REQUIRE_FONTS=1` makes it required).
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertEqual, fin, finRaw, live, log, removeTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { testFont, writeFixture, writeFixtureOf } from "./fixture.mjs";
import { catalogJson, startFontServer } from "./fontserver.mjs";
import { failure } from "./history-lib.mjs";
import { audioSummary, checkGolden, digest, hashTree, layerSummary } from "./reel-lib.mjs";
import { realFonts } from "./reel-real.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/** Polls a `fin` read every 200 ms (each poll is a process) until `predicate` answers. */
const poll = (what, predicate, timeoutMs, onTimeout) => waitFor(what, predicate, timeoutMs, onTimeout, 200);

/** The frame times the goldens sample: the plan's six, and 9.8 s inside the dissolve and the graphic. */
export const FRAME_TIMES = ["0", "0.5s", "4.9s", "9.8s", "12s", "20s", "29.9s"];

/** The caption words: Latin in the style's face, Hebrew through the catalog's fallback. */
export const WORDS = [
  { text: "Morning", start: "0", end: "0.6" },
  { text: "light", start: "0.6", end: "1.2" },
  { text: "on", start: "1.2", end: "1.5" },
  { text: "the", start: "1.5", end: "1.8" },
  { text: "water", start: "1.8", end: "2.6" },
  { text: "שלום", start: "2.8", end: "3.6" },
  { text: "עולם", start: "3.6", end: "4.4" },
  { text: "see", start: "4.6", end: "5.0" },
  { text: "you", start: "5.0", end: "5.4" },
  { text: "soon", start: "5.4", end: "6.2" },
];

/** The test catalog: the bold preset's face (`Inter` 700) and a Hebrew fallback, from generated fonts. */
async function testCatalog(fonts) {
  const inter = await testFont("latinBold");
  const hebrew = await testFont("rtl");
  const files = [
    { id: "inter", family: "Inter", weight: 700, scripts: ["Latn"], bytes: inter, url: fonts.serve("inter-bold.ttf", inter) },
    { id: "hebrew", family: "Test Hebrew", weight: 400, scripts: ["Hebr"], bytes: hebrew, url: fonts.serve("hebrew.ttf", hebrew) },
  ];
  return catalogJson("2026.11.1", files, ["hebrew"]);
}

/** The frame plans at `FRAME_TIMES`: `[{t, sha256, layers}]`, plus the plans themselves. */
function frames(f) {
  const plans = FRAME_TIMES.map((t) => ({ t, plan: f(["eval", "frame", "--at", t]).plan }));
  return { plans, golden: plans.map(({ t, plan }) => ({ t, sha256: digest(plan), layers: layerSummary(plan) })) };
}

const captionsOf = (plans) => plans.flatMap(({ plan }) => plan.layers.filter((l) => l.caption).map((l) => l.caption));

/** The project's records as exported to its folder: `{segment path: sha256}`. */
const records = (project) => hashTree(project, (rel) => rel.startsWith(".finisher/segments/"));

/** Polls `fin project status` until every record is exported to the folder; returns the head. */
async function exported(f) {
  const st = await poll(
    "the export to catch up",
    () => {
      const main = f(["project", "status"]).status.branches.find((b) => b.branch === "main");
      return main.exportedThrough === main.head ? main : undefined;
    },
    10_000,
  );
  return st.head;
}

export async function run() {
  const started = Date.now();
  const tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisher-e2e-reel-")));
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "reel");
  let server;
  const fonts = await startFontServer();
  try {
    const catalog = join(tmp, "catalog.json");
    writeFileSync(catalog, await testCatalog(fonts));
    // Background fetching off through the settings file (finisherd re-reads it while running),
    // so the same server goes online in step 5.
    mkdirSync(sd, { recursive: true });
    const settings = join(sd, "settings.json");
    writeFileSync(settings, JSON.stringify({ fonts: { autoFetch: false } }));
    server = await startServer(sd, { FIN_FONT_CATALOG: catalog, FIN_FONTS_TEST_BACKOFF_MS: "0", FIN_FONTS_TEST_NO_PROXY: "1" });
    const f = (args) => fin(sd, project, args);
    const raw = (args) => finRaw(sd, project, args);

    step(1, "project; tracks A2 (music), C1 (captions), V2 (overlays)");
    fin(sd, tmp, ["project", "create", project, "--name", "Reel"]);
    assertEqual(
      ["audio", "caption", "video"].map((kind) => f(["track", "add", "--kind", kind]).bindings.$track),
      ["t3", "t4", "t5"],
      "track ids",
    );

    step(2, "assets: an A/V MP4, a PNG, a dotLottie with a text slot, a WAV");
    await writeFixture(join(project, "shot.mp4"), 15);
    await writeFixtureOf("png", join(project, "logo.png"));
    await writeFixtureOf("dotlottie", join(project, "title.lottie"));
    await writeFixtureOf("wav:30", join(project, "music.wav"));
    assertEqual(
      ["shot.mp4", "logo.png", "title.lottie", "music.wav"].map((p) => f(["asset", "add", p]).resolved.kind),
      ["media", "image", "lottieTemplate", "media"],
      "asset kinds",
    );

    step(3, "V1: three clips, linked audio on A1, one at 3/2; V2: image, graphic; A2: music");
    const inserts = [
      ["--at", "0", "--dur", "10s"],
      ["--at", "end", "--in", "2s", "--dur", "6s"],
      ["--at", "end", "--in", "1s", "--dur", "14s"],
    ].map((a) => f(["clip", "insert", "--track", "t1", "--asset", "a1", ...a]));
    assertEqual(
      inserts.map((o) => [o.bindings.$clip, o.bindings.$audio, o.resolved.audio.track]),
      [["c1", "c2", "t2"], ["c3", "c4", "t2"], ["c5", "c6", "t2"]],
      "linked audio on A1 by default",
    );
    // c3 and its audio c4 at 3/2: source 2 s..11 s over 6 s.
    f(["param", "set", "c3", "speed", "3/2"]);
    f(["param", "set", "c4", "speed", "3/2"]);
    assertEqual(f(["image", "insert", "--track", "t5", "--at", "1s", "--asset", "a2", "--dur", "5s"]).bindings, { $image: "img1" }, "image");
    assertEqual(f(["graphic", "insert", "--track", "t5", "--at", "8s", "--template", "a3", "--slot", "title=Reel"]).bindings, { $graphic: "g1" }, "graphic");
    assertEqual(f(["clip", "insert", "--track", "t3", "--at", "0", "--asset", "a4", "--dur", "30s"]).bindings, { $clip: "c7" }, "music");
    f(["param", "set", "c7", "audio.gain", "-6dB"]);
    f(["param", "set", "c7", "audio.fadeIn", "2s"]);

    step(4, "captions offline: the face and a Hebrew fallback pinned, both missing, unmeasured");
    writeFileSync(join(project, "words.json"), JSON.stringify(WORDS));
    const imported = f(["caption", "import", "--track", "t4", "--words", "words.json", "--style", "preset:bold"]);
    assertEqual(imported.resolved.fonts.planned.map((p) => [p.key, p.source.entry]), PINNED, "planned pins");
    assertEqual(imported.resolved.items, ["$cap1", "$cap2", "$cap3"], "caption blocks");
    // The font summary is published after the commit.
    const missing = await poll("FONT_MISSING in fin status", () => {
      const st = f(["status"]);
      const codes = st.warnings.filter((w) => w.code === "FONT_MISSING").map((w) => w.details.key);
      return codes.length === 2 ? st : undefined;
    }, 10_000);
    assertEqual(missing.fonts.missing.map((m) => [m.key, m.reason]), PINNED.map(([key]) => [key, "offline"]), "missing fonts");
    assertEqual(missing.fonts.unmeasured, ["cap1", "cap2", "cap3"].map((item) => ({ item, reason: "font missing" })), "unmeasured captions");
    const captionLines = (view) => view.lines.filter((l) => / C1 /.test(l));
    for (const line of captionLines(f(["view"]))) {
      if (!line.endsWith("(unmeasured: font missing)")) throw new Error(`caption line not marked unmeasured: ${line}`);
    }
    const before = frames(f);
    const offlineCaptions = captionsOf(before.plans);
    if (offlineCaptions.length === 0 || offlineCaptions.some((c) => c.layout !== null || c.unmeasured?.reason !== "fontMissing")) {
      throw new Error(`offline captions should be unmeasured (fontMissing): ${JSON.stringify(offlineCaptions)}`);
    }
    const head = await exported(f);
    const recordsBefore = records(project);
    if (Object.keys(recordsBefore).length === 0) throw new Error("no exported records in the project folder");

    step(5, "online: the pinned fonts fetch by themselves, eval lays captions out, records unchanged");
    writeFileSync(settings, JSON.stringify({ fonts: { autoFetch: true } }));
    const cached = await poll("both pinned fonts cached", () => {
      const st = f(["fonts", "status"]);
      return st.autoFetch && st.files.every((file) => file.state.state === "cached") ? st : undefined;
    }, 15_000);
    assertEqual(cached.files.map((file) => [file.entry, file.state.state]), [["inter", "cached"], ["hebrew", "cached"]], "fonts status");
    assertEqual(["inter-bold.ttf", "hebrew.ttf"].map((n) => fonts.hitsOf(n)), [1, 1], "one download per font");
    await poll("no font warnings", () => f(["status"]).warnings.length === 0 || undefined, 10_000);
    const online = frames(f);
    const onlineCaptions = captionsOf(online.plans);
    if (onlineCaptions.some((c) => !c.layout || c.unmeasured)) throw new Error(`online captions should be laid out: ${JSON.stringify(onlineCaptions)}`);
    // Only the caption layers changed (and the plan names the text engine that measured them).
    assertEqual(online.plans.map(({ plan }) => digest(withoutCaptions(plan))), before.plans.map(({ plan }) => digest(withoutCaptions(plan))), "non-caption layers");
    if (captionLines(f(["view"])).some((l) => l.includes("unmeasured"))) throw new Error("view still marks captions unmeasured");
    assertEqual(await exported(f), head, "no record written by fetching");
    assertEqual(records(project), recordsBefore, "records byte-identical before and after the fetch");

    step(6, "keyframed overlay opacity (easeOut); a dissolve between c1 and c3, crossfade on the linked audio");
    f(["keyframe", "set", "img1", "opacity", "--at", "0", "0"]);
    f(["keyframe", "set", "img1", "opacity", "--at", "4s", "1", "--interp", "easeOut"]);
    const dissolve = f(["transition", "set", "--between", "c1", "c3", "--kind", "dissolve", "--dur", "1s"]);
    assertEqual(
      dissolve.resolved.transitions.map((t) => [t.between, t.track]),
      [[["c1", "c3"], "t1"], [["c2", "c4"], "t2"]],
      "the dissolve and its linked audio crossfade",
    );
    assertEqual(dissolve.bindings, { $transition: "tr1", $transition2: "tr2" }, "transition ids");

    step(7, "30 s; view, frame and audio plans equal the goldens");
    const status = f(["status"]);
    assertEqual(status.timelines.map((tl) => tl.end), ["30"], "timeline length");
    const view = f(["view"]);
    checkGolden("view.json", view);
    const final = frames(f);
    checkGolden("frames.json", final.golden);
    const audio = f(["eval", "audio", "--range", "0..30s"]).plan;
    checkGolden("audio.json", { range: "0..30s", sha256: digest(audio), items: audioSummary(audio) });
    if (!audio.items.some((i) => i.item === "c7" && i.gain[0].db[0] === -6 && i.gain[0].shapes?.[0]?.kind === "fadeIn")) {
      throw new Error("the music should fade in at -6 dB");
    }

    step(8, "an overflowing caption is refused; deleting c3 prunes both transitions, undo restores them");
    const long = Array.from({ length: 40 }, (_, i) => `overflowing${i}`).join(" ");
    failure(raw(["caption", "set-text", "cap1", long]), "CAPTION_OVERFLOW", 3, "caption set-text");
    const deleted = f(["clip", "delete", "c3"]);
    const removed = (diff) => diff.changes.filter((c) => c.kind === "transition" && c.change === "removed").map((c) => c.id);
    assertEqual(removed(deleted.diff), ["tr1", "tr2"], "transitions pruned by deleting c3");
    if (f(["view"]).lines.some((l) => / tr\d /.test(l))) throw new Error("a transition is left after deleting c3");
    f(["undo"]);
    const { rev, ...restored } = f(["view"]);
    assertEqual(restored, withoutRev(view), "view after undo");
    assertEqual(frames(f).golden, final.golden, "frames after undo");

    if (await realFonts(tmp)) log("real fonts: frames equal frames-real.json");
    log(`PASS reel in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    if (server) await stopServer(server);
    await fonts.close();
    removeTmp(tmp);
  }
}

/** The pins `caption import` plans: the bold preset's face, then the Hebrew fallback. */
const PINNED = [
  ["Inter|700|normal", "inter"],
  ["Test Hebrew|400|normal", "hebrew"],
];

const withoutRev = ({ rev, ...rest }) => rest;

/** A frame plan without its caption layers and text engine. */
function withoutCaptions({ textEngine, ...plan }) {
  return { ...plan, layers: plan.layers.filter((l) => !l.caption) };
}
