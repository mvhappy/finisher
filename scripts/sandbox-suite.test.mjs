import { test } from "node:test";
import assert from "node:assert/strict";
import { readdirSync } from "node:fs";
import { format, osName, parseResult, skipReasons, SUITES, suiteEnv, tally } from "./sandbox-suite.mjs";

const rec = (profile, result, extra = {}) => ({
  os: "linux",
  file: "server_data.json",
  case: `c-${profile}-${result}`,
  profile,
  result,
  detail: "",
  ...extra,
});

test("Linux requires Landlock; namespaces only when asked", () => {
  const linux = suiteEnv("linux", { reportDir: "/r" }, {});
  assert.equal(linux.FIN_REQUIRE_LANDLOCK, "1");
  assert.equal(linux.FIN_ESCAPE_REPORT, "/r");
  assert.equal(linux.FIN_REQUIRE_NAMESPACES, undefined);
  assert.equal(suiteEnv("linux", { requireNamespaces: true }, {}).FIN_REQUIRE_NAMESPACES, "1");
  assert.equal(suiteEnv("macos", {}, {}).FIN_REQUIRE_LANDLOCK, undefined);
});

test("OS names", () => {
  assert.equal(osName("linux"), "linux");
  assert.equal(osName("darwin"), "macos");
  assert.equal(osName("win32"), "windows");
  assert.equal(osName("freebsd"), null);
});

test("every listed suite exists, and every escape suite is listed", () => {
  const dir = new URL("../crates/fin-sandbox/tests/", import.meta.url);
  const files = readdirSync(dir).filter((f) => f.endsWith(".rs")).map((f) => f.slice(0, -3));
  for (const [os, names] of Object.entries(SUITES)) {
    assert.equal(names[0], "escape_common", os);
    for (const n of names) assert.ok(files.includes(n), `${os}: ${n}.rs`);
  }
  for (const f of files.filter((f) => f.startsWith("escape_"))) {
    assert.ok(Object.values(SUITES).some((l) => l.includes(f)), `${f} is in no OS's list`);
  }
});

test("libtest's summary is parsed", () => {
  assert.deepEqual(parseResult("running 4 tests\n....\ntest result: ok. 4 passed; 0 failed; 1 ignored; 0 measured"), {
    ok: true,
    passed: 4,
    failed: 0,
    ignored: 1,
  });
  assert.equal(parseResult("test result: FAILED. 3 passed; 1 failed; 0 ignored").ok, false);
  assert.equal(parseResult("error: could not compile"), null);
});

test("skip reasons are collected once each", () => {
  const out = [
    "links_mounts.json: agent: a nested user namespace: skipped, no unprivileged user namespace",
    "links_mounts.json: agent: a nested user namespace: skipped, no unprivileged user namespace",
    "x: no socket namespace on this host (Unavailable)",
    "test server_data ... ok",
  ].join("\n");
  assert.equal(skipReasons(out).length, 2);
});

test("records are tallied per OS and profile, in profile order", () => {
  const rows = tally([rec("plugin", "denied"), rec("agent", "denied"), rec("agent", "allowed"), rec("agent", "skipped")]);
  assert.deepEqual(
    rows.map((r) => [r.profile, r.denied, r.allowed, r.skipped, r.failed]),
    [
      ["agent", 1, 1, 1, 0],
      ["plugin", 1, 0, 0, 0],
    ],
  );
  assert.throws(() => tally([rec("agent", "maybe")]), /unknown result/);
});

test("the table lists failed cases and failed suites", () => {
  const text = format({
    records: [rec("agent", "denied"), rec("hook", "failed", { detail: "not denied: Ran(None)" })],
    suites: [
      { name: "escape_common", status: 101, result: { ok: false, passed: 2, failed: 1, ignored: 0 } },
      { name: "launcher", status: 0, result: { ok: true, passed: 5, failed: 0, ignored: 0 } },
    ],
    skips: ["namespace_linux: no socket namespace on this host"],
  });
  assert.match(text, /^os\s+profile\s+denied\s+allowed\s+skipped\s+failed$/m);
  assert.match(text, /^linux\s+agent\s+1\s+0\s+0\s+0$/m);
  assert.match(text, /failed cases:\n {2}linux hook server_data\.json: c-hook-failed: not denied/);
  assert.match(text, /^escape_common\s+2\s+1\s+0\s+FAILED$/m);
  assert.match(text, /^launcher\s+5\s+0\s+0\s+ok$/m);
  assert.match(text, /skipped \(reasons\):\n {2}namespace_linux/);
});
