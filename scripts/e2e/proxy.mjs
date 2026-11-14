// The "proxy" scenario (6B-1 Task 20; sub-project 6 spec §7): proxies, thumbnails, the media
// cache and its HTTP route end to end, through finisherd's real `media` worker. Clips made at run
// time (no media file is committed): an AV1 CFR clip and an AV1 VFR clip rotated 90° (`make_fixture`,
// everywhere), a third AV1 clip only the library holds, and an H.264 clip whose row depends on the
// machine: macOS and Windows proxy one written by the OS encoder once its backend reports H.264
// (6B-1 Tasks 5 and 7; skipped with a notice until then, `FIN_REQUIRE_PLATFORM_CODECS=1` fails),
// and Linux refuses a container-only H.264 file with `CODEC_UNAVAILABLE` and `laterBatch` guidance
// (6B-2 adds VA-API). Checked: the library's `proxy540` stage makes the `540` proxies by itself;
// `fin media proxy` answers them from the cache and makes the `1080` ones; `fin look` at seven
// times (the VFR clip's exact midpoints included) reads the proxies (`source: proxy`) with the
// frame ids and plans of `--originals`; `fin media thumbs --strip` and its index; `media.url` and
// an HTTP range request returning the exact bytes of the entry's file; `fin cache status` by
// recipe; `fin cache gc` refused to `fin` (`HUMAN_ONLY`), the console's `gc --to 0` evicting only
// what the open project does not pin, and everything once it closes; the journal's media entries
// (the Task 19 fields in `task19Diagnostics`; refusal guidance waits on Task 8). Needs a GPU adapter
// (`FIN_GPU`): skipped without one, unless `FIN_REQUIRE_GPU=1`.
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { ROOT, assertEqual, consoleLibraryAdd, consoleRpc, errorCode, fin, finRaw, live, log, removeTmp, scenarioTmp, socketCall, startServer, stopServer, storedEntries, waitFor } from "./lib.mjs";
import { AT } from "./look.mjs";
import { nearestFrame, placeFlat, placeRotated, readFrameId, readPng, vfrPts } from "./look-lib.mjs";
import { TASK19, TASK8, h264Guidance, h264Mode, h264Row, httpGet, presentDiagnostics, task19Diagnostics, writeH264Stub } from "./proxy-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/** The VFR clip's two exact midpoints among `AT` (look.mjs): source ticks 35 and 43 pick frames 22 and 27. */
const VFR_MIDPOINTS = [35, 43];

/** Runs `make_fixture` (built by `scripts/e2e.mjs`) into `out`. */
function makeFixture(out, ...opts) {
  const r = spawnSync("cargo", ["run", "-q", "--release", "-p", "fin-codecs", "--features", "testing", "--example", "make_fixture", "--", out, ...opts], { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.status !== 0) throw new Error(`make_fixture ${out} failed (exit ${r.status}): ${r.stderr}`);
}

/** `fin library show` of one file until `pred` holds on it. */
async function fileUntil(sd, cwd, lib, rel, what, pred, timeoutMs = 120_000) {
  let last;
  await waitFor(
    what,
    () => {
      const r = finRaw(sd, cwd, ["library", "show", lib, rel]);
      last = r.json;
      return r.status === 0 && pred(r.json);
    },
    timeoutMs,
    () => `: ${JSON.stringify(last).slice(0, 600)}`,
    500,
  );
  return last;
}

/** The `proxy540` row of a `fin library show` answer (`stages: [{stage, state, code, version, …}]`). */
const stageOf = (shown) => (shown?.stages ?? []).find((s) => s.stage === "proxy540");
const stageDone = (s) => s?.state === "done";
/** The stage passed the file over (`appliesTo` false: nothing here decodes it). */
const stageSkipped = (s) => s?.state === "skipped" && s.code === "NOT_APPLICABLE";

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-proxy-");
  live.tmp = tmp;
  const sd = join(tmp, "server");
  const root = join(tmp, "footage");
  const project = join(tmp, "proxy");
  let server;
  try {
    mkdirSync(sd, { recursive: true });
    mkdirSync(root, { recursive: true });
    writeFileSync(join(sd, "settings.json"), JSON.stringify({ fonts: { autoFetch: false } }));
    server = await startServer(sd);
    const caps = fin(sd, tmp, ["media", "caps"]);
    if (!caps.gpu) {
      if (process.env.FIN_REQUIRE_GPU === "1") throw new Error("FIN_REQUIRE_GPU=1 but the media worker opened no GPU adapter");
      log("SKIP proxy: no GPU adapter (set FIN_GPU, or FIN_REQUIRE_GPU=1 to fail)");
      return;
    }
    const h264 = h264Mode(h264Row(caps));
    log(`adapter ${caps.gpu.backend} ${caps.gpu.deviceType}${caps.gpu.software ? " (software)" : ""}; H.264 row: ${h264}`);

    step(1, "fixtures: AV1 540p30 CFR, AV1 540p VFR rotated 90°, a library-only AV1 clip, and the H.264 clip of this platform");
    makeFixture(join(root, "a.mp4"), "--size", "960x540", "--rate", "30", "--frames", "90", "--seed", "1");
    makeFixture(join(root, "b.mp4"), "--size", "960x540", "--rate", "30", "--frames", "60", "--vfr", "--rotate", "90", "--seed", "2");
    makeFixture(join(root, "d.mp4"), "--size", "640x360", "--rate", "30", "--frames", "30", "--seed", "4");
    // macOS and Windows: the OS encoder writes it (6B-1 Tasks 5 and 7 give `make_fixture` its `--codec h264`).
    if (h264 === "encode") makeFixture(join(root, "h.mp4"), "--codec", "h264", "--size", "960x540", "--rate", "30", "--frames", "30", "--seed", "5");
    else if (h264 === "refuse") writeH264Stub(join(root, "h.mp4"));
    else log("SKIP the H.264 rows: this machine's report names no H.264 encoder (6B-1 Tasks 5 and 7; FIN_REQUIRE_PLATFORM_CODECS=1 fails)");
    const av1Files = ["a.mp4", "b.mp4", "d.mp4"];
    const files = h264 === "skip" ? av1Files : [...av1Files, "h.mp4"];

    step(2, "a library over the clips: its scan, then the proxy540 stage makes every AV1 file's 540 proxy");
    const added = await consoleLibraryAdd(server, root, { name: "Footage" });
    if (added.job) fin(sd, tmp, ["wait", added.job.id, "--timeout", "120"]);
    for (const rel of av1Files) await fileUntil(sd, tmp, "Footage", rel, `the proxy540 stage of ${rel}`, (s) => stageDone(stageOf(s)));
    if (h264 === "refuse") {
      const shown = await fileUntil(sd, tmp, "Footage", "h.mp4", "the proxy540 stage of h.mp4", (s) => ["done", "skipped", "failed"].includes(stageOf(s)?.state));
      if (!stageSkipped(stageOf(shown))) throw new Error(`the proxy540 stage took the H.264 file nothing here decodes: ${JSON.stringify(stageOf(shown))}`);
    } else if (h264 === "encode") {
      await fileUntil(sd, tmp, "Footage", "h.mp4", "the proxy540 stage of h.mp4", (s) => stageDone(stageOf(s)));
    }

    step(3, "a project over the library's files: the 540 proxies are cache hits, the 1080 ones are made");
    fin(sd, tmp, ["project", "create", project, "--name", "Proxy"]);
    const f = (args) => fin(sd, project, args);
    const projectId = f(["status"]).projectId ?? f(["status"]).project?.id;
    if (typeof projectId !== "string") throw new Error("fin status names no project id");
    const ids = files.filter((n) => n !== "d.mp4").map((n) => ({ name: n, id: f(["asset", "add", join(root, n)]).bindings?.$asset }));
    const asset = Object.fromEntries(ids.map(({ name, id }) => [name, id]));
    assertEqual([asset["a.mp4"], asset["b.mp4"]], ["a1", "a2"], "asset ids");
    const keys = {};
    for (const n of ["a.mp4", "b.mp4", ...(h264 === "encode" ? ["h.mp4"] : [])]) {
      const p540 = f(["media", "proxy", asset[n], "--tier", "540"]);
      assertEqual([p540.state, p540.cached], ["ready", true], `the stage's 540 proxy of ${n}`);
      const p1080 = f(["media", "proxy", asset[n], "--tier", "1080"]);
      assertEqual([p1080.tier, p1080.state], ["1080", "ready"], `the 1080 proxy of ${n}`);
      keys[n] = { t540: p540.key, t1080: p1080.key };
      const status = f(["media", "status", asset[n]]);
      assertEqual(status.tiers.map((t) => [t.tier, t.state]).sort(), [["1080", "ready"], ["540", "ready"]], `fin media status ${n}`);
    }
    if (h264 === "refuse") {
      const r = finRaw(sd, project, ["media", "proxy", asset["h.mp4"], "--tier", "540"]);
      assertEqual([r.status, errorCode(r.text)], [5, "CODEC_UNAVAILABLE"], "the H.264 proxy on Linux");
      // The fields the refusal must carry; guidance (Task 8) may join them.
      assertEqual([r.json.error.details?.codec, r.json.error.details?.direction], ["h264", "decode"], "CODEC_UNAVAILABLE details");
      if (TASK8) h264Guidance(h264Row(caps), r.json);
      else log("NOTE: laterBatch 6B-2 guidance on the H.264 row and refusal waits on 6B-1 Task 8 (h264Guidance, TASK8 in proxy-lib.mjs)");
    }

    step(4, `fin look --at ${AT.join(",")}: through the proxies, the frame ids and plans of --originals`);
    f(["clip", "insert", "--track", "t1", "--asset", "a1", "--at", "0", "--dur", "2s"]);
    f(["clip", "insert", "--track", "t1", "--asset", "a2", "--at", "end", "--in", "1s", "--dur", "2s"]);
    const originals = f(["look", "--at", AT.join(","), "--originals"]);
    const proxied = f(["look", "--at", AT.join(",")]);
    assertEqual(originals.frames.map((fr) => fr.source), AT.map(() => "original"), "--originals reads the originals");
    assertEqual(proxied.frames.map((fr) => fr.source), AT.map(() => "proxy"), "the look reads the proxies");
    assertEqual(proxied.frames.map((fr) => fr.plan), originals.frames.map((fr) => fr.plan), "the same plans");
    const cfr = Array.from({ length: 90 }, (_, i) => i);
    const vfr = vfrPts(60);
    const want = [
      ...[0, 15, 30, 45].map((tick, i) => ({ i, place: placeFlat, scale: 1.125, want: nearestFrame(cfr, tick) })),
      // `2s`: the cut into the rotated VFR clip, its source at `--in 1s` (tick 30 at 30 ticks/s).
      { i: 4, place: placeRotated, scale: 2, want: nearestFrame(vfr, 30) },
      ...VFR_MIDPOINTS.map((tick, k) => ({ i: 5 + k, place: placeRotated, scale: 2, want: nearestFrame(vfr, tick) })),
    ];
    assertEqual(want.map((w) => w.i), AT.map((_, i) => i), "every look time has its frame id checked");
    for (const { i, place, scale, want: id } of want) {
      const [o, p] = [originals, proxied].map((look) => readFrameId(readPng(look.frames[i].file), place, scale));
      assertEqual([o, p], [id, id], `frame id at ${AT[i]} (original, proxy)`);
    }

    step(5, "fin media thumbs --strip: one sprite and its index");
    const strip = f(["media", "thumbs", "a1", "--strip"]);
    assertEqual(strip.state, "ready", "the strip");
    const names = (strip.files ?? []).map((x) => x.name).sort();
    if (!names.includes("strip.json") || !names.some((n) => n.endsWith(".png"))) throw new Error(`the strip's files: ${JSON.stringify(names)}`);

    step(6, "media.url and HTTP: a range of the 1080 proxy and the whole strip index, byte for byte");
    const entryFile = (key, file) => {
      const base = join(sd, "cache", "media");
      const dir = readdirSync(base, { recursive: true }).find((rel) => basename(String(rel)) === key);
      if (dir === undefined) throw new Error(`no cache directory for ${key}`);
      return readFileSync(join(base, String(dir), file));
    };
    const proxyKey = keys["a.mp4"].t1080;
    const mp4 = entryFile(proxyKey, "proxy.mp4");
    const { url } = await socketCall(server, "media.url", { key: proxyKey, file: "proxy.mp4" });
    const ranged = await httpGet(server.info.wsPort, url, { range: "bytes=100-1123" });
    assertEqual([ranged.status, ranged.headers["content-range"], ranged.headers["content-type"]], [206, `bytes 100-1123/${mp4.length}`, "video/mp4"], "the ranged response");
    if (!ranged.body.equals(mp4.subarray(100, 1124))) throw new Error("the ranged bytes differ from the entry's proxy.mp4");
    const index = await socketCall(server, "media.url", { key: strip.key, file: "strip.json" });
    const whole = await httpGet(server.info.wsPort, index.url);
    assertEqual(whole.status, 200, "the strip index over HTTP");
    if (!whole.body.equals(entryFile(strip.key, "strip.json"))) throw new Error("the strip index over HTTP differs from the entry's");
    const strips = JSON.parse(whole.body.toString("utf8"));
    assertEqual(strips.sprites?.length, 1, "a 3 s source has one sprite");

    step(7, "fin cache status lists the entries by recipe");
    const before = fin(sd, tmp, ["cache", "status"]);
    for (const recipe of ["proxy-540", "proxy-1080"]) {
      if (!(before.byRecipe?.[recipe]?.entries > 0)) throw new Error(`fin cache status has no ${recipe} entry: ${JSON.stringify(before)}`);
    }
    const proxies = Object.values(keys).length;
    // The 540 of a, b (and h), d's 540, their 1080s, the strip.
    assertEqual(before.entries, 2 * proxies + 1 + 1, "cache entries");

    step(8, "fin cache gc is the owner's; the console's gc --to 0 keeps what the open project pins, then evicts all once it closes");
    const refused = finRaw(sd, tmp, ["cache", "gc", "--to", "0"]);
    assertEqual([refused.status, errorCode(refused.text)], [7, "HUMAN_ONLY"], "fin cache gc");
    const open = await consoleRpc(server, "media.cache.gc", { toGiB: 0 });
    // Only d.mp4's 540 proxy serves no open project.
    assertEqual(open.evicted, 1, "evicted while the project is open");
    assertEqual(fin(sd, tmp, ["cache", "status"]).entries, before.entries - 1, "entries kept for the open project");
    assertEqual((await socketCall(server, "project.close", { projectId })).closed, true, "project.close");
    const closed = await consoleRpc(server, "media.cache.gc", { toGiB: 0 });
    assertEqual(closed.evicted, before.entries - 1, "evicted once the project closed");
    assertEqual(fin(sd, tmp, ["cache", "status"]).entries, 0, "an empty cache");

    step(9, "the journal: media.capabilities and job.done of the proxy and thumbs jobs");
    const entries = await storedEntries(server, { kinds: ["job.done", "media.capabilities"] });
    // The stage's three 540s and the two (or three) 1080s ran as jobs.
    presentDiagnostics(entries, { proxyJobs: 3 + proxies });
    if (TASK19) task19Diagnostics(entries, { proxyFrames: [90, 60] });
    else log("NOTE: media.capabilities v2 and job.done.media of media.proxy/media.thumbs wait on 6B-1 Task 19 (task19Diagnostics, TASK19 in proxy-lib.mjs)");

    log(`PASS proxy in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  } finally {
    try {
      if (server) await stopServer(server);
    } finally {
      removeTmp(tmp);
    }
  }
}
