// The "library" scenario (plan 5D Task 15): a library added from the console, its first scan
// waited on with `fin wait`, then `fin library files|search|show` over what it catalogued.
// Files are renamed, moved, rewritten and deleted and a `fin library scan` sees each change;
// NFC/NFD twins are both listed and a path naming both is `PATH_AMBIGUOUS`; placeholders (the
// test overlay's dataless folder) and a Nextcloud stub stay `cloudOnly` and are never opened (the
// overlay's open log); a nested mount unmounted through the overlay goes offline with nothing
// missing until it is mounted again; and every scan leaves the tree byte for byte as it was (`root_is_byte_identical_after_scan`).
//
// The server runs fin-node's test-hooks build, whose `fin-workers` reads the overlay (a release
// worker never does). Nothing under the root is written by the server: the test process writes it.
import { existsSync, mkdirSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { TEST_NATIVE_DIR, writeFixture } from "./fixture.mjs";
import { assertEqual, consoleLibraryAdd, fin, live, log, removeTmp, scenarioTmp, startServer, stopServer, waitFor } from "./lib.mjs";
import { allFiles, openLog, put, scanAndWait, show, showFails, treeHash } from "./library-lib.mjs";

const step = (n, text) => log(`step ${n}: ${text}`);

/** The NFC and NFD spellings of one name (`café.mov`). */
const NFC = "café.mov";
const NFD = "café.mov";

/** The test overlay at the root: a dataless folder and a nested mount, with an open log outside the root. */
function writeOverlay(root, openLogPath, { unmounted = false } = {}) {
  // The overlay logs opens of the paths it lists: the stub (both names) and `a.mov` are listed with
  // nothing changed, so an open of the stub would be logged, and `a.mov`'s probe shows the log works.
  const paths = {
    cloud: { placeholder: "dataless" },
    disk: unmounted ? { unmounted: true } : { mountRoot: true },
    "a.mov": {},
    "stub.mov": {},
    "stub.mov.nextcloud": {},
  };
  writeFileSync(join(root, ".fin-fake.json"), JSON.stringify({ openLog: openLogPath, paths }));
}

/** Folders whose files the probe stage takes (the dataless folder has none). */
const PROBED_DIRS = ["", "clips", "move", "rewrite", "del", "disk", "disk/sub"];

/** Waits until the probe stage has taken every file: no row is `present` without a fingerprint. */
async function probePass(sd, cwd) {
  await waitFor(
    "the probe pass",
    () => PROBED_DIRS.every((dir) => allFiles(sd, cwd, "Footage", dir === "" ? undefined : dir).files.every((f) => f.state !== "present" || f.fp !== null)),
    60_000,
    () => "",
    200,
  );
}

export async function run() {
  const started = Date.now();
  const tmp = scenarioTmp("finisher-e2e-lib-");
  live.tmp = tmp;
  const sd = join(tmp, "s");
  const root = join(tmp, "Footage");
  const opens = join(tmp, "opens.ndjson");
  let server;
  try {
    step(1, "a tree: media, folders to change, twins, a dataless folder, a Nextcloud stub, a nested mount");
    mkdirSync(root);
    await writeFixture(join(root, "a.mov"), 2);
    for (const rel of ["clips/b.mov", "clips/interview_take3.final.mov", "move/m.mov", "move/stays.mov", "rewrite/r.mov", "del/d.mov", "del/stays.mov", "cloud/c1.mov", "cloud/deep/c2.mov", "disk/x.mov", "disk/sub/y.mov"]) put(root, rel);
    put(root, "stub.mov.nextcloud", "");
    put(root, NFC);
    put(root, NFD);
    const twins = readdirSync(root).filter((n) => n.normalize("NFC") === NFC).length === 2;
    if (!twins) log("the filesystem unifies NFC and NFD names: the twin checks are skipped");
    writeOverlay(root, opens);

    server = await startServer(sd, { FIN_NODE_BINARY_DIR: TEST_NATIVE_DIR });
    const cwd = tmp;

    step(2, "the console adds the library; fin waits for its first scan");
    let before = treeHash(root);
    const added = await consoleLibraryAdd(server, root, { name: "Footage" });
    if (added.job === undefined) throw new Error(`library.add queued no scan: ${JSON.stringify(added)}`);
    const lib = added.library.id;
    const waited = fin(sd, cwd, ["wait", added.job.id, "--timeout", "120"]);
    assertEqual(waited.jobs[0].state, "succeeded", "the first scan");
    // The probe stage runs after the scan: the real MP4 gets its fingerprint and duration.
    const a = await waitFor("a.mov probed", () => {
      const f = show(sd, cwd, "Footage", "a.mov");
      return f.fp !== null ? f : undefined;
    }, 60_000, () => "", 200);
    if (!a.fp.startsWith("fp1:") || a.durationMs === null || a.durationMs < 1900) throw new Error(`a.mov probe: ${JSON.stringify(a)}`);
    await probePass(sd, cwd);
    assertEqual(treeHash(root), before, "root_is_byte_identical_after_scan (first scan and probes)");

    step(3, "fin library ls, files, search and show");
    const ls = fin(sd, cwd, ["library", "ls"]).libraries;
    assertEqual(ls.map((l) => [l.id, l.name, l.storage]), [[lib, "Footage", "local"]], "fin library ls");
    const top = allFiles(sd, cwd, "Footage");
    const topNames = top.files.map((f) => f.display).sort();
    const wantTop = ["a.mov", "stub.mov", ...(twins ? [NFC, NFC] : [NFC])].sort();
    assertEqual(topNames, wantTop, "root files (the twins as two rows, the stub as the file it stands for)");
    assertEqual(top.dirs.map((d) => d.name).sort(), ["clips", "cloud", "del", "disk", "move", "rewrite"], "root folders");
    const hits = fin(sd, cwd, ["search", "interview"]).hits;
    assertEqual(hits.map((h) => h.display), ["clips/interview_take3.final.mov"], "fin search interview");
    assertEqual(fin(sd, cwd, ["library", "search", "interv", "--lib", "Footage"]).hits.length, 1, "fin library search (prefix)");
    const b = show(sd, cwd, "Footage", "clips/b.mov");
    // Not media: catalogued, probed once and marked so (a probe never retries it until it changes).
    assertEqual([b.state, b.code, b.size], ["probeFailed", "UNSUPPORTED_CONTAINER", Buffer.byteLength("bytes of clips/b.mov")], "clips/b.mov");

    step(4, "twins are both listed; a path naming both is PATH_AMBIGUOUS");
    if (twins) {
      const twinRows = top.files.filter((f) => f.name === NFC);
      assertEqual(twinRows.length, 2, "twin rows");
      if (!twinRows.every((f) => f.collision)) throw new Error(`twins not flagged: ${JSON.stringify(twinRows)}`);
      const err = showFails(sd, cwd, "Footage", NFC, "PATH_AMBIGUOUS");
      assertEqual([...err.details.fileIds].sort(), twinRows.map((f) => f.id).sort(), "the ambiguity names both rows");
    }

    step(5, "the dataless folder and the stub stay cloudOnly; no placeholder was ever opened");
    // A dataless folder is never listed: it is catalogued `cloudOnly`, with nothing below it.
    const cloud = top.dirs.find((x) => x.name === "cloud");
    assertEqual([cloud.state, cloud.placeholder], ["cloudOnly", "dataless"], "the dataless folder");
    assertEqual(allFiles(sd, cwd, "Footage", "cloud").files, [], "nothing catalogued under the dataless folder");
    assertEqual(show(sd, cwd, "Footage", "stub.mov").state, "cloudOnly", "stub.mov");
    const stub = show(sd, cwd, "Footage", "stub.mov");
    assertEqual(stub.placeholder, "nextcloud", "the stub's placeholder");
    const placeholderOpens = () => openLog(opens).filter((l) => l.path.startsWith("cloud") || l.path.startsWith("stub"));
    assertEqual(placeholderOpens(), [], "open counter of the placeholders");
    if (!openLog(opens).some((l) => l.op === "open" && l.path === "a.mov")) throw new Error(`the open log saw no probe of a.mov: is the overlay read? ${JSON.stringify(openLog(opens))}`);

    step(6, "rename, move, rewrite and delete; a full scan sees each change");
    const ids = {
      a: a.id,
      m: show(sd, cwd, "Footage", "move/m.mov").id,
      r: show(sd, cwd, "Footage", "rewrite/r.mov"),
    };
    renameSync(join(root, "a.mov"), join(root, "a renamed.mov"));
    renameSync(join(root, "move", "m.mov"), join(root, "clips", "m.mov"));
    writeFileSync(join(root, "rewrite", "r.mov"), "rewritten, and longer than before");
    rmSync(join(root, "del", "d.mov"));
    before = treeHash(root);
    scanAndWait(sd, cwd, "Footage");
    await probePass(sd, cwd);
    assertEqual(treeHash(root), before, "root_is_byte_identical_after_scan (second scan)");
    const renamed = show(sd, cwd, "Footage", "a renamed.mov");
    assertEqual([renamed.id, renamed.state, renamed.fp], [ids.a, "present", a.fp], "a rename keeps the row, its history and fingerprint");
    assertEqual(show(sd, cwd, "Footage", "clips/m.mov").id, ids.m, "a move keeps the row");
    const r = show(sd, cwd, "Footage", "rewrite/r.mov");
    assertEqual([r.id, r.size], [ids.r.id, Buffer.byteLength("rewritten, and longer than before")], "a rewrite updates the row");
    if (!["present", "probeFailed"].includes(r.state) || r.fp === ids.r.fp) throw new Error(`a rewrite keeps no stale probe: ${JSON.stringify(r)}`);
    if (!(r.lastChange > ids.r.lastChange)) throw new Error(`rewrite lastChange did not move: ${ids.r.lastChange} → ${r.lastChange}`);
    const d = allFiles(sd, cwd, "Footage", "del", "missing").files;
    assertEqual(d.map((f) => [f.display, f.state]), [["del/d.mov", "missing"]], "a deleted file is missing");
    // (A folder left empty would be `DIR_EMPTY_SUSPECT` and mark nothing until confirmed, so each keeps a file.)
    assertEqual(allFiles(sd, cwd, "Footage", "move").files.map((f) => f.display), ["move/stays.mov"], "the moved file left its folder");

    step(7, "the nested mount unmounted through the overlay: offline, nothing missing");
    writeOverlay(root, opens, { unmounted: true });
    before = treeHash(root);
    scanAndWait(sd, cwd, "Footage");
    assertEqual(treeHash(root), before, "root_is_byte_identical_after_scan (unmounted)");
    const status = fin(sd, cwd, ["library", "status", "Footage"]).library;
    if (!status.warnings.some((w) => w.code === "ROOT_UNMOUNTED" && w.dir === "disk")) throw new Error(`no ROOT_UNMOUNTED warning: ${JSON.stringify(status.warnings)}`);
    for (const rel of ["disk/x.mov", "disk/sub/y.mov"]) assertEqual(show(sd, cwd, "Footage", rel).state, "offline", rel);
    assertEqual((status.counts.missing ?? 0), 1, "missing files (only del/d.mov)");

    step(8, "mounted again: the next scan brings its files back");
    writeOverlay(root, opens);
    scanAndWait(sd, cwd, "Footage");
    for (const rel of ["disk/x.mov", "disk/sub/y.mov"]) {
      const f = show(sd, cwd, "Footage", rel);
      if (!["present", "probeFailed"].includes(f.state)) throw new Error(`${rel} not back after the remount: ${JSON.stringify(f)}`);
    }
    const back = fin(sd, cwd, ["library", "status", "Footage"]).library;
    assertEqual(back.counts.missing ?? 0, 1, "missing files after the remount (only del/d.mov)");
    if (back.warnings.some((w) => w.code === "ROOT_UNMOUNTED")) throw new Error(`ROOT_UNMOUNTED kept: ${JSON.stringify(back.warnings)}`);
    assertEqual(placeholderOpens(), [], "open counter of the placeholders after every scan");

    log(`library scenario passed in ${Date.now() - started} ms`);
  } finally {
    if (server) await stopServer(server);
    if (existsSync(tmp)) removeTmp(tmp);
    live.tmp = undefined;
  }
}
