// The pure retry policy and isolation (5C-1 plan Task 10, Decisions 31, 32).
import fc from "fast-check";
import { describe, expect, test } from "vitest";
import { finError } from "../../src/errors.ts";
import type { JobHostExitInfo } from "../../src/jobs/host.ts";
import { afterRun, backoffMs, budget, classify, type FailureFacts, type IsolationState, nextRun, nextStep, startIsolation, withoutDone } from "../../src/jobs/isolation.ts";

const exit = (o: Partial<JobHostExitInfo> & { reason: string }): JobHostExitInfo => ({ stderrTail: "", lifetimeMs: 1, ...o }) as unknown as JobHostExitInfo;
const err = (code: string) => finError(code, code);

describe("classification", () => {
  test("classification_table", () => {
    // Decision 31's rows, in its order.
    const rows: Array<[string, FailureFacts, string]> = [
      ["SIGBUS", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "exited", signal: "bus" }) }, "resource"],
      ["EXCEPTION_IN_PAGE_ERROR", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "exited", code: 0xc0000006 }) }, "resource"],
      ["the host's resource fault", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "resourceFault" }) }, "resource"],
      ["abandoned (stuck in the kernel)", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "abandoned" }) }, "resource"],
      ["a crash, the resource unhealthy", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "exited", code: 3 }), healthy: false }, "resource"],
      ["exit without an answer", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "exited", code: 3 }) }, "crash"],
      ["another signal", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "exited", signal: "segv" }) }, "crash"],
      ["protocolError", { cause: "exit", error: err("WORKER_CRASHED"), exit: exit({ reason: "protocolError", what: "seq" } as never) }, "crash"],
      ["an engine-side protocol error", { cause: "protocol", error: err("WORKER_CRASHED") }, "crash"],
      ["WORKER_OOM cold", { cause: "exit", error: err("WORKER_OOM"), exit: exit({ reason: "oom" }) }, "oom"],
      ["WORKER_OOM on a reused warm worker", { cause: "exit", error: err("WORKER_OOM"), exit: exit({ reason: "oom" }), warmReused: true }, "oomWarm"],
      ["WORKER_HUNG", { cause: "exit", error: err("WORKER_HUNG"), exit: exit({ reason: "hung" }) }, "transient"],
      ["JOB_STALLED", { cause: "exit", error: err("JOB_STALLED"), exit: exit({ reason: "stalled" }) }, "transient"],
      ["JOB_TIMEOUT", { cause: "exit", error: err("JOB_TIMEOUT"), exit: exit({ reason: "timedOut" }) }, "transient"],
      ["HELPER_FAILED", { cause: "helper", error: err("HELPER_FAILED") }, "transient"],
      ["noHello", { cause: "exit", error: err("WORKER_UNAVAILABLE"), exit: exit({ reason: "noHello" }) }, "transient"],
      ["SpawnFailed", { cause: "spawn", error: err("WORKER_UNAVAILABLE") }, "transient"],
      ["error retry: backoff", { cause: "worker", error: err("IO_ERROR"), retry: "backoff" }, "transient"],
      ["error retry: fresh", { cause: "worker", error: err("GPU_DEVICE_LOST"), retry: "fresh" }, "fresh"],
      ["error retry: never", { cause: "worker", error: err("BAD_INPUT"), retry: "never" }, "deterministic"],
      ["error without a hint", { cause: "worker", error: err("BAD_INPUT") }, "deterministic"],
      ["WORKER_PROTOCOL_MISMATCH at hello", { cause: "mismatch", error: err("WORKER_PROTOCOL_MISMATCH") }, "mismatch"],
      ["WORKER_PROTOCOL_MISMATCH from the host", { cause: "exit", error: err("WORKER_PROTOCOL_MISMATCH"), exit: exit({ reason: "protocolMismatch" }) }, "mismatch"],
      ["a failed prepare", { cause: "prepare", error: err("PREP_BAD") }, "hook"],
      ["a failed onSucceeded", { hook: true, error: err("HOOK_BAD") }, "hook"],
      ["cancel", { cause: "stopped", error: err("JOB_CANCELLED") }, "cancel"],
      ["a kill for cancel", { cause: "exit", error: err("JOB_CANCELLED"), exit: exit({ reason: "killed", kill: "cancel" } as never) }, "cancel"],
      ["a planted link", { cause: "tampered", error: err("JOB_FAILED") }, "deterministic"],
      ["a file prepare left wrong", { cause: "files", error: finError("IO_ERROR", "x", { problem: "missing" }) }, "deterministic"],
      ["the directory could not be made", { cause: "files", error: finError("IO_ERROR", "x", { step: "jobDir" }) }, "transient"],
    ];
    for (const [what, facts, cls] of rows) expect([what, classify(facts).cls]).toEqual([what, cls]);
    expect(classify(rows.find((r) => r[0].includes("unhealthy"))![1]).error.details).toMatchObject({ resourceHealthy: false });
  });

  test("next_step_table", () => {
    const c = { attempts: 1, maxAttempts: 3, interactive: false, batch: true, inFlight: 1, freshUsed: false };
    expect(nextStep("crash", c)).toEqual({ do: "isolate" });
    expect(nextStep("crash", { ...c, batch: false })).toEqual({ do: "retry", delayMs: 10_000 });
    expect(nextStep("crash", { ...c, interactive: true })).toEqual({ do: "retry", delayMs: 0 });
    expect(nextStep("crash", { ...c, interactive: true, attempts: 2, maxAttempts: 2 })).toEqual({ do: "fail" });
    expect(nextStep("oom", c)).toEqual({ do: "isolate" });
    expect(nextStep("oom", { ...c, inFlight: 0 })).toEqual({ do: "fail" });
    expect(nextStep("resource", c)).toEqual({ do: "retry", delayMs: 10_000 });
    expect(nextStep("transient", { ...c, attempts: 2 })).toEqual({ do: "retry", delayMs: 20_000 });
    expect(nextStep("transient", { ...c, attempts: 3 })).toEqual({ do: "fail" });
    expect(nextStep("fresh", c)).toEqual({ do: "retry", delayMs: 0 });
    expect(nextStep("fresh", { ...c, freshUsed: true })).toEqual({ do: "fail" });
    expect(nextStep("oomWarm", c)).toEqual({ do: "retry", delayMs: 0 });
    for (const cls of ["deterministic", "mismatch", "hook", "cancel"] as const) expect(nextStep(cls, c)).toEqual({ do: "fail" });
  });

  test("backoff_is_5s_times_2_to_the_n", () => {
    expect([1, 2, 3].map(backoffMs)).toEqual([10_000, 20_000, 40_000]);
    expect(backoffMs(0)).toBe(5000);
    expect(backoffMs(6)).toBe(300_000);
    expect(backoffMs(40)).toBe(300_000);
  });
});

/** What a worker does with a run: the items it began, acknowledged, and whether it died. */
type Worker = (run: ReadonlyArray<string>) => { begun: string[]; done: string[]; inFlight: string[]; crashed: boolean };

/** Dies inside a poison item (after its `begin`). */
const inFlightPoison =
  (poison: ReadonlySet<string>): Worker =>
  (run) => {
    const begun: string[] = [];
    const done: string[] = [];
    for (const i of run) {
      begun.push(i);
      if (poison.has(i)) return { begun, done, inFlight: [i], crashed: true };
      done.push(i);
    }
    return { begun, done, inFlight: [], crashed: false };
  };

/** Dies before a poison item's `begin` (`between: true`). */
const betweenPoison =
  (poison: ReadonlySet<string>): Worker =>
  (run) => {
    const begun: string[] = [];
    const done: string[] = [];
    for (const i of run) {
      if (poison.has(i)) return { begun, done, inFlight: [], crashed: true };
      begun.push(i);
      done.push(i);
    }
    return { begun, done, inFlight: [], crashed: false };
  };

/** Dies inside the second item of any run of two or more (no single item explains it). */
const crashOnSecond: Worker = (run) => {
  if (run.length < 2) return { begun: [...run], done: [...run], inFlight: [], crashed: false };
  return { begun: run.slice(0, 2), done: run.slice(0, 1), inFlight: run.slice(1, 2), crashed: true };
};

/** Runs a batch through isolation as the engine does; checks that no acknowledged item is begun again. */
function simulate(items: ReadonlyArray<string>, worker: Worker) {
  const acked = new Set<string>();
  const runs: string[][] = [];
  const ran = (run: ReadonlyArray<string>) => {
    runs.push([...run]);
    const r = worker(run);
    for (const i of r.begun) if (acked.has(i)) throw new Error(`${i} was acknowledged and ran again`);
    for (const i of r.done) acked.add(i);
    return r;
  };
  const first = ran(items);
  if (!first.crashed) return { exhausted: false, state: undefined as IsolationState | undefined, acked, runs, crashRuns: 0 };
  let state: IsolationState | "exhausted" = afterRun(startIsolation(items), { items, ...first });
  let crashRuns = 1;
  while (state !== "exhausted") {
    const next = nextRun(state);
    if (next === undefined) break;
    const r = ran(next);
    if (r.crashed) crashRuns++;
    state = afterRun(state, { items: next, ...r });
  }
  return { exhausted: state === "exhausted", state: state === "exhausted" ? undefined : state, acked, runs, crashRuns };
}

const log2 = (n: number): number => (n <= 1 ? 0 : Math.ceil(Math.log2(n)));
const batch = fc.integer({ min: 1, max: 256 }).chain((n) => {
  const items = Array.from({ length: n }, (_, i) => `i${i}`);
  return fc.tuple(fc.constant(items), fc.subarray(items));
});

describe("isolation", () => {
  test("isolation_properties", () => {
    fc.assert(
      fc.property(batch, ([items, poison]) => {
        const n = items.length;
        const p = poison.length;
        const r = simulate(items, inFlightPoison(new Set(poison)));
        expect(r.exhausted).toBe(false);
        const poisoned = r.state?.poisoned ?? [];
        expect([...poisoned].sort()).toEqual([...poison].sort());
        expect(r.acked.size + poisoned.length).toBe(n);
        expect(r.crashRuns).toBeLessThanOrEqual(Math.min(2 * n, 2 * log2(n) + 2 * p));
        if (p === 0) expect(r.crashRuns).toBe(0);
      }),
      { numRuns: 300 },
    );
  });

  test("a_crash_between_items_is_halved_to_the_poison", () => {
    // With nothing in flight the remainder is halved; one or two poison items stay inside the budget.
    const upToTwo = fc.integer({ min: 1, max: 256 }).chain((n) => {
      const items = Array.from({ length: n }, (_, i) => `i${i}`);
      return fc.tuple(fc.constant(items), fc.subarray(items, { maxLength: Math.min(2, n) }));
    });
    fc.assert(
      fc.property(upToTwo, ([items, poison]) => {
        const r = simulate(items, betweenPoison(new Set(poison)));
        expect(r.exhausted).toBe(false);
        expect([...(r.state?.poisoned ?? [])].sort()).toEqual([...poison].sort());
        expect(r.acked.size + poison.length).toBe(items.length);
      }),
      { numRuns: 300 },
    );
  });

  test("unexplained_crashes_exhaust_the_budget", () => {
    for (let n = 2; n <= 256; n++) {
      const items = Array.from({ length: n }, (_, i) => `i${i}`);
      const r = simulate(items, crashOnSecond);
      const allowed = 2 * log2(n);
      // Each crashing run resolves two items, so ⌊n/2⌋ crashes would finish the batch.
      if (Math.floor(n / 2) > allowed) {
        expect([n, r.exhausted, r.crashRuns]).toEqual([n, true, allowed + 1]);
      } else {
        expect([n, r.exhausted, r.crashRuns, r.acked.size]).toEqual([n, false, Math.floor(n / 2), n]);
      }
      expect(r.state?.poisoned ?? []).toEqual([]);
    }
  });

  test("in_flight_sets_are_bisected_then_the_rest", () => {
    // A probe group of 4 begun together, acknowledged only after the reply (5D): the set is halved.
    let s = startIsolation(["a", "b", "c", "d", "e", "f"]);
    const next = afterRun(s, { items: ["a", "b", "c", "d", "e", "f"], done: [], inFlight: ["a", "b", "c", "d"], crashed: true });
    if (next === "exhausted") throw new Error("exhausted");
    expect(next.queue).toEqual([["a", "b"], ["c", "d"], ["e", "f"]]);
    s = next;
    const alone = afterRun(s, { items: ["a", "b"], done: ["a"], inFlight: ["b"], crashed: true });
    if (alone === "exhausted") throw new Error("exhausted");
    expect(alone.queue[0]).toEqual(["b"]);
    expect(alone.poisoned).toEqual([]);
    // A run of one that crashes poisons it, in flight or not.
    const poisoned = afterRun(alone, { items: ["b"], done: [], crashed: true });
    if (poisoned === "exhausted") throw new Error("exhausted");
    expect(poisoned.poisoned).toEqual(["b"]);
    expect(poisoned.queue).toEqual([["c", "d"], ["e", "f"]]);
    expect(budget(poisoned)).toBe(Math.min(12, 2 * 3 + 2));
  });

  test("acknowledged_items_are_dropped_on_resume", () => {
    const s = startIsolation(["a", "b", "c"]);
    expect(withoutDone(s, new Set(["a", "c"])).queue).toEqual([["b"]]);
    expect(nextRun(withoutDone(s, new Set(["a", "b", "c"])))).toBeUndefined();
  });
});
