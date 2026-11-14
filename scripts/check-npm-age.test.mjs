import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { findViolations } from "./check-crate-age.mjs";
import { fetchPublishTimes, main, parseLock, resolveDates } from "./check-npm-age.mjs";
import { runAll } from "./check-dep-age.mjs";

const NOW = new Date("2026-11-07T12:00:00Z");
const DAY = 86_400_000;
const ago = (ms) => new Date(NOW.getTime() - ms).toISOString();

const LOCK = `lockfileVersion: '9.0'

importers:

  .:
    dependencies:
      openai:
        specifier: 7.47.0
        version: 7.47.0(ws@8.22.0)

packages:

  '@pnpm/exe.linux-x64@12.15.0':
    resolution: {integrity: sha512-aaa}
    cpu: [x64]

  openai@7.47.0:
    resolution: {integrity: sha512-bbb}
    peerDependencies:
      ws: ^8

  tarball-pkg@1.0.0:
    resolution: {tarball: https://example.com/tarball-pkg-1.0.0.tgz}

  gitpkg@2.0.0:
    resolution: {commit: 0123abc, repo: https://example.com/gitpkg.git, type: git}

  local@3.0.0:
    resolution: {directory: ../local, type: directory}

  openai@7.46.0:
    resolution: {integrity: sha512-ccc}

snapshots:

  openai@7.47.0(ws@8.22.0):
    optionalDependencies:
      ws: 8.22.0

  ws@8.22.0: {}
`;

test("parseLock reads registry packages from the packages section only", () => {
  assert.deepEqual(parseLock(LOCK), [
    { name: "@pnpm/exe.linux-x64", version: "12.15.0" },
    { name: "openai", version: "7.46.0" },
    { name: "openai", version: "7.47.0" },
  ]);
});

test("parseLock handles an empty or package-less lock", () => {
  assert.deepEqual(parseLock(""), []);
  assert.deepEqual(parseLock("lockfileVersion: '9.0'\n\nimporters:\n  .: {}\n"), []);
});

test("findViolations flags a young or undated npm package (shared policy)", () => {
  const pkgs = [
    { name: "old", version: "1.0.0" },
    { name: "young", version: "2.0.0" },
    { name: "edge", version: "3.0.0" },
    { name: "undated", version: "4.0.0" },
  ];
  const dates = { "old@1.0.0": ago(30 * DAY), "young@2.0.0": ago(2 * DAY), "edge@3.0.0": ago(7 * DAY) };
  const out = findViolations(pkgs, dates, NOW, 7);
  assert.equal(out.length, 2);
  assert.match(out[0], /^young 2\.0\.0: published .* \(2 days ago, need >= 7\)$/);
  assert.match(out[1], /^undated 4\.0\.0: no publication date$/);
  assert.equal(findViolations(pkgs, dates, NOW, 1).length, 1); // a lower threshold passes `young`
});

test("resolveDates asks once per package name and only for uncached versions", async () => {
  const pkgs = [
    { name: "a", version: "1.0.0" },
    { name: "a", version: "1.1.0" },
    { name: "b", version: "2.0.0" },
    { name: "c", version: "3.0.0" },
  ];
  const calls = [];
  const sleeps = [];
  const seen = [];
  const dates = await resolveDates(
    pkgs,
    { "c@3.0.0": "cached" },
    async (name) => {
      calls.push(name);
      return { created: "x", "1.0.0": "t1", "1.1.0": "t11", "2.0.0": "t2" };
    },
    { delayMs: 50, sleep: async (ms) => sleeps.push(ms), onFetch: (k) => seen.push(k) },
  );
  assert.deepEqual(calls, ["a", "b"]);
  assert.deepEqual(sleeps, [50]); // between requests, not before the first
  assert.deepEqual(seen, ["a@1.0.0", "a@1.1.0", "b@2.0.0"]);
  assert.deepEqual(dates, { "c@3.0.0": "cached", "a@1.0.0": "t1", "a@1.1.0": "t11", "b@2.0.0": "t2" });
});

test("resolveDates leaves a version absent from the registry undated", async () => {
  const dates = await resolveDates([{ name: "a", version: "9.9.9" }], {}, async () => ({ "1.0.0": "t" }), { delayMs: 0 });
  assert.deepEqual(dates, {});
});

test("fetchPublishTimes encodes scoped names, sends a descriptive User-Agent without an email", async () => {
  let seenUrl;
  let seenHeaders;
  const fetchFn = async (url, init) => {
    seenUrl = url;
    seenHeaders = init.headers;
    return { ok: true, status: 200, json: async () => ({ time: { "1.0.0": "2026-01-01T00:00:00.000Z" } }) };
  };
  const time = await fetchPublishTimes("@scope/pkg", { registry: "https://reg.example/", fetchFn });
  assert.equal(seenUrl, "https://reg.example/@scope%2fpkg");
  assert.match(seenHeaders["User-Agent"], /^finisher-dep-age-check/);
  assert.doesNotMatch(seenHeaders["User-Agent"], /@|mailto/);
  assert.equal(time["1.0.0"], "2026-01-01T00:00:00.000Z");
});

test("fetchPublishTimes retries a 429 honouring retry-after, then gives up on errors", async () => {
  const answers = [
    { ok: false, status: 429, headers: { get: () => "3" } },
    { ok: false, status: 429, headers: { get: () => null } },
    { ok: true, status: 200, json: async () => ({ time: { "1.0.0": "t" } }) },
  ];
  const sleeps = [];
  const time = await fetchPublishTimes("p", { fetchFn: async () => answers.shift(), sleep: async (ms) => sleeps.push(ms) });
  assert.deepEqual(sleeps, [3000, 2000]);
  assert.equal(time["1.0.0"], "t");
  await assert.rejects(fetchPublishTimes("p", { fetchFn: async () => ({ ok: false, status: 404 }) }), /HTTP 404/);
  await assert.rejects(fetchPublishTimes("p", { fetchFn: async () => ({ ok: true, status: 200, json: async () => ({}) }) }), /no `time` map/);
});

test("main fails on a young locked package and caches what it learned", async () => {
  const root = mkdtempSync(join(tmpdir(), "npm-age-"));
  const realFetch = globalThis.fetch;
  const realLog = console.log;
  const realErr = console.error;
  const lines = [];
  try {
    writeFileSync(join(root, "pnpm-lock.yaml"), LOCK);
    const young = new Date(Date.now() - 2 * DAY).toISOString();
    const old = new Date(Date.now() - 60 * DAY).toISOString();
    globalThis.fetch = async (url) => ({
      ok: true,
      status: 200,
      json: async () => ({ time: url.endsWith("/openai") ? { "7.46.0": old, "7.47.0": young } : { "12.15.0": old } }),
    });
    console.log = (l) => lines.push(l);
    console.error = () => {};
    process.exitCode = 0;
    await main(["--root", root, "--delay-ms", "0"], {});
    assert.equal(process.exitCode, 1);
    assert.equal(lines.filter((l) => l.startsWith("openai 7.47.0: published")).length, 1);
    assert.match(lines.at(-1), /1 package\(s\) younger than 7 days/);
    const cache = JSON.parse(readFileSync(join(root, "target", "npm-age-cache.json"), "utf8"));
    assert.equal(cache["openai@7.47.0"], young);

    // Second run: fully cached (no network), and a 1-day threshold passes.
    globalThis.fetch = async () => assert.fail("cache miss");
    lines.length = 0;
    await main(["--root", root, "--days", "1"], {});
    assert.equal(process.exitCode, 0);
    assert.deepEqual(lines, []);
  } finally {
    globalThis.fetch = realFetch;
    console.log = realLog;
    console.error = realErr;
    process.exitCode = 0;
    rmSync(root, { recursive: true, force: true });
  }
});

test("main reports a missing lockfile", async () => {
  const root = mkdtempSync(join(tmpdir(), "npm-age-"));
  try {
    mkdirSync(join(root, "target"));
    await assert.rejects(main(["--root", root], {}), /no pnpm-lock\.yaml/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("runAll runs every check and returns the worst exit code", async () => {
  const order = [];
  const code = (n) => async () => {
    order.push(n);
    process.exitCode = n;
  };
  const saved = process.exitCode;
  try {
    assert.equal(await runAll([["a", code(1)], ["b", code(0)]], [], {}), 1);
    assert.equal(await runAll([["a", code(0)], ["b", code(0)]], [], {}), 0);
    const boom = async () => {
      throw new Error("offline");
    };
    const realErr = console.error;
    console.error = () => {};
    try {
      assert.equal(await runAll([["a", code(1)], ["b", boom], ["c", code(0)]], [], {}), 2);
    } finally {
      console.error = realErr;
    }
    assert.deepEqual(order, [1, 0, 0, 0, 1, 0]);
  } finally {
    process.exitCode = saved;
  }
});
