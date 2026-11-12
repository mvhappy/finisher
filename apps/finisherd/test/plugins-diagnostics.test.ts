// The plugin runtime's diagnostics (sp8b plan Task 15; spec §11) over a real fin-node `Journal`:
// `plugin.stats` per plugin and minute, `plugin.command` with its training arguments, and
// nothing built while the journal is off. `worker.exit` and `plugin.exit` are in
// `jobs/journal-sink.test.ts`.
import { mkdtempSync, rmSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import type { Envelope, ExportPage } from "@finisher/core-types";
import { Journal } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, expect, test } from "vitest";
import type { CallCtx } from "../src/jobs/kinds.ts";
import { makeHostCalls } from "../src/plugins/host/calls.ts";
import { makeEgress } from "../src/plugins/host/egress.ts";
import { commandOutcome, commandRunOf, journalPluginCommand, type PluginCommandRun } from "../src/plugins/host/command-journal.ts";
import type { EgressCounters } from "../src/plugins/host/egress.ts";
import { makePluginLog } from "../src/plugins/host/log.ts";
import { log2Bucket, makePluginStats, methodFamily, STATS_SAMPLES } from "../src/plugins/host/stats.ts";

let journal: Journal | undefined;
let dir: string | undefined;
afterEach(async () => {
  await journal?.close();
  journal = undefined;
  if (dir !== undefined) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function open(privacy: object): Journal {
  dir = mkdtempSync(join(tmpdir(), "fin-plugin-diag-"));
  journal = Journal.open(dir, JSON.stringify(privacy), hostname());
  return journal;
}

async function entries(j: Journal, kinds: string[]): Promise<Envelope[]> {
  const page = JSON.parse(await j.export(JSON.stringify({ kinds }), null, 1000)) as ExportPage;
  return page.lines;
}

const ID = "@acme/fancy-captions";
const counters = (over: Partial<EgressCounters> = {}): EgressCounters => ({
  requests: 0, completed: 0, denied: 0, notHttps: 0, privateAddress: 0, secretMissing: 0, rateLimited: 0,
  tooLarge: 0, timeouts: 0, failed: 0, redirectsFollowed: 0, redirectsReturned: 0, bytesOut: 0, bytesIn: 0, ...over,
});

/** Every string (keys included) in `v`. */
function strings(v: unknown, out: string[] = []): string[] {
  if (typeof v === "string") out.push(v);
  else if (Array.isArray(v)) for (const x of v) strings(x, out);
  else if (v !== null && typeof v === "object") for (const [k, x] of Object.entries(v)) out.push(k, ...strings(x));
  return out;
}

test("method families and buckets", () => {
  expect(["project.snapshot", "project.view", "net.fetch", "net.read", "log", "command", "media.pcm", "x"].map(methodFamily)).toEqual([
    "project", "project", "net", "net", "log", "command", "other", "other",
  ]);
  expect([0, 1, 2, 3, 1024, 2047].map(log2Bucket)).toEqual([0, 1, 2, 2, 11, 11]);
});

test("stats_hold_no_free_text", async () => {
  const j = open({ diagnostics: "on" });
  let egress = new Map([[ID, counters({ requests: 2, completed: 1, denied: 1, bytesIn: 900 })]]);
  const logs: string[] = [];
  const stats = makePluginStats({ journal: j, egress: () => egress, log: (m) => logs.push(m) });
  // The first window holds what the broker counted before it.
  expect(stats.take().map(([id, p]) => [id, p.egress?.requests, p.families])).toEqual([[ID, 2, []]]);
  egress = new Map([[ID, counters({ requests: 5, completed: 3, denied: 1, privateAddress: 1, bytesIn: 5000, bytesOut: 10 })]]);
  const plugin = `${ID}@1.2.0`;
  stats.call(plugin, "project.snapshot", 800, undefined, 20, 700_000);
  stats.call(plugin, "project.view", 3000, "BRANCH_NOT_ALLOWED", 30, 0);
  stats.call(plugin, "net.fetch", 50_000, "EGRESS_DENIED", 100, 0);
  stats.call(plugin, "media.pcm", 10, "INVALID_REQUEST", 5, 0);
  stats.jobEnded(plugin, "PLUGIN_TIMEOUT");
  stats.jobEnded(plugin, "PLUGIN_CRASHED");
  stats.jobEnded(plugin, "CONFLICT");
  stats.dropped(plugin, "log");
  stats.dropped(plugin, "stderr");
  expect(stats.flush()).toBe(1);
  const [e, ...rest] = await entries(j, ["plugin.stats"]);
  expect(rest).toEqual([]);
  expect(e).toMatchObject({ kind: "plugin.stats", kv: 1, source: "finisherd" });
  const p = e?.payload as Record<string, unknown>;
  expect(p).toMatchObject({ windowS: 60, timeouts: 1, crashes: 1, logDropped: 1, stderrDropped: 1 });
  expect(p["plugin"]).toEqual({ hashed: expect.stringMatching(/^k1:[0-9a-f]{32}$/) });
  expect(p["families"]).toEqual([
    { family: "project", calls: 2, errors: { BRANCH_NOT_ALLOWED: 1 }, reachDenials: 1, p50Micros: 3000, p95Micros: 3000, bytesInLog2: log2Bucket(50), bytesOutLog2: log2Bucket(700_000) },
    { family: "net", calls: 1, errors: { EGRESS_DENIED: 1 }, reachDenials: 0, p50Micros: 50_000, p95Micros: 50_000, bytesInLog2: log2Bucket(100), bytesOutLog2: 0 },
    { family: "other", calls: 1, errors: { INVALID_REQUEST: 1 }, reachDenials: 0, p50Micros: 10, p95Micros: 10, bytesInLog2: log2Bucket(5), bytesOutLog2: 0 },
  ]);
  // The window's egress only: the difference from the last window.
  expect(p["egress"]).toEqual({
    requests: 3, completed: 2, denied: 0, notHttps: 0, privateAddress: 1, secretMissing: 0, rateLimited: 0, tooLarge: 0, timeouts: 0, failed: 0,
    redirectsFollowed: 0, redirectsReturned: 0, bytesInLog2: log2Bucket(4100), bytesOutLog2: log2Bucket(10),
  });
  // Nothing free: words of the schema, codes and the keyed hash; no id, version or method.
  const text = JSON.stringify(e);
  for (const leak of ["acme", "fancy", "1.2.0", "snapshot", "fetch", "pcm"]) expect(text).not.toContain(leak);
  for (const s of strings(p)) expect(s).toMatch(/^([a-z][A-Za-z0-9]*|[A-Z][A-Z0-9_]*|k1:[0-9a-f]{32})$/);
  // An idle window records nothing.
  expect(stats.flush()).toBe(0);
  expect(logs).toEqual([]);
});

test("percentiles come from a bounded sample", () => {
  const j = { enabled: true, recordPlugin: () => {} };
  const stats = makePluginStats({ journal: j, random: () => 0.999 });
  for (let i = 1; i <= 1000; i++) stats.call(ID, "log", i, undefined, 0, 0);
  const [[, p] = ["", undefined]] = stats.take();
  const f = p?.families?.[0];
  expect(f?.calls).toBe(1000);
  // The reservoir keeps its first `STATS_SAMPLES` when every later draw falls outside it.
  expect(f?.p50Micros).toBe(Math.floor(STATS_SAMPLES / 2) + 1);
  expect(f?.p95Micros).toBe(Math.floor(STATS_SAMPLES * 0.95) + 1);
});

test("events_off_means_nothing_built", async () => {
  const j = open({ diagnostics: "off" });
  const recorded: string[] = [];
  const spy = { get enabled() { return j.enabled; }, recordPlugin: (...a: Parameters<Journal["recordPlugin"]>) => { recorded.push(a[0]); j.recordPlugin(...a); } };
  let egress = new Map([[ID, counters({ requests: 1 })]]);
  const stats = makePluginStats({ journal: spy, egress: () => egress });
  expect(stats.counting()).toBe(false);
  stats.call(ID, "project.snapshot", 1, undefined, 1, 1);
  stats.jobEnded(ID, "PLUGIN_CRASHED");
  stats.dropped(ID, "log");
  egress = new Map([[ID, counters({ requests: 4 })]]);
  expect(stats.take()).toEqual([]);
  expect(stats.flush()).toBe(0);
  const run: PluginCommandRun = { plugin: `${ID}@1.0.0`, command: "tighten", outcome: "accepted", durationMs: 3, args: { gapMs: 40 } };
  expect(journalPluginCommand(spy, run)).toBe(false);
  expect(recorded).toEqual([]);
  // fin-node itself builds nothing either.
  j.recordPlugin("plugin.command", 1, ID, JSON.stringify({ command: "tighten", outcome: "accepted", micros: 1 }));
  j.recordPlugin("worker.exit", 2, ID, JSON.stringify({ role: "worker", family: "plugin", reason: "done", jobsServed: 1, lifetimeS: 1, sandboxed: false }));
  expect(await entries(j, ["plugin.stats", "plugin.command", "plugin.exit", "worker.exit"])).toEqual([]);
  // Turned on, the next window counts only its own requests.
  j.applySettings(JSON.stringify({ diagnostics: "on" }));
  egress = new Map([[ID, counters({ requests: 6 })]]);
  expect(stats.take().map(([id, p]) => [id, p.egress?.requests])).toEqual([[ID, 2]]);
});

test("plugin_command_training_args_are_content", async () => {
  const j = open({ diagnostics: "on", training: "on" });
  const run: PluginCommandRun = {
    plugin: `${ID}@1.0.0`, command: "tighten-gaps", outcome: "rebased", txId: "01JC0000000000000000000002", ops: { "item.trim": 2 }, durationMs: 31.5, args: { gapMs: 40, note: "keep the intro" },
  };
  expect(journalPluginCommand(j, run)).toBe(true);
  // Written before training goes (the writer drops content of a purpose gone since).
  const [a] = await entries(j, ["plugin.command"]);
  j.applySettings(JSON.stringify({ diagnostics: "on", training: "off" }));
  expect(journalPluginCommand(j, { ...run, outcome: "conflict", code: "CONFLICT" })).toBe(true);
  const [, b] = await entries(j, ["plugin.command"]);
  expect(a).toMatchObject({ kind: "plugin.command", kv: 1, source: "finisherd" });
  expect(a?.payload).toMatchObject({ outcome: "rebased", ops: { "item.trim": 2 }, txId: "01JC0000000000000000000002", micros: 31_500, args: { inline: { gapMs: 40, note: "keep the intro" } } });
  expect(a?.payload).toMatchObject({ plugin: { hashed: expect.stringMatching(/^k1:/) }, command: expect.stringMatching(/^k1:/) });
  // Without training the arguments are never built.
  expect(b?.payload).toMatchObject({ outcome: "conflict", code: "CONFLICT" });
  expect(b?.payload).not.toHaveProperty("args");
  expect(b?.payload).not.toHaveProperty("argsOmitted");
  for (const e of [a, b]) for (const leak of ["acme", "fancy", "tighten", "1.0.0"]) expect(JSON.stringify(e)).not.toContain(leak);
});

test("a refused plugin.command is logged, never thrown", () => {
  const logs: string[] = [];
  const refusing = { enabled: true, recordPlugin: () => { throw new Error("EVENT_INVALID"); } };
  expect(journalPluginCommand(refusing, { plugin: ID, command: "c", outcome: "data", durationMs: 1 }, (m) => logs.push(m))).toBe(false);
  expect(logs).toEqual([expect.stringContaining("refused a plugin.command")]);
});

test("command outcomes of errors", () => {
  expect(["CONFLICT", "PLUGIN_COMMAND_ARGS", "PLUGIN_TX_INVALID", "PLUGIN_TIMEOUT", "PLUGIN_CRASHED", "PLUGIN_PROTOCOL", "PLUGIN_PERMISSION_DENIED", "PLUGIN_DISABLED"].map(commandOutcome)).toEqual([
    "conflict", "invalid", "invalid", "timeout", "failed", "failed", "refused", "refused",
  ]);
});

test("dropped log calls and stderr lines reach the stats", () => {
  const j = { enabled: true, recordPlugin: () => {} };
  const stats = makePluginStats({ journal: j });
  const log = makePluginLog({ write: () => {}, clock: () => 1000, dropped: (p, w) => stats.dropped(p, w) });
  for (let i = 0; i < 25; i++) log.call("k", `${ID}@1.0.0`, "info", "m");
  log.stderr("k", `${ID}@1.0.0`, "x".repeat(4000));
  for (let i = 0; i < 20; i++) log.stderr("k", `${ID}@1.0.0`, "y".repeat(4000));
  const [[id, p] = ["", undefined]] = stats.take();
  expect(id).toBe(ID);
  expect(p).toMatchObject({ logDropped: 5, stderrDropped: 5 });
});

test("every host call of a plugin job is counted, answered or refused", async () => {
  const j = { enabled: true, recordPlugin: () => {} };
  const stats = makePluginStats({ journal: j });
  const calls = makeHostCalls({ deps: () => undefined, keyNow: () => "", coldKey: () => undefined, log: makePluginLog({ write: () => {} }), egress: makeEgress(), stats });
  const ctx = { job: { id: "j1" }, role: "worker", owner: { kind: "console", id: "console" }, via: { plugin: `${ID}@1.0.0` } } as unknown as CallCtx;
  await Effect.runPromiseExit(calls.onCall(ctx, "made.up", {}));
  await Effect.runPromiseExit(calls.onCall(ctx, "media.pcm", {}));
  await Effect.runPromiseExit(calls.onCall(ctx, "log", { level: "info" }));
  // A job of no plugin is not counted.
  const { via: _via, ...noPlugin } = ctx;
  await Effect.runPromiseExit(calls.onCall(noPlugin, "log", {}));
  const [[id, p] = ["", undefined], ...rest] = stats.take();
  expect(rest).toEqual([]);
  expect(id).toBe(ID);
  expect(p?.families).toEqual([
    expect.objectContaining({ family: "other", calls: 2, errors: { INVALID_REQUEST: 2 } }),
    expect.objectContaining({ family: "log", calls: 1, errors: { PLUGIN_PROTOCOL: 1 } }),
  ]);
});

test("task 9's onRun event maps onto a plugin.command run", () => {
  const e = { plugin: `${ID}@1.0.0`, command: "tighten", principal: { kind: "console", id: "console" }, dryRun: false, ms: 12 };
  expect(commandRunOf({ ...e, outcome: "rebased" }, { txId: "01JC0000000000000000000002", args: { gapMs: 1 } })).toEqual({
    plugin: `${ID}@1.0.0`, command: "tighten", outcome: "rebased", durationMs: 12, txId: "01JC0000000000000000000002", args: { gapMs: 1 },
  });
  expect(commandRunOf({ ...e, outcome: "error", code: "PLUGIN_TX_INVALID", dryRun: true })).toEqual({ plugin: `${ID}@1.0.0`, command: "tighten", outcome: "invalid", code: "PLUGIN_TX_INVALID", dryRun: true, durationMs: 12 });
  expect(commandRunOf({ ...e, outcome: "error", code: "PLUGIN_NOT_FOUND" }).outcome).toBe("refused");
});
