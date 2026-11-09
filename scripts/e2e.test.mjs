// Tests for the e2e script's own checks and cleanup (the full run is `pnpm e2e`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { checkRevs, parseFrames } from "./e2e.mjs";
import { clipsOf, failure } from "./e2e/history-lib.mjs";
import { contentFields, contentProblems, kindDef, leaks, loadContentFields, parseJsonl } from "./e2e/diagnostics-lib.mjs";
import { audioSummary, blessing, checkGolden, cubicBezier, engineProblem, firstDifference, frameGolden, hashTree, layerSummary } from "./e2e/reel-lib.mjs";
import { realCache } from "./e2e/reel-real.mjs";
import { createServer } from "node:net";
import { createServer as createHttpServer } from "node:http";
import { statSync } from "node:fs";
import { errorCode, handshakeMac, rawConnect } from "./e2e/lib.mjs";
import { secretHits, wsUpgrade, writePrivate } from "./e2e/auth-lib.mjs";

test("parseFrames splits one message carrying several NDJSON frames", () => {
  const text = `${JSON.stringify({ _tag: "Chunk", requestId: "1", values: [] })}\n${JSON.stringify({ _tag: "Exit", requestId: "1" })}\n`;
  assert.deepEqual(
    parseFrames(text).map((f) => f._tag),
    ["Chunk", "Exit"],
  );
  assert.deepEqual(parseFrames("\n"), []);
});

test("checkRevs accepts every committed rev exactly once, in order", () => {
  assert.equal(checkRevs([1, 2, 3], [1, 2, 3]), undefined);
});

test("checkRevs rejects a gap, a duplicate and a reordering", () => {
  assert.match(checkRevs([1, 3], [1, 2, 3]), /subscriber saw revs \[1,3\]/);
  assert.match(checkRevs([1, 2, 2, 3], [1, 2, 3]), /subscriber saw revs/);
  assert.match(checkRevs([2, 1, 3], [1, 2, 3]), /subscriber saw revs/);
});

test("checkRevs rejects non-contiguous commits from fin", () => {
  assert.match(checkRevs([1, 3], [1, 3]), /not contiguous/);
});

// The script itself, with a fake server that never writes finisherd.json and records its pid.
const SCRIPT = fileURLToPath(new URL("./e2e.mjs", import.meta.url));
const FAKE_SERVER = "require('node:fs').writeFileSync(process.argv[1], String(process.pid)); setInterval(() => {}, 1000);";

function runWithFakeServer(env) {
  const dir = mkdtempSync(join(tmpdir(), "e2e-test-"));
  try {
    const pidFile = join(dir, "pid");
    const r = spawnSync(process.execPath, [SCRIPT, "--skip-build"], {
      encoding: "utf8",
      timeout: 30_000,
      env: { ...process.env, E2E_SERVER_CMD: JSON.stringify([process.execPath, "-e", FAKE_SERVER, pidFile]), ...env },
    });
    const pid = existsSync(pidFile) ? Number(readFileSync(pidFile, "utf8")) : 0;
    return { ...r, pid };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

test("a server that never becomes ready is killed before the failure is reported", () => {
  const r = runWithFakeServer({ E2E_START_TIMEOUT_MS: "1000" });
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /e2e: FAIL: timed out waiting for finisherd\.json/);
  assert.ok(r.pid > 0, `the fake server never started: ${r.stderr}`);
  assert.equal(alive(r.pid), false, `server pid ${r.pid} still running`);
});

test("the watchdog kills the server and fails the run at the deadline", () => {
  const r = runWithFakeServer({ E2E_START_TIMEOUT_MS: "60000", E2E_DEADLINE_MS: "1500" });
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /e2e: FAIL: watchdog: the run did not finish within 1\.5s; killed finisherd/);
  assert.ok(r.pid > 0, `the fake server never started: ${r.stderr}`);
  assert.equal(alive(r.pid), false, `server pid ${r.pid} still running`);
});

test("clipsOf lists every track's clips as id:start+duration", () => {
  const snapshot = { project: { timelines: [{ tracks: [{ id: "t1", items: [{ id: "c1", start: "0", duration: "4" }] }, { id: "t2", items: [] }] }] } };
  assert.deepEqual(clipsOf(snapshot), { t1: ["c1:0+4"], t2: [] });
});

test("failure checks the exit status and the error code of a refused fin call", () => {
  const refused = { status: 4, json: { error: { code: "CONFLICT" } }, text: "{}" };
  assert.equal(failure(refused, "CONFLICT", 4, "merge").code, "CONFLICT");
  assert.throws(() => failure(refused, "NOT_FOUND", 4, "merge"), /expected exit 4 NOT_FOUND/);
  assert.throws(() => failure({ ...refused, status: 3 }, "CONFLICT", 4, "merge"), /expected exit 4 CONFLICT/);
});

test("reel goldens are blessed only on request and never under CI", () => {
  assert.equal(blessing({}), false);
  assert.equal(blessing({ E2E_REEL_BLESS: "1" }), true);
  assert.throws(() => blessing({ E2E_REEL_BLESS: "1", CI: "true" }), /refused under CI/);
});

test("checkGolden writes when blessing, compares otherwise, and leaves the actual value on a mismatch", () => {
  const dir = mkdtempSync(join(tmpdir(), "reel-golden-"));
  try {
    assert.throws(() => checkGolden("g.json", { a: 1 }, { dir, bless: false }), /no golden g\.json/);
    checkGolden("g.json", { a: [1, 2] }, { dir, bless: true });
    checkGolden("g.json", { a: [1, 2] }, { dir, bless: false });
    assert.throws(() => checkGolden("g.json", { a: [1, 3] }, { dir, bless: false }), /\$\.a\[1\]: got 3, expected 2/);
    assert.deepEqual(JSON.parse(readFileSync(join(dir, "g.json.actual"), "utf8")), { a: [1, 3] });
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("checkGolden writes the full plans beside the golden on a mismatch", () => {
  const dir = mkdtempSync(join(tmpdir(), "reel-golden-"));
  try {
    checkGolden("frames.json", [1], { dir, bless: true });
    assert.throws(() => checkGolden("frames.json", [2], { dir, bless: false, plans: [{ p: 1 }] }), /frames\.plans\.actual\.json/);
    assert.deepEqual(JSON.parse(readFileSync(join(dir, "frames.plans.actual.json"), "utf8")), [{ p: 1 }]);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("frameGolden digests the plan without its text engine and summarises the captions", () => {
  const layout = { lines: [{}, {}], words: [{}, {}, {}], faces: ["f1"], width: 10, height: 20, missing: [] };
  const plan = { layers: [{ item: "cap1", kind: "caption", track: "t4", opacity: 1, caption: { layout } }] };
  const a = frameGolden("1s", { ...plan, textEngine: "fin-text/0.0.0 rev/1 x" });
  const b = frameGolden("1s", { ...plan, textEngine: "fin-text/0.0.0 rev/2 y" });
  assert.equal(a.sha256, b.sha256);
  assert.deepEqual(a.captions, [{ item: "cap1", lines: 2, faces: ["f1"], width: 10, height: 20, words: 3, missing: [] }]);
});

test("engineProblem wants a text engine exactly on plans that measured a caption", () => {
  const measured = { layers: [{ caption: { layout: {} } }] };
  const bare = { layers: [{ caption: { layout: null } }, { item: "c1" }] };
  assert.equal(engineProblem({ ...measured, textEngine: "fin-text/0.0.0 rev/3 cosmic-text/0.19" }, "x"), undefined);
  assert.match(engineProblem(measured, "x"), /textEngine undefined/);
  assert.match(engineProblem({ ...measured, textEngine: "fake/1" }, "x"), /not "fin-text/);
  assert.equal(engineProblem(bare, "x"), undefined);
  assert.match(engineProblem({ ...bare, textEngine: "fin-text/0.0.0 rev/3 a" }, "x"), /measured nothing/);
});

test("cubicBezier solves the CSS easeOut curve", () => {
  assert.ok(Math.abs(cubicBezier(0, 0, 0.58, 1, 0) - 0) < 1e-9);
  assert.ok(Math.abs(cubicBezier(0, 0, 0.58, 1, 1) - 1) < 1e-9);
  const mid = cubicBezier(0, 0, 0.58, 1, 0.25);
  assert.ok(mid > 0.35 && mid < 0.5, `${mid}`);
});

test("firstDifference names the first differing path", () => {
  assert.equal(firstDifference({ a: 1 }, { a: 1 }), undefined);
  assert.equal(firstDifference({ a: { b: "x" } }, { a: { b: "y" } }), '$.a.b: got "x", expected "y"');
  assert.equal(firstDifference({ a: 1, c: 2 }, { a: 1 }), "$.c: got 2, expected undefined");
});

test("hashTree hashes the files a filter keeps, by forward-slash relative path", () => {
  const dir = mkdtempSync(join(tmpdir(), "reel-tree-"));
  try {
    writeFileSync(join(dir, "a.txt"), "a");
    writeFileSync(join(dir, "b.txt"), "b");
    const tree = hashTree(dir, (rel) => rel === "a.txt");
    assert.deepEqual(Object.keys(tree), ["a.txt"]);
    assert.equal(tree["a.txt"], "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("plan summaries name layers, transitions, captions and gain shapes", () => {
  const plan = {
    layers: [
      { item: "c1", kind: "clip", track: "t1", opacity: 1, source: { time: "49/5" }, transition: { id: "tr1", role: "out", progress: 0.3 } },
      { item: "cap1", kind: "caption", track: "t4", opacity: 0.5, caption: { layout: { lines: [{}, {}], faces: ["x"], width: 64, height: 128 } } },
      { item: "cap2", kind: "caption", track: "t4", opacity: 1, caption: { layout: null, unmeasured: { reason: "fontMissing" } } },
    ],
  };
  assert.deepEqual(layerSummary(plan), [
    "c1 clip t1 src 49/5 tr1 out 0.3",
    "cap1 caption t4 opacity 0.5 2 lines, 1 faces, 64x128/64 px",
    "cap2 caption t4 unmeasured fontMissing",
  ]);
  const audio = { items: [{ item: "c7", track: "t3", from: "0", to: "30", speed: "1", gain: [{ from: "0", db: [-6, -6], shapes: [{ kind: "fadeIn" }] }, { from: "2", db: [-6, -6] }] }] };
  assert.deepEqual(audioSummary(audio), ["c7 t3 0..30 x1 -6dB fadeIn@0"]);
});

test("the real-font pass skips without a cache, unless FIN_REQUIRE_FONTS=1", () => {
  assert.equal(realCache({}), undefined);
  assert.throws(() => realCache({ FIN_REQUIRE_FONTS: "1" }), /FIN_FONT_CACHE is not set/);
  const missing = join(tmpdir(), "no-such-font-cache-dir");
  assert.equal(existsSync(missing), false);
  assert.throws(() => realCache({ FIN_FONT_CACHE: missing, FIN_REQUIRE_FONTS: "1" }), /does not exist/);
  assert.equal(realCache({ FIN_FONT_CACHE: tmpdir() }), tmpdir());
});

test("diagnostics: content fields come from the generated schema's x-fin-class", () => {
  const schema = {
    $defs: {
      LogCommit: { properties: { outcome: { $ref: "#/$defs/Outcome" }, intent: { allOf: [{ $ref: "#/$defs/X" }, { "x-fin-class": "content" }] } } },
      Plain: { properties: { a: { type: "string" } } },
    },
  };
  assert.deepEqual(contentFields(schema), { LogCommit: new Set(["intent"]) });
  assert.equal(kindDef("log.commit"), "LogCommit");
  assert.equal(kindDef("error.panic"), "ErrorPanic");
  // The real schema classes log.commit's intent as content.
  assert.ok(loadContentFields().LogCommit?.has("intent"));
});

test("diagnostics: contentProblems flags a training stamp and a content field", () => {
  const fields = { LogCommit: new Set(["intent"]) };
  const ok = { id: "1", kind: "log.commit", consent: { purposes: ["diagnostics"] }, payload: { outcome: "accepted" } };
  assert.deepEqual(contentProblems(ok, fields), []);
  const bad = { ...ok, consent: { purposes: ["diagnostics", "training"] }, payload: { intent: [] } };
  assert.deepEqual(contentProblems(bad, fields), ["log.commit 1 is stamped training", "log.commit 1 has content field intent"]);
});

test("diagnostics: leaks finds needles and any unkeyed blake3 hash, skipping empty needles", () => {
  const files = [
    ["a.jsonl", Buffer.from('{"h":"k1:00ff","p":"/tmp/x"}\n')],
    ["b.jsonl", Buffer.from('{"h":"blake3:abcd"}\n')],
  ];
  assert.deepEqual(leaks(files, ["/tmp/x", "", undefined, "absent"]), [
    { file: "a.jsonl", needle: "/tmp/x" },
    { file: "b.jsonl", needle: "blake3:" },
  ]);
  assert.deepEqual(parseJsonl('{"a":1}\n\n{"b":2}\n'), [{ a: 1 }, { b: 2 }]);
});

test("the e2e handshake MACs match the shared vectors (5A Task 6: connectWs proves over the socket)", () => {
  const v = JSON.parse(readFileSync(new URL("../tests/fixtures/auth/handshake.json", import.meta.url), "utf8"));
  for (const h of v.handshakes) {
    const key = Buffer.from(v.keys[h.key], "base64url");
    const [cn, sn] = [Buffer.from(h.clientNonce, "hex"), Buffer.from(h.serverNonce, "hex")];
    assert.equal(handshakeMac(key, v.labels.hello, [h.envId, h.endpoint, cn, sn]).toString("hex"), h.proof, h.name);
    assert.equal(handshakeMac(key, v.labels.prove, [h.envId, h.endpoint, sn, cn]).toString("hex"), h.mac, h.name);
  }
});

// The harness honours CARGO_TARGET_DIR and keeps the server's socket path short.
const LIB = new URL("./e2e/lib.mjs", import.meta.url).href;
const inChild = (code, env) =>
  spawnSync(process.execPath, ["--input-type=module", "-e", `import * as lib from ${JSON.stringify(LIB)}; ${code}`], { encoding: "utf8", env: { ...process.env, ...env } });

test("fin and fin-plugin are looked for in CARGO_TARGET_DIR when it is set", () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const set = inChild("console.log(JSON.stringify([lib.TARGET_DIR, lib.FIN, lib.FIN_PLUGIN]))", { CARGO_TARGET_DIR: "/somewhere/else" });
  const [dir, fin, plugin] = JSON.parse(set.stdout);
  assert.equal(dir, "/somewhere/else");
  assert.ok(fin.startsWith("/somewhere/else/release/fin") && plugin.startsWith("/somewhere/else/release/fin-plugin"));
  const unset = inChild("console.log(lib.TARGET_DIR)", { CARGO_TARGET_DIR: "" });
  assert.equal(unset.stdout.trim(), join(root, "target"));
});

test("scenarioTmp keeps the socket path under 104 bytes whatever TMPDIR is", { skip: process.platform === "win32" }, () => {
  const deep = mkdtempSync(join(tmpdir(), "d".repeat(60)));
  try {
    const r = inChild(
      "const d = lib.scenarioTmp('finisher-e2e-plug-'); console.log(d); console.log(Buffer.byteLength(d + '/server-data/run/fin.sock'));",
      { TMPDIR: deep },
    );
    const [dir, bytes] = r.stdout.trim().split("\n");
    assert.ok(Number(bytes) <= 104, `${dir}: ${bytes} bytes`);
    assert.ok(existsSync(dir));
    rmSync(dir, { recursive: true, force: true });
  } finally {
    rmSync(deep, { recursive: true, force: true });
  }
});

// The auth scenario's helpers (5A Task 12).
test("auth: errorCode finds the first error code in an Exit, an Error or a message", () => {
  assert.equal(errorCode({ _tag: "Failure", cause: [{ error: { code: "UNAUTHENTICATED" } }] }), "UNAUTHENTICATED");
  assert.equal(errorCode(new Error('auth.ticket failed: {"code":"TICKET_LIMIT"}')), "TICKET_LIMIT");
  assert.equal(errorCode({ _tag: "Success", value: {} }), undefined);
});

test("auth: secretHits finds long needles anywhere and short ones only standing alone", () => {
  const long = "k".repeat(43);
  const sources = [
    { name: "a", text: `x ${long} y` },
    { name: "b", text: "code ABC123 here" },
    { name: "c", text: "xABC123x" },
    { name: "d", text: "nothing" },
  ];
  assert.deepEqual(
    secretHits(sources, [long, "ABC123", ""]).map((h) => [h.source, h.needle.length > 0]),
    [["a", true], ["b", true]],
  );
  assert.ok(!JSON.stringify(secretHits(sources, [long])).includes(long), "a hit never prints the secret");
});

test("auth: writePrivate creates an owner-only file", { skip: process.platform === "win32" }, () => {
  const dir = mkdtempSync(join(tmpdir(), "e2e-priv-"));
  try {
    writePrivate(join(dir, "k"), "secret\n");
    assert.equal(statSync(join(dir, "k")).mode & 0o777, 0o600);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("auth: rawConnect answers each call with its Exit and says when the connection ends", { skip: process.platform === "win32" }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "e2e-raw-"));
  const path = join(dir, "s.sock");
  const fake = createServer((sock) => {
    sock.setEncoding("utf8").on("data", (d) => {
      const req = JSON.parse(d.split("\n")[0]);
      if (req.tag === "bye") return sock.end();
      sock.write(`${JSON.stringify({ _tag: "Exit", requestId: req.id, exit: { _tag: "Success", value: { tag: req.tag } } })}\n`);
    });
  });
  await new Promise((r) => fake.listen(path, r));
  try {
    const raw = await rawConnect({ info: { socket: path } });
    assert.deepEqual(await raw.call("one", {}), { _tag: "Success", value: { tag: "one" } });
    assert.deepEqual(await raw.call("bye", {}), { _tag: "Closed" });
    await raw.closed;
  } finally {
    fake.close();
    rmSync(dir, { recursive: true, force: true });
  }
});

test("auth: wsUpgrade sends the headers it is given and reports the status and body", async () => {
  const seen = [];
  const http = createHttpServer((req, res) => {
    seen.push([req.headers.host, req.headers.origin, req.headers["sec-websocket-protocol"], req.url]);
    res.writeHead(403, { "content-type": "text/plain" }).end("no");
  });
  await new Promise((r) => http.listen(0, "127.0.0.1", r));
  try {
    const server = { info: { wsPort: http.address().port } };
    const r = await wsUpgrade(server, { protocols: ["finisher.v2", "finisher.ticket.t"], host: "evil.example:1", origin: "http://evil.example", path: "/rpc?x=1" });
    assert.deepEqual([r.status, r.body], [403, "no"]);
    assert.deepEqual(seen, [["evil.example:1", "http://evil.example", "finisher.v2, finisher.ticket.t", "/rpc?x=1"]]);
  } finally {
    http.close();
  }
});

// The remote scenario's device client (5B Task 13): its proofs are made apart from the server.
test("remote: dpopProof is an ES256 JWS over its header and claims, verifiable with its own JWK", async () => {
  const { dpopProof, newDeviceKey } = await import("./e2e/device-lib.mjs");
  const { webcrypto, createHash } = await import("node:crypto");
  const key = await newDeviceKey();
  const jwt = await dpopProof(key, "POST", "https://127.0.0.1:1/auth/token", { nonce: "n0", accessToken: "tok" });
  const [h, p, s] = jwt.split(".");
  const header = JSON.parse(Buffer.from(h, "base64url").toString());
  const claims = JSON.parse(Buffer.from(p, "base64url").toString());
  assert.deepEqual(header, { typ: "dpop+jwt", alg: "ES256", jwk: key.jwk });
  assert.equal(claims.htm, "POST");
  assert.equal(claims.htu, "https://127.0.0.1:1/auth/token");
  assert.equal(claims.nonce, "n0");
  assert.equal(claims.ath, createHash("sha256").update("tok").digest("base64url"));
  assert.equal(Buffer.from(s, "base64url").length, 64);
  const pub = await webcrypto.subtle.importKey("jwk", key.jwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
  assert.equal(await webcrypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, pub, Buffer.from(s, "base64url"), Buffer.from(`${h}.${p}`)), true);
  // RFC 7638's thumbprint: the required members in order, no spaces.
  const want = createHash("sha256").update(`{"crv":"P-256","kty":"EC","x":"${key.jwk.x}","y":"${key.jwk.y}"}`).digest("base64url");
  assert.equal(key.jkt, want);
});
