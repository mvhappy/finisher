// The job event sink (5C-1 plan Task 7, Decision 42).
import type { JobDone } from "@finisher/core-types";
import { Effect } from "effect";
import { describe, expect, test } from "vitest";
import { JobEventSinkTag, layerMemorySink, memorySink, type WorkerExitEvent } from "../../src/jobs/events.ts";

const done = (n: number): JobDone => ({
  kind: "test.echo",
  family: "test",
  outcome: "succeeded",
  attempts: 1,
  priority: "normal",
  warm: false,
  queuedMs: n,
  runMicros: 10,
  isolated: false,
  helpers: 0,
});

const exit: WorkerExitEvent = {
  role: "worker",
  family: "testplugin",
  reason: "done",
  jobsServed: 1,
  lifetimeS: 2,
  sandboxed: false,
  pluginId: "test.plugin",
};

describe("job events", () => {
  test("memory_sink_caps_and_records", async () => {
    const s = memorySink(3);
    for (let i = 0; i < 5; i++) s.jobDone({ ...done(i), extra: { i } }, `j${i}`);
    expect(s.done.map((d) => d.queuedMs)).toEqual([2, 3, 4]);
    expect(s.done[2]?.extra).toEqual({ i: 4 });
    s.workerExit(exit);
    expect(s.exits).toEqual([exit]);
    s.forward({ family: "test", job: "j1" }, "test.diag", { n: 1 });
    expect(s.forwarded).toEqual([{ origin: { family: "test", job: "j1" }, kind: "test.diag", payload: { n: 1 } }]);
    expect(memorySink().done.length).toBe(0);
    const fromLayer = await Effect.runPromise(Effect.provide(Effect.gen(function* () {
      return yield* JobEventSinkTag;
    }), layerMemorySink(2)));
    fromLayer.jobDone(done(1), "j1");
    expect((fromLayer as ReturnType<typeof memorySink>).done.length).toBe(1);
  });
});
