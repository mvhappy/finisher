// The "look" scenario (6A-2 Task 15, spec §3.3): `fin look` end to end. Two AV1 clips made by
// `fin-codecs`' `make_fixture` (a 540p30 CFR clip and a 540p VFR clip tagged for a 90° rotation)
// go on a timeline with a dissolve, a rotated multiply overlay with keyframed opacity and a
// caption; `fin look` renders it at seven times through finisherd's real `media` worker. Checked:
// the plan hashes against `look-golden/plans.json`; the frame id read back from each PNG where a
// clip lies unobstructed against the nearest-frame rule re-derived here (a midpoint of the VFR
// clip picks the earlier frame); every PNG against the CPU reference over a copy of the same job
// directory (tolerance T1; its hashes against `look-golden/t0.json` when this machine's fixtures
// have an entry); the same directory replayed by `fin-workers media --replay`; `skipped` naming
// the caption; and a look whose worker is killed mid-job, which the job engine retries.
// `E2E_LOOK_BLESS=1` rewrites the goldens (refused under CI). Needs a GPU adapter (`FIN_GPU`);
// without one the scenario is skipped, unless `FIN_REQUIRE_GPU=1`.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { ROOT, WIN, assertEqual, fin, live, log, removeTmp, scenarioTmp, startServer, stopServer, storedEntries, waitFor } from "./lib.mjs";
import { checkGolden } from "./reel-lib.mjs";
import { GOLDEN_DIR, blessing, copyJobAside, filesDigest, finAsync, machineClass, mediaWorkers, nearestFrame, placeFlat, placeRotated, pngsBelow, readFrameId, readPng, vfrPts } from "./look-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/**
 * The looked-at times: the plan's five, then two exact midpoints of the VFR clip. It starts at
 * 2 s on V1 from 1 s (30 ticks of 1/30 s) into the file, so timeline 13/6 s is source tick 35
 * (between the frames at ticks 34 and 36) and 73/30 s is tick 43 (between 42 and 44).
 */
export const AT = ["0", "0.5s", "1s", "1.5s", "2s", "13/6", "73/30"];
const VFR_MIDPOINTS = [35, 43];

const WORDS = [
  { text: "Morning", start: "0", end: "0.6" },
  { text: "light", start: "0.6", end: "1.2" },
  { text: "on", start: "1.2", end: "1.8" },
  { text: "the", start: "1.8", end: "2.4" },
  { text: "water", start: "2.4", end: "3.2" },
  { text: "again", start: "3.2", end: "3.9" },
];

/** Runs a `cargo` command in the repository; throws with its output on failure. */
function cargo(args, { quiet = true } = {}) {
  const r = spawnSync("cargo", args, { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.status !== 0) throw new Error(`cargo ${args.join(" ")} failed (exit ${r.status}): ${r.stderr}`);
  if (!quiet) log(r.stdout.trim());
  return r.stdout;
}

/** The fixture clips: `a.mp4` (540p30 CFR, 90 frames) and `b.mp4` (540p VFR, rotated 90°, 60 frames). */
function makeFixtures(dir) {
  const make = (name, ...opts) => cargo(["run", "-q", "--release", "-p", "fin-codecs", "--features", "testing", "--example", "make_fixture", "--", join(dir, name), "--size", "960x540", "--rate", "30", ...opts]);
  make("a.mp4", "--frames", "90", "--seed", "1");
  make("b.mp4", "--frames", "60", "--vfr", "--rotate", "90", "--seed", "2");
  return [join(dir, "a.mp4"), join(dir, "b.mp4")];
}

/** The CPU reference over a job directory, compared with `pngs` (in `at` order). */
function referenceRender(jobDir, pngs) {
  const out = cargo(["run", "-q", "--release", "-p", "fin-workers", "--example", "reference_render", "--", jobDir, ...pngs.flatMap((p) => ["--png", p])]);
  return JSON.parse(out.trim().split("\n").pop()).frames;
}

/** `fin-workers media --replay <dir> --originals-anywhere`: the answer's `result`. */
function replay(binary, dir) {
  const r = spawnSync(binary, ["media", "--replay", dir, "--originals-anywhere"], { encoding: "utf8", env: process.env, timeout: 120_000 });
  const answer = JSON.parse(r.stdout.trim().split("\n").pop() ?? "{}");
  if (r.status !== 0 || !answer.result) throw new Error(`fin-workers media --replay failed (exit ${r.status}): ${r.stdout} ${r.stderr}`);
  return answer.result;
}

const same = (a, b) => readFileSync(a).equals(readFileSync(b));

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-look-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const project = join(tmp, "look");
  const jobs = join(sd, "jobs");
  let server;
  try {
    step(1, "fixtures: a 540p30 CFR clip and a 540p VFR clip rotated 90°");
    const clips = makeFixtures(tmp);
    const fixtures = filesDigest(clips);
    log(`fixtures sha256 ${fixtures.slice(0, 16)}… (${machineClass()})`);

    mkdirSync(sd, { recursive: true });
    // No font fetching: the caption stays unmeasured (and its plan free of the text engine's version).
    writeFileSync(join(sd, "settings.json"), JSON.stringify({ fonts: { autoFetch: false } }));
    server = await startServer(sd);
    const f = (args) => fin(sd, project, args);
    const caps = fin(sd, tmp, ["media", "caps"]);
    if (!caps.gpu) {
      if (process.env.FIN_REQUIRE_GPU === "1") throw new Error("FIN_REQUIRE_GPU=1 but the media worker opened no GPU adapter");
      log("SKIP look: no GPU adapter (set FIN_GPU, or FIN_REQUIRE_GPU=1 to fail)");
      return;
    }
    log(`adapter ${caps.gpu.backend} ${caps.gpu.deviceType}${caps.gpu.software ? " (software)" : ""}`);

    step(2, "project; V1: the CFR clip then the VFR clip with a 0.2 s dissolve; V2: the VFR clip as an overlay; a caption");
    fin(sd, tmp, ["project", "create", project, "--name", "Look"]);
    assertEqual(f(["track", "add", "--kind", "video"]).bindings.$track, "t3", "V2");
    assertEqual(f(["track", "add", "--kind", "caption"]).bindings.$track, "t4", "C1");
    assertEqual(clips.map((c) => f(["asset", "add", c]).resolved.kind), ["media", "media"], "asset kinds");
    const [a, b] = [f(["clip", "insert", "--track", "t1", "--asset", "a1", "--at", "0", "--dur", "2s"]), f(["clip", "insert", "--track", "t1", "--asset", "a2", "--at", "end", "--in", "1s", "--dur", "2s"])];
    assertEqual([a.bindings.$clip, b.bindings.$clip], ["c1", "c2"], "V1 clips");
    const dissolve = f(["transition", "set", "--between", "c1", "c2", "--kind", "dissolve", "--dur", "0.2s"]);
    assertEqual(dissolve.bindings.$transition, "tr1", "the dissolve");
    const over = f(["clip", "insert", "--track", "t3", "--asset", "a2", "--at", "0.25s", "--in", "0", "--dur", "1.5s"]).bindings.$clip;
    // Item-relative keyframes: opacity 0 → 1 over the overlay, a 0° → 30° turn, a multiply blend.
    f(["keyframe", "set", over, "opacity", "--at", "0", "0"]);
    f(["keyframe", "set", over, "opacity", "--at", "1.5s", "1"]);
    f(["keyframe", "set", over, "transform.rotation", "--at", "0", "0"]);
    f(["keyframe", "set", over, "transform.rotation", "--at", "1.5s", "30"]);
    f(["param", "set", over, "blend", "multiply"]);
    // Small enough to stay off the frame-id corners of the clips under it.
    f(["param", "set", over, "transform.scale", "0.4,0.4"]);
    writeFileSync(join(project, "words.json"), JSON.stringify(WORDS));
    f(["caption", "import", "--track", "t4", "--words", "words.json", "--style", "preset:bold"]);
    assertEqual(f(["status"]).timelines.map((tl) => tl.end), ["4"], "timeline length");

    step(3, `fin look --at ${AT.join(",")}`);
    const asideDir = join(tmp, "aside");
    let result;
    let jobDir;
    for (let attempt = 1; attempt <= 3 && !jobDir; attempt++) {
      // The job directory is removed once the job ends: copy its `in/` aside while it runs.
      rmSync(asideDir, { recursive: true, force: true });
      let done = false;
      const pending = finAsync(sd, project, ["look", "--at", AT.join(",")]).finally(() => (done = true));
      jobDir = await copyJobAside(jobs, asideDir, () => done);
      const r = await pending;
      if (r.status !== 0) throw new Error(`fin look exited ${r.status}: ${r.text} ${r.stderr}`);
      result = r.json;
      if (!jobDir) log(`attempt ${attempt}: the look ended before its job directory could be copied; again`);
    }
    if (!jobDir) throw new Error("could not copy the look's job directory aside in 3 attempts");
    assertEqual(result.frames.length, AT.length, "frames");
    assertEqual(result.frames.map((fr) => [fr.width, fr.height]), AT.map(() => [1080, 1920]), "frame sizes");
    const pngs = result.frames.map((fr) => fr.file);

    step(4, "skipped: the caption, not yet drawn");
    if (result.skipped.length === 0) throw new Error("no layer was skipped: the caption should be");
    for (const s of result.skipped) {
      if (s.kind !== "caption" || s.code !== "LAYER_UNAVAILABLE" || s.details.reason !== "notYet") throw new Error(`unexpected skipped entry ${JSON.stringify(s)}`);
    }

    step(5, "plan hashes equal plans.json");
    checkGolden("plans.json", result.frames.map((fr, i) => ({ at: AT[i], blake3: fr.plan })), { dir: GOLDEN_DIR, bless: blessing() });

    step(6, "frame ids read from the PNGs equal the nearest-frame rule");
    const cfr = Array.from({ length: 90 }, (_, i) => i);
    const vfr = vfrPts(60);
    const ids = [
      // The CFR clip, alone in the frame's top-left corner (the overlay and the dissolve are elsewhere or later).
      ...[0, 1, 2, 3].map((i) => ({ i, place: placeFlat, scale: 1.125, want: nearestFrame(cfr, [0, 15, 30, 45][i]) })),
      // The VFR clip on V1 from 2 s: exact midpoints between two of its frames.
      ...VFR_MIDPOINTS.map((tick, k) => ({ i: 5 + k, place: placeRotated, scale: 2, want: nearestFrame(vfr, tick) })),
    ];
    assertEqual(VFR_MIDPOINTS.map((tick) => vfr.filter((p) => p < tick).length - 1), [22, 27], "the frame before each midpoint");
    assertEqual(ids.slice(4).map((x) => x.want), [22, 27], "midpoints pick the earlier frame");
    for (const { i, place, scale, want } of ids) {
      assertEqual(readFrameId(readPng(pngs[i]), place, scale), want, `frame id at ${AT[i]}`);
    }

    step(7, "T1 against the CPU reference over the same job directory; T0 hashes");
    const ref = referenceRender(asideDir, pngs);
    for (const [i, r] of ref.entries()) {
      if (!r.t1.pass) throw new Error(`T1 at ${AT[i]}: ${JSON.stringify(r.t1)}`);
    }
    log(`T1 worst: PSNR ${Math.min(...ref.map((r) => r.t1.psnr ?? Infinity)).toFixed(1)} dB, max |Δ| ${Math.max(...ref.map((r) => r.t1.maxAbs))}`);
    t0(ref, AT, fixtures);

    step(8, "the same directory replayed by fin-workers media --replay writes the same PNGs");
    mkdirSync(join(asideDir, "out"), { recursive: true });
    const replayed = replay(join(ROOT, "packages", "fin-node", WIN ? "fin-workers.exe" : "fin-workers"), asideDir);
    assertEqual(replayed.frames.map((fr) => fr.plan), result.frames.map((fr) => fr.plan), "replayed plan hashes");
    for (const [i, fr] of replayed.frames.entries()) {
      if (!same(join(asideDir, "out", fr.file), pngs[i])) throw new Error(`the replayed ${fr.file} differs from the server's ${pngs[i]}`);
    }

    step(9, "a look whose worker is killed mid-job is retried and succeeds, leaving no strays");
    await killedLook(sd, project, tmp, jobs);

    step(10, "diagnostics: job.done carries the media object; media.capabilities and render.stats name no adapter");
    await diagnostics(server);

    log(`PASS look in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    try {
      if (server) await stopServer(server);
    } finally {
      removeTmp(tmp);
    }
  }
}

/** The journal's `job.done` of the two `media.look` jobs (the 7-frame look; the killed 64-frame one, run twice), and the media kinds. */
async function diagnostics(server) {
  const lookDone = async () => {
    const done = (await storedEntries(server, { kinds: ["job.done"] })).filter((e) => e.payload.kind === "media.look");
    return done.length >= 2 ? done : undefined;
  };
  let done;
  for (let i = 0; i < 100 && !done; i++) {
    done = await lookDone();
    if (!done) await new Promise((r) => setTimeout(r, 50));
  }
  if (!done) throw new Error("the journal holds fewer than two media.look job.done entries");
  const [first, killed] = done.slice(-2);
  assertEqual([first.kv, first.payload.outcome, first.payload.attempts], [2, "succeeded", 1], "the first look's job.done");
  const m = first.payload.media;
  assertEqual([m.codec, m.backend, m.hw, m.frames, m.cacheHit, m.copyPath, typeof m.resolution], ["av1", "dav1d", false, AT.length, false, "copy", "string"], "job.done.media");
  if (!Number.isInteger(m.rtFactorMilli) || m.rtFactorMilli <= 0) throw new Error(`job.done.media.rtFactorMilli: ${m.rtFactorMilli}`);
  assertEqual([killed.payload.outcome, killed.payload.attempts, killed.payload.media?.frames], ["succeeded", 2, 64], "the killed look's job.done: two attempts");
  const kinds = await storedEntries(server, { kinds: ["media.capabilities", "render.stats"] });
  for (const kind of ["media.capabilities", "render.stats"]) {
    if (!kinds.some((e) => e.kind === kind)) throw new Error(`no ${kind} entry in the journal`);
  }
  const text = JSON.stringify(kinds);
  if (/llvmpipe|vulkan device|\/dev\/|\.mp4/i.test(text)) throw new Error(`a media diagnostics entry names an adapter or a path: ${text.slice(0, 400)}`);
}

/** Checks `frames` (the reference's hashes) against the T0 entry for these fixtures, or records one when blessing. */
function t0(ref, at, fixtures) {
  const path = join(GOLDEN_DIR, "t0.json");
  const doc = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : { note: T0_NOTE, entries: [] };
  const frames = ref.map((r, i) => ({ at: at[i], blake3: r.blake3 }));
  const entry = doc.entries.find((e) => e.fixtures === fixtures);
  if (blessing()) {
    const fresh = { class: machineClass(), fixtures, frames };
    doc.note = T0_NOTE;
    doc.entries = [...doc.entries.filter((e) => e.fixtures !== fixtures), fresh];
    mkdirSync(GOLDEN_DIR, { recursive: true });
    writeFileSync(path, `${JSON.stringify(doc, null, 2)}\n`);
    log(`blessed t0.json (${fresh.class})`);
  } else if (!entry) {
    if (process.env.E2E_LOOK_REQUIRE_T0 === "1") throw new Error(`no T0 entry for these fixtures (${machineClass()}, sha256 ${fixtures}); bless one with E2E_LOOK_BLESS=1`);
    log(`T0: no entry for these fixtures (${machineClass()}, sha256 ${fixtures.slice(0, 16)}…): not compared`);
  } else {
    assertEqual(frames, entry.frames, `T0 hashes (entry recorded on ${entry.class})`);
    log(`T0: ${frames.length} hashes equal the entry recorded on ${entry.class}`);
  }
}

const T0_NOTE =
  "T0 hashes (BLAKE3 of the CPU reference's RGBA8) of the e2e look scenario, per fixture set. An entry is keyed by the SHA-256 of the two clips make_fixture wrote: SVT-AV1's bytes may differ between machine classes (see fin-codecs/tests/golden/encode.json), and the reference is only comparable over identical decoded pixels. dav1d is bit-exact, so entries with the same fixtures digest must agree on every OS. A machine whose digest has no entry compares nothing (E2E_LOOK_REQUIRE_T0=1 fails instead); E2E_LOOK_BLESS=1 records one.";

/** 64 frames at a quarter of the pixels, the worker killed after the first PNG: the job engine retries it on a fresh worker. */
async function killedLook(sd, project, tmp, jobs) {
  const out = join(tmp, "killed-look");
  const at = Array.from({ length: 64 }, (_, i) => `f${i}`);
  const pending = finAsync(sd, project, ["look", "--at", at.join(","), "--size", "540x960", "--out", out]);
  await waitFor("the first PNG of the job", () => pngsBelow(jobs).length > 0, 60_000, () => `: ${pngsBelow(jobs).length} found`, 5);
  const workers = mediaWorkers();
  if (workers.length === 0) throw new Error("no fin-workers media process to kill");
  for (const pid of workers) process.kill(pid, "SIGKILL");
  log(`killed media worker(s) ${workers.join(", ")}`);
  const r = await pending;
  if (r.status !== 0) throw new Error(`the killed look did not succeed (exit ${r.status}): ${r.text} ${r.stderr}`);
  assertEqual(r.json.frames.length, 64, "frames after the retry");
  assertEqual([r.json.stats.deviceReused, r.json.stats.workerJobs], [false, 1], "a fresh worker rendered the retry");
  const files = readdirSync(out).sort();
  assertEqual(files, r.json.frames.map((fr) => fr.file.split(/[\\/]/).pop()).sort(), "files in the look directory");
  if (files.some((n) => !/^[A-Za-z0-9_-]+-\d+_\d+-\d+x\d+-[0-9a-f]{8}\.png$/.test(n))) throw new Error(`stray files in the look directory: ${files.join(", ")}`);
}
