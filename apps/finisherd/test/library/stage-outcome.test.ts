// The stage pump's pure parts (plan 5D Task 11; Decision 40): what a job's end does to each
// item's row, and the server-wide in-flight set.
import type { JobView } from "@finisher/contracts";
import { describe, expect, test } from "vitest";
import { InFlight } from "../../src/library/inflight.ts";
import { itemFates, rescanItems, rowOf } from "../../src/library/stage-outcome.ts";
import { StageRegistry } from "../../src/library/stages.ts";

type Ended = Pick<JobView, "state" | "result" | "error" | "itemErrors">;
const err = (code: string) => ({ code, message: code, details: {} });

describe("item fates", () => {
  test("items_done_errors_failed_poison_permanent", () => {
    const job: Ended = {
      state: "succeeded",
      result: { items: { a: { bytes: 1 } }, itemErrors: [] },
      itemErrors: [{ item: "b", error: err("FILE_CHANGED") }, { item: "c", error: err("POISON_ITEM") }, { item: "d", error: err("LIBRARY_OFFLINE") }],
    } as Ended;
    const f = itemFates(job, ["a", "b", "c", "d", "e"], false);
    expect(Object.fromEntries(f)).toEqual({
      a: { fate: "done" },
      // The catalog is behind the file: no attempt, its directory is listed again.
      b: { fate: "pending", code: "FILE_CHANGED" },
      c: { fate: "failed", code: "POISON_ITEM", permanent: true },
      d: { fate: "pending", code: "LIBRARY_OFFLINE" },
      e: { fate: "failed", code: "STAGE_ITEM_UNANSWERED", permanent: false },
    });
    const base = { fp: "x", stage: "s", version: "v", at: 5 };
    expect(rowOf(f.get("a")!, base, 1)).toMatchObject({ state: "done", attempts: 1 });
    expect(rowOf(f.get("b")!, base, 1)).toMatchObject({ state: "stale", attempts: 1, code: "FILE_CHANGED" });
    expect(rescanItems(f)).toEqual(["b"]);
    expect(rowOf(f.get("c")!, base, 0)).toMatchObject({ state: "failed", attempts: 3 });
    expect(rowOf(f.get("d")!, base, 1)).toMatchObject({ state: "stale", attempts: 1, code: "LIBRARY_OFFLINE" });
  });

  test("files_the_catalog_is_behind_on_cost_no_attempt", () => {
    const job = {
      state: "succeeded", result: { items: {} },
      itemErrors: [{ item: "a", error: err("FILE_NOT_FOUND") }, { item: "b", error: err("FILE_CLOUD_ONLY") }, { item: "c", error: err("ASSET_UNREADABLE") }],
    } as Ended;
    const f = itemFates(job, ["a", "b", "c"], false);
    expect(f.get("a")).toEqual({ fate: "pending", code: "FILE_NOT_FOUND" });
    expect(f.get("b")).toEqual({ fate: "pending", code: "FILE_CLOUD_ONLY" });
    expect(f.get("c")).toEqual({ fate: "failed", code: "ASSET_UNREADABLE", permanent: false });
    expect(rescanItems(f)).toEqual(["a", "b"]);
  });

  test("whole_job_failures_and_free_cancels", () => {
    expect(itemFates({ state: "failed", error: err("TEST_FAIL") } as Ended, ["a"], false).get("a")).toEqual({ fate: "failed", code: "TEST_FAIL", permanent: false });
    expect(itemFates({ state: "failed", error: err("RESOURCE_STUCK") } as Ended, ["a"], false).get("a")).toEqual({ fate: "pending", code: "RESOURCE_STUCK" });
    expect(itemFates({ state: "cancelled" } as Ended, ["a"], true).get("a")).toEqual({ fate: "pending", code: "JOB_CANCELLED" });
    // Cancelled by someone else: an attempt.
    expect(itemFates({ state: "cancelled" } as Ended, ["a"], false).get("a")).toEqual({ fate: "failed", code: "JOB_CANCELLED", permanent: false });
    // A real item error before a pump cancel still counts.
    const cut = { state: "cancelled", itemErrors: [{ item: "a", error: err("POISON_ITEM") }] } as Ended;
    expect(itemFates(cut, ["a", "b"], true).get("a")).toMatchObject({ fate: "failed", permanent: true });
    expect(itemFates(cut, ["a", "b"], true).get("b")).toMatchObject({ fate: "pending" });
  });
});

describe("in-flight set", () => {
  test("keys_reserve_rename_wait_release", () => {
    const f = new InFlight();
    const c = f.add({ jobId: "r1", library: "L1", stage: "s", version: "v", items: ["fpA", "fpB"], reserved: true });
    expect(f.stageJob("fpA", "s")).toBe(c);
    expect(f.stageJob("fpA", "t")).toBeUndefined();
    const j = f.rename("r1", "job1")!;
    expect(j.reserved).toBe(false);
    expect(f.stageJob("fpB", "s")?.jobId).toBe("job1");
    f.wait(j, "L2", "fpA", 7);
    f.wait(j, "L1", "fpA", 1); // the owner never waits on itself
    expect([...j.waiters]).toEqual([["L2", new Map([["fpA", 7]])]]);
    expect(f.count("L1", "s")).toBe(1);
    f.add({ jobId: "p1", library: "L1", stage: "probe", version: "", items: ["12"] });
    expect(f.probing("L1", 12)).toBe(true);
    expect(f.probing("L2", 12)).toBe(false);
    expect(f.release("job1")?.jobId).toBe("job1");
    expect(f.stageJob("fpA", "s")).toBeUndefined();
    expect(f.size()).toBe(1);
  });

  test("registry_checks_specs", () => {
    const r = new StageRegistry();
    const spec = { name: "x.a", version: "1", appliesTo: () => true, jobKind: "k", batch: 1, params: () => ({}) };
    r.register(spec);
    r.register({ ...spec, name: "x.b", dependsOn: ["probe", "x.a"] });
    expect(r.get("x.b")?.dependsOn).toEqual(["x.a"]);
    expect(r.dependencies(r.get("x.b")!)).toEqual([{ stage: "x.a", version: "1" }]);
    expect(r.dependencies({ ...spec, name: "x.c", dependsOn: ["x.none"] })).toBeUndefined();
    expect(() => r.register({ ...spec, name: "Bad" })).toThrow(/name/);
    expect(() => r.register({ ...spec, name: "x.c", maxInFlight: 0 })).toThrow(/maxInFlight/);
    expect(() => r.register({ ...spec, name: "x.d", version: "" })).toThrow(/version/);
    expect(r.forLibrary({ stages: [] }).map((s) => s.name)).toEqual(["x.a", "x.b"]);
    expect(r.forLibrary({ stages: ["x.b"] }).map((s) => s.name)).toEqual(["x.b"]);
  });
});
