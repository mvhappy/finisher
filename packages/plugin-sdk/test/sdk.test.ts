// The SDK against a fake host over real stdio (plan 8B Task 13): a bundled test plugin and the
// bundled `ts-tools` example run as `node` processes, the permission one under the flags the
// `node` runtime uses (`apps/finisherd/src/plugins/host/node-runtime.ts`).
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { build as buildExample } from "../../../examples/plugins/ts-tools/build.mjs";
import { CANCEL_GRACE_MS, ENVELOPE_VERSION, SDK_MINOR } from "../src/index.ts";
import { bundle } from "../tools/bundle.mjs";
import { commandJob, context, FakeHost } from "./fake-host.ts";

let tmp: string;
let pkg: string;
let probe: string;
let example: string;
const hosts: FakeHost[] = [];

beforeAll(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-plugin-sdk-"));
  pkg = join(tmp, "pkg");
  probe = join(pkg, "dist", "probe.mjs");
  bundle({ entry: join(import.meta.dirname, "fixtures", "probe-plugin.ts"), outfile: probe });
  example = buildExample();
});

afterEach(() => {
  for (const h of hosts.splice(0)) h.kill();
});

afterAll(() => rmSync(tmp, { recursive: true, force: true }));

function host(main: string, ...extra: string[]): FakeHost {
  const h = new FakeHost([main, ...extra]);
  hosts.push(h);
  return h;
}

/** The `node` runtime's flags (`nodeArgs`): read the package, read and write its temp. */
function confined(main: string, dir: string, warm: boolean): FakeHost {
  const t = join(tmp, "key-tmp");
  mkdirSync(t, { recursive: true });
  const flags = ["--permission", `--allow-fs-read=${dir}`, `--allow-fs-read=${t}`, `--allow-fs-write=${t}`, main];
  return host(flags[0]!, ...flags.slice(1), ...(warm ? ["--warm"] : []));
}

describe("the envelope", () => {
  test("hello is v 1 with the SDK's minor, the command kind and no envelope hash", async () => {
    const cold = host(probe);
    const warm = host(probe, "--warm");
    for (const [h, isWarm] of [[cold, false], [warm, true]] as const) {
      const hello = await h.next();
      expect(hello).toEqual({ v: ENVELOPE_VERSION, t: "hello", seq: 1, minor: SDK_MINOR, role: "worker", family: "plugin", kinds: [{ kind: "plugin.command", version: 1 }], warm: isWarm, pid: h.child.pid });
      expect(hello).not.toHaveProperty("envelope");
    }
  });

  test("a cold process serves one job and exits without ready", async () => {
    const h = host(probe);
    const job = commandJob("echo", { a: 1 });
    const out = await h.run(job);
    expect(out).toMatchObject({ t: "result", job: job.id, value: { kind: "data", value: { args: { a: 1 }, run: { command: "echo", rev: 7, project: "p1", branch: "main" }, context: context() } } });
    expect(await h.exited).toBe(0);
    expect(h.got.map((m) => m.t)).toEqual(["hello", "result"]);
    expect(h.problems).toEqual([]);
  });

  test("a trailing --warm sends ready after each job and serves the next", async () => {
    const h = host(probe, "--warm");
    for (const n of [1, 2]) {
      const job = commandJob("echo", { n });
      const from = h.got.length;
      await h.run(job);
      const ready = await h.next((m) => m.t === "ready", from);
      expect(h.got.indexOf(ready)).toBe(h.got.indexOf(await h.outcome(job.id)) + 1);
    }
    expect(h.child.exitCode).toBeNull();
    h.child.stdin.end();
    expect(await h.exited).toBe(0);
    expect(h.problems).toEqual([]);
  });

  test("a tx result is a CommandOut of kind tx", async () => {
    const h = host(probe);
    const out = await h.run(commandJob("ops"));
    expect(out).toMatchObject({ t: "result", value: { kind: "tx", ops: [{ op: "marker.remove", marker: "mk1" }] } });
  });

  test("alive goes out at the start's aliveMs", async () => {
    const h = host(probe, "--warm");
    const job = commandJob("wait-cancel");
    h.start(job, 40);
    await h.next((m) => m.t === "alive");
    const first = h.got.findIndex((m) => m.t === "alive");
    await h.next((m) => m.t === "alive", first + 1);
    h.send({ t: "cancel", job: job.id, reason: "user" });
    await h.outcome(job.id);
  });

  test("an unknown host message type is not fatal, call-shaped or not", async () => {
    const h = host(probe, "--warm");
    await h.next();
    h.send({ t: "future.thing", x: 1 });
    h.send({ t: "future.call", id: 5, method: "plugin.ping", params: {} });
    h.send({ t: "attach", helper: "h" });
    const out = await h.run(commandJob("echo"));
    expect(out.t).toBe("result");
    expect(h.got.filter((m) => m.t !== "hello" && m.t !== "result" && m.t !== "ready" && m.t !== "alive")).toEqual([]);
  });

  test("a host line with a wrong seq is a protocol error: exit 65", async () => {
    const h = host(probe, "--warm");
    await h.next();
    h.send({ v: 1, t: "shutdown", seq: 9 }, true);
    expect(await h.exited).toBe(65);
    expect(h.stderr.join("\n")).toContain("seq");
  });

  test("shutdown and stdin EOF end the process", async () => {
    const a = host(probe, "--warm");
    await a.next();
    a.send({ t: "shutdown" });
    expect(await a.exited).toBe(0);
    const b = host(probe, "--warm");
    await b.next();
    b.child.stdin.end();
    expect(await b.exited).toBe(0);
  });
});

describe("cancel", () => {
  test("cancel aborts the signal and the job ends cancelled; warm, it stays", async () => {
    const h = host(probe, "--warm");
    const job = commandJob("wait-cancel");
    h.start(job);
    await h.next();
    h.send({ t: "cancel", job: job.id, reason: "user" });
    expect(await h.outcome(job.id)).toMatchObject({ t: "error", error: { code: "JOB_CANCELLED" } });
    await h.next((m) => m.t === "ready");
    expect((await h.run(commandJob("echo"))).t).toBe("result");
  });

  test("cancel fails the job's pending calls", async () => {
    const h = host(probe, "--warm");
    h.calls["project.view"] = () => undefined;
    const job = commandJob("wait-call");
    h.start(job);
    await h.next((m) => m.t === "call");
    h.send({ t: "cancel", job: job.id, reason: "user" });
    expect(await h.outcome(job.id)).toMatchObject({ t: "error", error: { code: "JOB_CANCELLED" } });
  });

  test("a handler that ignores its signal is abandoned with its process", async () => {
    const h = host(probe, "--warm");
    const job = commandJob("ignore-cancel");
    h.start(job);
    await h.next();
    const at = Date.now();
    h.send({ t: "cancel", job: job.id, reason: "user" });
    expect(await h.outcome(job.id)).toMatchObject({ t: "error", error: { code: "JOB_CANCELLED" } });
    expect(Date.now() - at).toBeGreaterThanOrEqual(CANCEL_GRACE_MS - 100);
    expect(await h.exited).toBe(0);
    expect(h.got.some((m) => m.t === "ready")).toBe(false);
  });
});

describe("host calls", () => {
  const project = { id: "p1", name: "Paged", rate: "30", schemaVersion: 1, assets: {}, timelines: [], note: "é".repeat(4000) };

  /** `project`'s JSON in `pages` pages of `rev` (one page's rev can be overridden). */
  function pagesOf(pages: number, rev: number, badPage?: number): (p: { page?: number }) => unknown {
    const bytes = Buffer.from(JSON.stringify(project));
    const size = Math.ceil(bytes.length / pages);
    return ({ page = 0 }) => ({ bytesB64: bytes.subarray(page * size, (page + 1) * size).toString("base64"), page, pages, rev: page === badPage ? rev + 1 : rev });
  }

  test("snapshot joins its pages at the job's rev", async () => {
    const h = host(probe);
    h.calls["project.snapshot"] = pagesOf(3, 7);
    const out = await h.run(commandJob("snapshot", {}, 7));
    expect(out).toMatchObject({ t: "result", value: { kind: "data", value: project } });
    expect(h.callsOf("project.snapshot").map((c) => c.params)).toEqual([{}, { page: 1 }, { page: 2 }]);
  });

  test("a snapshot page of another rev is PLUGIN_PROTOCOL", async () => {
    const h = host(probe);
    h.calls["project.snapshot"] = pagesOf(3, 7, 2);
    const out = await h.run(commandJob("snapshot", {}, 7));
    expect(out).toMatchObject({ t: "error", error: { code: "PLUGIN_PROTOCOL", details: { reason: "snapshotRev", expected: 7, got: 8 } } });
  });

  test("fetch reads the body in pages until eof", async () => {
    const h = host(probe);
    const body = Buffer.from("<title>Hi</title>".padEnd(2500, "."));
    h.calls["net.fetch"] = () => ({ status: 200, headers: [{ name: "content-type", value: "text/html" }], handle: "b1", length: body.length });
    h.calls["net.read"] = ({ offset, length }: { offset: number; length: number }) => ({ bytesB64: body.subarray(offset, offset + length).toString("base64"), eof: offset + length >= body.length });
    const job = commandJob("fetch", { url: "https://api.example.org/x" }, 7, context({ pageBytes: 1000 }));
    const out = await h.run(job);
    expect(out).toMatchObject({ t: "result", value: { kind: "data", value: { status: 200, redirect: false, text: body.toString() } } });
    expect(h.callsOf("net.fetch")[0]?.params).toEqual({ request: { method: "GET", url: "https://api.example.org/x" } });
    expect(h.callsOf("net.read").map((c) => c.params)).toEqual([0, 1000, 2000].map((offset) => ({ handle: "b1", offset, length: offset === 2000 ? 500 : 1000 })));
  });

  test("a body shorter than its length is PLUGIN_PROTOCOL", async () => {
    const h = host(probe);
    h.calls["net.fetch"] = () => ({ status: 200, headers: [], handle: "b1", length: 10 });
    h.calls["net.read"] = () => ({ bytesB64: Buffer.from("abc").toString("base64"), eof: true });
    const out = await h.run(commandJob("fetch", { url: "https://api.example.org/x", grant: "g" }));
    expect(out).toMatchObject({ t: "error", error: { code: "PLUGIN_PROTOCOL", details: { reason: "bodyLength", length: 10, got: 3 } } });
    expect(h.callsOf("net.fetch")[0]?.params).toMatchObject({ grant: "g" });
  });

  test("a call's error reply reaches the handler as a PluginError", async () => {
    const h = host(probe);
    h.calls["project.view"] = () => ({ error: { code: "PLUGIN_PERMISSION_DENIED", message: "no", details: { perm: "project:read" } } });
    const out = await h.run(commandJob("view", { budget: 100 }));
    expect(out).toMatchObject({ t: "error", error: { code: "PLUGIN_PERMISSION_DENIED", details: { perm: "project:read" } } });
    expect(h.callsOf("project.view")[0]?.params).toEqual({ budget: 100 });
  });

  test("log is a call whose reply nobody waits for", async () => {
    const h = host(probe);
    h.calls["log"] = () => undefined;
    const out = await h.run(commandJob("log", { msg: "hello" }));
    expect(out).toMatchObject({ t: "result", value: { kind: "data", value: null } });
    expect(h.callsOf("log")[0]?.params).toEqual({ level: "warn", msg: "hello" });
  });
});

describe("errors", () => {
  const cases: Array<[string, Record<string, unknown>]> = [
    ["fail-plugin", { code: "PROBE_FAILED", message: "the probe failed", details: { at: 1, fix: "try again" } }],
    ["fail-like", { code: "PROBE_LIKE", message: "like a plugin error", details: { b: 2 } }],
    ["fail-generic", { code: "PLUGIN_COMMAND_FAILED", message: "boom", details: { name: "TypeError" } }],
    ["fail-enoent", { code: "PLUGIN_COMMAND_FAILED", details: { name: "Error", nodeCode: "ENOENT" } }],
    ["no-such-command", { code: "PLUGIN_PROTOCOL", details: { reason: "unknownCommand", command: "no-such-command" } }],
    ["bad-result", { code: "PLUGIN_PROTOCOL", details: { reason: "badResult" } }],
    ["big", { code: "PLUGIN_PROTOCOL", details: { reason: "tooLarge" } }],
  ];

  test("a handler's failure becomes the job's error, and a warm process keeps serving", async () => {
    const h = host(probe, "--warm");
    for (const [command, error] of cases) {
      expect([command, await h.run(commandJob(command))]).toMatchObject([command, { t: "error", error }]);
    }
    const kind = { ...commandJob("echo"), kind: "plugin.hook" };
    expect(await h.run(kind)).toMatchObject({ t: "error", error: { code: "PLUGIN_PROTOCOL", details: { reason: "unknownKind", kind: "plugin.hook" } } });
    expect(await h.run(commandJob("echo", {}, 7, null))).toMatchObject({ t: "error", error: { code: "PLUGIN_PROTOCOL", details: { reason: "noContext" } } });
    expect((await h.run(commandJob("echo"))).t).toBe("result");
    expect(h.problems).toEqual([]);
  });

  test("node_permission_denials_surface_as_errors", async () => {
    const outside = mkdtempSync(join(tmpdir(), "fin-plugin-outside-"));
    try {
      writeFileSync(join(outside, "secret.txt"), "no");
      const h = confined(probe, pkg, true);
      const out = await h.run(commandJob("read-outside", { path: join(outside, "secret.txt") }));
      expect(out).toMatchObject({ t: "error", error: { code: "PLUGIN_PERMISSION_DENIED", details: { perm: "fs", nodePermission: "FileSystemRead" } } });
      expect(JSON.stringify(out)).not.toContain(outside);
      await h.next((m) => m.t === "ready");
      expect((await h.run(commandJob("echo"))).t).toBe("result");
      expect(h.child.exitCode).toBeNull();
    } finally {
      rmSync(outside, { recursive: true, force: true });
    }
  });
});

describe("the ts-tools example", () => {
  const dir = join(import.meta.dirname, "..", "..", "..", "examples", "plugins", "ts-tools");

  test("bundled example has no import outside node: and itself", () => {
    const text = readFileSync(example, "utf8");
    const specs = [...text.matchAll(/(?:^|[\s;])(?:import|export)\b[^;"'`]*?\bfrom\s*["']([^"']+)["']|\bimport\s*\(\s*["']([^"']+)["']|^\s*import\s*["']([^"']+)["']/gm)].map((m) => m[1] ?? m[2] ?? m[3]);
    expect(specs.length).toBeGreaterThan(0);
    expect(specs.filter((s) => !s?.startsWith("node:"))).toEqual([]);
    expect(text).not.toMatch(/\brequire\s*\(/);
    expect(text).not.toMatch(/^\s*import\s*\{|^\s*export\s/m);
  });

  test("its manifest passes 8A validate", async () => {
    const { validateManifest } = await import("@finisher/fin-node");
    const report = JSON.parse(await validateManifest(dir, "dev")) as { ok: boolean; errors?: unknown[]; manifest?: { contributes?: { commands?: Array<{ name: string; writes?: boolean }> } } };
    expect(report.errors ?? []).toEqual([]);
    expect(report.ok).toBe(true);
    expect(report.manifest?.contributes?.commands?.map((c) => [c.name, c.writes])).toEqual([["trim-silence", true], ["fetch-title", false]]);
  });

  test("trim-silence and fetch-title run under the node runtime's permission flags", async () => {
    const h = confined(example, dir, true);
    const project = {
      id: "p1",
      name: "Talk",
      rate: "30",
      schemaVersion: 1,
      assets: {},
      timelines: [
        {
          id: "tl1",
          name: "Main",
          rate: "30",
          resolution: { width: 1920, height: 1080 },
          tracks: [
            { id: "v1", kind: "video", name: "V", items: [{ id: "it1", start: "0", duration: "1", audio: { gain: -90 } }] },
            { id: "a1", kind: "audio", name: "A", items: [{ id: "it2", start: "0", duration: "1", audio: { gain: -70 } }, { id: "it3", start: "1", duration: "1", audio: { gain: -6 } }, { id: "it4", start: "2", duration: "1" }] },
            { id: "a2", kind: "audio", name: "B", mute: true, items: [{ id: "it5", start: "0", duration: "1" }] },
          ],
        },
      ],
    };
    h.calls["project.snapshot"] = () => ({ bytesB64: Buffer.from(JSON.stringify(project)).toString("base64"), page: 0, pages: 1, rev: 7 });
    h.calls["log"] = () => ({});
    expect(await h.run(commandJob("trim-silence", { thresholdDb: -60 }))).toMatchObject({
      t: "result",
      value: { kind: "tx", ops: [{ op: "item.delete", item: "it2", mode: "ripple" }, { op: "item.delete", item: "it5", mode: "ripple" }] },
    });
    const page = Buffer.from("<html><head><title>\n  Example   Domain </title></head></html>");
    h.calls["net.fetch"] = () => ({ status: 200, headers: [], handle: "h1", length: page.length });
    h.calls["net.read"] = () => ({ bytesB64: page.toString("base64"), eof: true });
    expect(await h.run(commandJob("fetch-title", { url: "https://api.example.org/" }))).toMatchObject({ t: "result", value: { kind: "data", value: { status: 200, title: "Example Domain" } } });
    expect(h.callsOf("net.fetch")[0]?.params).toEqual({ request: { method: "GET", url: "https://api.example.org/", headers: [{ name: "accept", value: "text/html" }] } });
    expect(h.problems).toEqual([]);
  });
});
