// The engine's events in the diagnostics journal (D34; 5C-1 plan Decision 42, Task 12), over a
// real fin-node `Journal`: `job.done` and `worker.exit` recorded as `finisherd`, refusals logged
// and never thrown, nothing while recording is off.
import { mkdtempSync, rmSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import type { Envelope, ExportPage } from "@finisher/core-types";
import { Journal } from "@finisher/fin-node";
import { afterEach, expect, test } from "vitest";
import type { JobDoneEvent, WorkerExitEvent } from "../../src/jobs/events.ts";
import { journalSink } from "../../src/jobs/journal-sink.ts";

let journal: Journal | undefined;
let dir: string | undefined;
afterEach(async () => {
  await journal?.close();
  journal = undefined;
  if (dir !== undefined) rmSync(dir, { recursive: true, force: true });
  dir = undefined;
});

function open(privacy: object): Journal {
  dir = mkdtempSync(join(tmpdir(), "fin-jobs-journal-"));
  journal = Journal.open(dir, JSON.stringify(privacy), hostname());
  return journal;
}

async function entries(j: Journal, kinds: string[]): Promise<Envelope[]> {
  const page = JSON.parse(await j.export(JSON.stringify({ kinds }), null, 1000)) as ExportPage;
  return page.lines;
}

const done: JobDoneEvent = {
  kind: "test.echo",
  family: "test",
  outcome: "succeeded",
  attempts: 1,
  priority: "normal",
  warm: false,
  queuedMs: 3,
  runMicros: 1200,
  isolated: false,
  helpers: 0,
  extra: { notAField: true },
};
const exit: WorkerExitEvent = { role: "worker", family: "testplugin", reason: "done", jobsServed: 2, lifetimeS: 1, sandboxed: false, exitCode: 0, pluginId: "@acme/thing" };

test("job_done_and_worker_exit_reach_the_journal", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  sink.jobDone(done, "01JOB");
  sink.workerExit(exit);
  const [d] = await entries(j, ["job.done"]);
  expect(d).toMatchObject({ kind: "job.done", kv: 2, source: "finisherd", payload: { kind: "test.echo", outcome: "succeeded", runMicros: 1200 } });
  expect(d?.payload).not.toHaveProperty("extra");
  expect(d?.payload).not.toHaveProperty("notAField");
  expect(d?.payload).not.toHaveProperty("media");
  const [w] = await entries(j, ["worker.exit"]);
  expect(w).toMatchObject({ kind: "worker.exit", kv: 2, source: "finisherd", payload: { role: "worker", family: "testplugin", reason: "done", jobsServed: 2 } });
  // No raw plugin id reaches the journal.
  expect(JSON.stringify(w)).not.toContain("acme");
  expect(logs).toEqual([]);
});

test("worker_exit_kv2_carries_plugin", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  sink.workerExit({ ...exit, family: "plugin", reason: "crashed", exitCode: 3, lifetimeS: 7 });
  sink.workerExit({ ...exit, family: "plugin", reason: "idleExpired", pluginId: "@finisher/captions-basic" });
  const { pluginId: _none, ...plain } = exit;
  sink.workerExit({ ...plain, family: "media" });
  const ws = await entries(j, ["worker.exit"]);
  expect(ws.map((w) => w.kv)).toEqual([2, 2, 2]);
  const hashed = (ws[0]?.payload as { plugin?: { hashed?: string } }).plugin?.hashed;
  expect(hashed).toMatch(/^k1:[0-9a-f]{32}$/);
  expect(ws[1]?.payload).toMatchObject({ plugin: { bundled: "finisher/captions-basic" } });
  expect(ws[2]?.payload).not.toHaveProperty("plugin");
  // `plugin.exit` from the same exits, none for a process of no plugin.
  const es = await entries(j, ["plugin.exit"]);
  expect(es.map((e) => e.payload)).toEqual([
    { plugin: { hashed }, reason: "crash", exitCode: 3, jobsServed: 2, uptimeS: 7 },
    { plugin: { bundled: "finisher/captions-basic" }, reason: "idle", exitCode: 0, jobsServed: 2, uptimeS: 1 },
  ]);
  expect(es.every((e) => e.source === "finisherd" && e.kv === 1)).toBe(true);
  expect(JSON.stringify([...ws, ...es])).not.toContain("acme");
  expect(logs).toEqual([]);
});

test("a_refused_event_is_logged_not_thrown", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  expect(() => sink.jobDone({ ...done, outcome: "exploded" as never }, "01BAD")).not.toThrow();
  expect(logs).toEqual([expect.stringContaining("refused job 01BAD's job.done")]);
  sink.forward({ family: "test", job: "j" }, "test.notAKind", {});
  expect(logs.at(-1)).toContain("not a journal kind");
  expect(await entries(j, ["job.done"])).toEqual([]);
});

test("nothing_is_recorded_while_off", async () => {
  const j = open({ diagnostics: "off" });
  const sink = journalSink(j, () => {});
  sink.jobDone(done, "01OFF");
  sink.workerExit(exit);
  expect(await entries(j, ["job.done", "worker.exit", "plugin.exit"])).toEqual([]);
});

const media = { codec: "av1", backend: "dav1d", hw: false, resolution: "hd1080", frames: 3, rtFactorMilli: 4200, cacheHit: true, copyPath: "copy" };
const look: JobDoneEvent = { ...done, kind: "media.look", family: "media", extra: { media, notAField: true } };

test("a_media_look_job_done_carries_its_media_object", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  sink.jobDone(look, "01LOOK");
  const [d] = await entries(j, ["job.done"]);
  expect(d).toMatchObject({ kv: 2, payload: { kind: "media.look", family: "media", media } });
  expect(d?.payload).not.toHaveProperty("notAField");
  expect(logs).toEqual([]);
});

test("a_media_object_with_names_outside_the_tables_reads_as_other", async () => {
  const j = open({ diagnostics: "on" });
  const sink = journalSink(j, () => {});
  const hostile = { ...media, codec: "/home/someone/secret.mp4", backend: "NVIDIA GeForce RTX 5090" };
  sink.jobDone({ ...look, extra: { media: hostile } }, "01HOSTILE");
  const [d] = await entries(j, ["job.done"]);
  expect(d?.payload).toMatchObject({ media: { codec: "other", backend: "other" } });
  expect(JSON.stringify(d)).not.toContain("secret");
  expect(JSON.stringify(d)).not.toContain("NVIDIA");
});

test("a_malformed_media_object_costs_the_job_only_its_media_object", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  sink.jobDone({ ...look, extra: { media: { ...media, resolution: "enormous", frames: "many" } } }, "01BADMEDIA");
  const [d] = await entries(j, ["job.done"]);
  expect(d).toMatchObject({ kv: 2, payload: { kind: "media.look", outcome: "succeeded" } });
  expect(d?.payload).not.toHaveProperty("media");
  expect(logs).toEqual([expect.stringContaining("refused job 01BADMEDIA's job.done")]);
});

test("worker_media_diagnostics_are_forwarded_to_their_kinds", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  const origin = { family: "media", job: "j1" };
  sink.forward(origin, "media.capabilities", {
    codecs: [{ codec: "av1", decode: "sw", encode: "sw", backends: [{ name: "dav1d", class: "software" }] }],
    gpu: { backend: "vulkan", adapterType: "cpu", vendorId: 65541, deviceId: 0, driverMajor: 25, software: true },
    copyPath: "copy",
    sandbox: "none",
    renderVersion: 1,
  });
  const histogram = { start: 1_794_571_200_000, windowMs: 60_000, count: 2, maxMicros: 900, buckets: [0, 0, 0, 0, 0, 0, 0, 0, 0, 2] };
  sink.forward(origin, "render.stats", { phase: "decode", backend: "dav1d", histogram });
  const [c] = await entries(j, ["media.capabilities"]);
  expect(c).toMatchObject({ kv: 1, source: "finisherd", payload: { gpu: { vendorId: 65541, deviceId: 0, driverMajor: 25 }, sandbox: "none" } });
  const [r] = await entries(j, ["render.stats"]);
  expect(r).toMatchObject({ kv: 1, source: "finisherd", payload: { phase: "decode", backend: "dav1d", histogram: { count: 2 } } });
  expect(logs).toEqual([]);
});
