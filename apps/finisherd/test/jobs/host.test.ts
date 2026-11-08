// The engine's host wrapper over a fake `JobHost` (9a review): deterministic event orders.
import type { JobHostEvent, WorkerMsg } from "@finisher/core-types";
import { afterEach, describe, expect, test } from "vitest";
import { EngineHost, type ProcEvent } from "../../src/jobs/host.ts";
import { FakeHost } from "../support/fake-host.ts";
import { closeTestEngines, testEngine } from "../support/jobs.ts";

afterEach(closeTestEngines);

const hello = (proc: number, kind: string): JobHostEvent => ({
  type: "hello",
  proc,
  hello: { v: 1, minor: 0, role: "worker", family: "test", kinds: [{ kind, version: 1 }], warm: false, pid: 4242 },
});

describe("engine host", () => {
  test("a_whole_run_in_one_turn_after_spawned_succeeds", async () => {
    // hello, the result and the exit all arrive in the turn of `spawned`: nothing is lost or reordered.
    const fake = new FakeHost((host, proc) => {
      host.emit(hello(proc, "test.echo"));
      const start = host.sent.find((s) => s.proc === proc && s.msg.t === "start");
      const job = start?.msg.t === "start" ? start.msg.job.id : "?";
      host.emit({ type: "message", proc, msg: { t: "result", job, value: { value: 7, pid: 4242 } } as WorkerMsg });
      host.exit(proc, { reason: "exited", code: 0 });
    });
    const t = await testEngine({ jobHost: fake.factory });
    const v = await t.runJob("test.echo", { value: 7 });
    expect(v.state).toBe("succeeded");
    expect(v.result).toEqual({ value: 7, pid: 4242 });
    expect(t.sink.exits).toEqual([expect.objectContaining({ reason: "done", jobsServed: 1 })]);
  });

  test("events_of_unknown_processes_are_dropped", async () => {
    const fake = new FakeHost(() => {});
    const logs: string[] = [];
    const host = await EngineHost.start({ binaryDir: "/nowhere" }, () => 0, (_l, m) => logs.push(m), fake.factory);
    const seen: ProcEvent[] = [];
    fake.emit(hello(99, "test.echo"));
    fake.emit({ type: "exit", proc: 99, info: { reason: "exited", code: 0, stderrTail: "", lifetimeMs: 1 } });
    const group = host.openGroup({ label: "g" });
    const proc = await host.spawn(group, { role: "worker", family: "test", program: { bundled: "fin-workers" }, cwd: "/" }, (e) => seen.push(e));
    fake.emit(hello(proc, "test.echo"));
    fake.exit(proc, { reason: "exited", code: 0 });
    // Its exit was handled: a late event of the same id goes nowhere.
    fake.emit({ type: "stderr", proc, line: "late" });
    expect(seen.map((e) => e.type)).toEqual(["hello", "exit"]);
    expect(logs.filter((m) => m.includes("unknown process"))).toHaveLength(3);
    expect(host.processes()).toEqual([]);
  });
});
