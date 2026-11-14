// 6B-1 Task 9: the 5C asks of sub-project 6 spec §3.2, met through 5C's interfaces: `hwenc` is
// sized from the capability report's encoder session limit (2 where unknown, plan Decision 18),
// and the batch media kinds take a RAM-scaled memory rule.
import type { FinError, JobView } from "@finisher/contracts";
import { resolveMemoryRule } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { defaultPools } from "../src/jobs/pools.ts";
import { hwencSize, mediaCapabilities } from "../src/media/caps.ts";
import { BATCH_MEMORY, BATCH_MEMORY_LARGE, mediaBatchMemory } from "../src/media/memory.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";

const OWNER = { kind: "person", id: "p1" } as const;
const MiB = 1024 * 1024;

afterEach(closeTestEngines);

/** An engine that answers `media.capabilities` with `result` (or fails) and records pools. */
function fakeEngine(result: unknown, ok = true) {
  const pools = defaultPools(8);
  const job = { id: "j1" } as JobView;
  const engine = {
    submit: () => Effect.succeed(job),
    wait: () =>
      Effect.succeed([
        ok
          ? ({ ...job, state: "succeeded", result } as unknown as JobView)
          : ({ ...job, state: "failed", error: { code: "WORKER_CRASHED", message: "x", details: {} } } as unknown as JobView),
      ]),
    definePool: (name: string, size: number) => pools.define(name, size),
  };
  return { engine, pools };
}

const encoder = (backend: string, hw: boolean, sessionLimit?: number) => ({ backend, class: "gpu", codec: "h264", hw, intraOnly: true, sessionLimit });

describe("hwenc", () => {
  test("hwenc_resized_from_caps", async () => {
    const { engine, pools } = fakeEngine({ codecs: [], audio: [], encoders: [encoder("mf-hw", true, 3), encoder("svt-av1", false)], gpu: null, worker: {} });
    await Effect.runPromise(mediaCapabilities({ engine })(OWNER));
    expect(pools.size("hwenc")).toBe(3);
  });

  test("a failed ask, then a good one, resizes on the good one", async () => {
    const pools = defaultPools(8);
    let calls = 0;
    const good = { codecs: [], audio: [], encoders: [encoder("videotoolbox", true, 4)], gpu: null, worker: {} };
    const engine = {
      submit: () => Effect.succeed({ id: "j" } as JobView),
      wait: () => {
        calls++;
        const state = calls === 1 ? { state: "failed", error: { code: "WORKER_CRASHED", message: "x", details: {} } } : { state: "succeeded", result: good };
        return Effect.succeed([{ id: "j", ...state } as unknown as JobView]);
      },
      definePool: (name: string, size: number) => pools.define(name, size),
    };
    const caps = mediaCapabilities({ engine });
    await Effect.runPromise(Effect.flip(caps(OWNER)));
    expect(pools.size("hwenc")).toBe(2);
    await Effect.runPromise(caps(OWNER));
    expect(pools.size("hwenc")).toBe(4);
  });

  test("a failed report leaves 2", async () => {
    const { engine, pools } = fakeEngine(undefined, false);
    const e = await Effect.runPromise(Effect.flip(mediaCapabilities({ engine })(OWNER)));
    expect((e as FinError).code).toBe("WORKER_CRASHED");
    expect(pools.size("hwenc")).toBe(2);
  });

  test("a v1 report, or hardware encoders without a counted limit, keep 2", async () => {
    const { engine, pools } = fakeEngine({ codecs: { video: [] }, gpu: null, worker: {} });
    await Effect.runPromise(mediaCapabilities({ engine })(OWNER));
    expect(pools.size("hwenc")).toBe(2);
    expect(hwencSize({ encoders: [encoder("videotoolbox", true)] })).toBe(2);
    // Several hardware encoders: the smallest counted limit (the pool is shared).
    expect(hwencSize({ encoders: [encoder("mf-hw", true, 4), encoder("mf-ms", true, 1), encoder("svt-av1", false, 9)] })).toBe(1);
    expect(hwencSize({ encoders: [encoder("mf-hw", true, 0)] })).toBe(2);
  });
});

describe("batch media memory", () => {
  test("batch_memory_rule_scales", () => {
    expect(mediaBatchMemory({ width: 1920, height: 1080 })).toEqual({ ramFraction: 0.25, capMiB: 4096 });
    expect(mediaBatchMemory({ width: 1080, height: 1920 })).toEqual(BATCH_MEMORY);
    expect(mediaBatchMemory({ width: 3840, height: 2160 })).toEqual({ ramFraction: 0.35, capMiB: 6144 });
    expect(mediaBatchMemory({ width: 1921, height: 1080 })).toEqual(BATCH_MEMORY_LARGE);
    expect(mediaBatchMemory(undefined)).toEqual(BATCH_MEMORY);
    // min(4 GiB, 25 %) on 8 GiB and 64 GiB; min(6 GiB, 35 %) on 8 GiB.
    expect(resolveMemoryRule(BATCH_MEMORY, 8192 * MiB)).toBe(2048 * MiB);
    expect(resolveMemoryRule(BATCH_MEMORY, 65536 * MiB)).toBe(4096 * MiB);
    expect(resolveMemoryRule(BATCH_MEMORY_LARGE, 8192 * MiB)).toBe(Math.floor(0.35 * 8192 * MiB));
  });

  // 5C's `WORKER_OOM` carries `{used?, signal?}`, never `limit` (`src/jobs/host.ts`).
  test("a job over a RAM-scaled limit fails WORKER_OOM (5C's path)", async () => {
    const os = await import("node:os");
    const rule = { ramFraction: (64 * MiB) / os.totalmem(), capMiB: 4096 };
    const t = await testEngine({ timing: { rssSampleMs: 50 }, testKinds: { overrides: { "test.oom": { memory: rule } } } });
    const v = await t.runJob("test.oom", { mib: 256, holdMs: 5000 });
    expect(v.state).toBe("failed");
    expect(v.error?.code).toBe("WORKER_OOM");
    expect(v.error?.details).toHaveProperty("used");
  });
});
