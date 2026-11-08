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
  extra: { notInKv1: true },
};
const exit: WorkerExitEvent = { role: "worker", family: "testplugin", reason: "done", jobsServed: 2, lifetimeS: 1, sandboxed: false, exitCode: 0, pluginId: "@acme/thing" };

test("job_done_and_worker_exit_reach_the_journal", async () => {
  const j = open({ diagnostics: "on" });
  const logs: string[] = [];
  const sink = journalSink(j, (_l, m) => logs.push(m));
  sink.jobDone(done, "01JOB");
  sink.workerExit(exit);
  const [d] = await entries(j, ["job.done"]);
  expect(d).toMatchObject({ kind: "job.done", kv: 1, source: "finisherd", payload: { kind: "test.echo", outcome: "succeeded", runMicros: 1200 } });
  expect(d?.payload).not.toHaveProperty("extra");
  const [w] = await entries(j, ["worker.exit"]);
  expect(w).toMatchObject({ kind: "worker.exit", source: "finisherd", payload: { role: "worker", family: "testplugin", reason: "done", jobsServed: 2 } });
  // No raw plugin id reaches the journal.
  expect(JSON.stringify(w)).not.toContain("acme");
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
  expect(await entries(j, ["job.done", "worker.exit"])).toEqual([]);
});
