// `media.look` and `media.capabilities` (6A plan Task 13): the handlers over a real project
// engine and a real job engine with a real `fin-workers media` (lavapipe with
// `FIN_GPU=software`; without an adapter the rendering tests skip unless `FIN_REQUIRE_GPU=1`).
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
// v1 until the worker answers v2 (6B-1 Task 8).
import type { LookResult, LegacyMediaCapabilities as MediaCapabilities } from "@finisher/core-types";
import type { Engine } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { mediaCapabilities } from "../src/media/caps.ts";
import { LOOK_MAX_SIDE, look, type LookRequest, type MediaRpcDeps } from "../src/media/look.ts";
import type { JobEngineOptions } from "../src/jobs/layer.ts";
import { closeTestEngines, OWNER, type TestEngine, testEngine, until } from "./support/jobs.ts";
import { type LookProject, lookProject, makeAv1Fixture } from "./support/media.ts";

const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = mkdtempSync(join(tmpdir(), "fin-media-rpc-"));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const failure = async <A>(e: Effect.Effect<A, FinError>): Promise<FinError> => {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit.value)}`);
  const reason = exit.cause.reasons.find((r) => r._tag === "Fail");
  if (reason === undefined || reason._tag !== "Fail") throw new Error("expected a FinError");
  return reason.error;
};

interface Setup {
  readonly p: LookProject;
  readonly t: TestEngine;
  readonly deps: MediaRpcDeps;
  readonly serverData: string;
  look(over?: Partial<LookRequest>): Effect.Effect<unknown, FinError>;
}

async function setup(engineOpts: Partial<JobEngineOptions> = {}): Promise<Setup> {
  const serverData = mkdtempSync(join(work, "sd-"));
  const p = await lookProject(serverData, mkdtempSync(join(work, "proj-")) + "/p", media);
  engines.push(p.engine);
  const projects = {
    prepareMediaJob: (projectId: string, branch: string, timeline: string, at: ReadonlyArray<string>, inputDir: string, fontCache: string | undefined, paramsJson: string | undefined) =>
      projectId !== p.engine.projectId
        ? Effect.fail(toFinError(new Error("unknown project")))
        : Effect.tryPromise({
            try: async () => JSON.parse(await p.engine.prepareMediaJob(branch, timeline, [...at], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null })),
            catch: toFinError,
          }),
    dirOf: (projectId: string) => (projectId === p.engine.projectId ? p.dir : undefined),
  };
  const t = await testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects }), ...engineOpts });
  const deps: MediaRpcDeps = {
    engine: t.engine,
    registry: {
      ensure: (id) => (id === p.engine.projectId ? Effect.void : Effect.fail(toFinError(Object.assign(new Error("no such project"), { finNode: true, code: "PROJECT_NOT_FOUND" })))),
      resolveBranch: (_id, ref) => Effect.succeed(ref),
      snapshot: (_id, branch) => Effect.tryPromise({ try: async () => JSON.parse(await p.engine.snapshot(branch)), catch: toFinError }),
    },
  };
  return { p, t, deps, serverData, look: (over = {}) => look(deps, OWNER, { projectId: p.engine.projectId, at: ["0"], ...over }) };
}

/** Whether a failed look is just a machine without an adapter. */
const noGpu = (e: FinError): boolean => {
  if (e.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) {
    console.warn("media.look: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead");
    return true;
  }
  return false;
};

/** The look's result, or `undefined` where there is no GPU. */
async function rendered(s: Setup, over: Partial<LookRequest> = {}): Promise<LookResult | undefined> {
  const exit = await Effect.runPromiseExit(s.look(over));
  if (Exit.isSuccess(exit)) return exit.value as LookResult;
  const e = (exit.cause.reasons.find((r) => r._tag === "Fail") as { error: FinError } | undefined)?.error;
  if (e !== undefined && noGpu(e)) return undefined;
  throw new Error(`look failed: ${JSON.stringify(e ?? exit.cause)}`);
}

describe("media.look", () => {
  test("renders three times of a project to workspace/shared/look", async () => {
    const s = await setup();
    const r = await rendered(s, { at: ["0", "1/10", "7/10"], size: { width: 80, height: 45 } });
    if (r === undefined) return;
    expect(r.frames.map((f) => f.at)).toEqual(["0", "1/10", "7/10"]);
    const dest = join(s.p.dir, "workspace", "shared", "look");
    for (const f of r.frames) {
      expect(dirname(f.file)).toBe(dest);
      expect([f.width, f.height]).toEqual([80, 45]);
      expect(f.plan).toMatch(/^[0-9a-f]{64}$/);
    }
    expect(readdirSync(dest).sort()).toEqual(r.frames.map((f) => basename(f.file)).sort());
    expect(r.adapter.backend).not.toBe("");
    // The second clip's file is missing (a2): its frame is drawn and what it left out is reported.
    expect(r.skipped.map((k) => k.code)).toContain("ORIGINALS_MISSING");
  });

  test("a time past the end is INVALID_RANGE, the end itself is a frame", async () => {
    const s = await setup();
    // The timeline is 4/5 s long.
    const past = await failure(s.look({ at: ["0", "9/10"] }));
    expect(past).toMatchObject({ code: "INVALID_RANGE", details: { field: "at", at: "9/10", duration: "4/5" } });
    expect((await failure(s.look({ at: ["-1"] }))).code).toBe("INVALID_RANGE");
    const end = await Effect.runPromiseExit(s.look({ at: ["4/5"], wait: false }));
    expect(Exit.isSuccess(end)).toBe(true);
  });

  test("an unknown timeline is NOT_FOUND, an unknown project PROJECT_NOT_FOUND", async () => {
    const s = await setup();
    expect(await failure(s.look({ timeline: "tl9" }))).toMatchObject({ code: "NOT_FOUND", details: { id: "tl9", kind: "timeline", known: ["tl1"] } });
    expect((await failure(s.look({ projectId: "nope" }))).code).toBe("PROJECT_NOT_FOUND");
  });

  test("the request is checked before any job: counts, sides and times", async () => {
    const s = await setup();
    const before = s.t.engine.list({}).jobs.length;
    expect((await failure(s.look({ at: [] }))).code).toBe("INVALID_RANGE");
    expect((await failure(s.look({ at: Array.from({ length: 65 }, () => "0") }))).code).toBe("INVALID_RANGE");
    for (const size of [{ width: 0, height: 9 }, { width: LOOK_MAX_SIDE + 1, height: 9 }, { width: 1.5, height: 9 }]) {
      expect(await failure(s.look({ size })), JSON.stringify(size)).toMatchObject({ code: "INVALID_RANGE", details: { field: "size" } });
    }
    for (const at of ["abc", "1.5", "1/0", ""]) expect((await failure(s.look({ at: [at] }))).code, at).toBe("INVALID_TIME");
    expect(s.t.engine.list({}).jobs.length).toBe(before);
  });

  test("times are sent on in lowest terms, so equal looks coalesce", async () => {
    const s = await setup();
    const a = (await Effect.runPromise(s.look({ at: ["2/4", "0/5"], wait: false }))) as { job: JobView };
    expect((a.job.params as { at: string[] }).at).toEqual(["1/2", "0"]);
    const b = (await Effect.runPromise(s.look({ at: ["1/2", "0"], wait: false }))) as { job: JobView };
    expect(b.job.id).toBe(a.job.id);
    await s.t.wait(a.job.id);
  });

  test("an out folder inside assets/ is EXPORT_DESTINATION and writes nothing", async () => {
    const s = await setup();
    const e = await Effect.runPromiseExit(s.look({ out: join(s.p.dir, "assets", "looks") }));
    expect(Exit.isFailure(e)).toBe(true);
    const err = (e as Exit.Failure<unknown, FinError>).cause.reasons.map((r) => (r._tag === "Fail" ? r.error : undefined)).find((x) => x !== undefined)!;
    if (noGpu(err)) return;
    expect(err.code).toBe("EXPORT_DESTINATION");
    expect(existsSync(join(s.p.dir, "assets", "looks"))).toBe(false);
  });

  test("wait: false answers {job} at once and the job ends with the same result", async () => {
    const s = await setup();
    const answer = (await Effect.runPromise(s.look({ at: ["0", "1/10"], size: { width: 40, height: 24 }, wait: false }))) as { job: JobView };
    expect(answer.job).toMatchObject({ kind: "media.look", family: "media", project: s.p.engine.projectId });
    const done = await s.t.wait(answer.job.id);
    if (done.state === "failed" && done.error?.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) return;
    expect(done.state).toBe("succeeded");
    // The same look again, waiting, is the same files (equal looks share their names).
    const again = (await rendered(s, { at: ["0", "1/10"], size: { width: 40, height: 24 } }))!;
    expect(again.frames.map((f) => f.file)).toEqual((done.result as LookResult).frames.map((f) => f.file));
  });

  test("the job carries the head it read, as ids", async () => {
    const s = await setup();
    const answer = (await Effect.runPromise(s.look({ wait: false }))) as { job: JobView };
    expect(answer.job.params).toMatchObject({ projectId: s.p.engine.projectId, branch: "main", rev: 1, timeline: "tl1", at: ["0"], outName: "look" });
    await s.t.wait(answer.job.id);
  });
});

describe("media.capabilities", () => {
  test("the worker answers codecs, the GPU and its versions; the answer is kept", async () => {
    const s = await setup();
    const caps = mediaCapabilities(s.deps);
    const first = await Effect.runPromise(caps(OWNER)).catch((e: FinError) => {
      if (noGpu(e)) return undefined;
      throw e;
    });
    if (first === undefined) return;
    const c: MediaCapabilities = first;
    expect(c.codecs.video.find((v) => v.codec === "av1")).toMatchObject({ decode: "software", encode: "software", backends: ["dav1d", "svt-av1"] });
    expect(c.codecs.video.find((v) => v.codec === "hevc")).toMatchObject({ decode: "none", encode: "none" });
    // No profile is applied here (the worker profile is not ready by default): `none` (6B-1 Task 18).
    expect(c.worker).toEqual({ version: expect.stringMatching(/^\d+\.\d+\.\d+/), renderVersion: 2, sandbox: "none" });
    expect(c.gpu === null || typeof c.gpu?.vendor === "number").toBe(true);
    const jobs = s.t.engine.list({}).jobs.filter((j) => j.kind === "media.capabilities").length;
    expect(await Effect.runPromise(caps(OWNER))).toEqual(first);
    expect(s.t.engine.list({}).jobs.filter((j) => j.kind === "media.capabilities").length).toBe(jobs);
  });

  test("a failed ask is not kept", async () => {
    let calls = 0;
    const failing = {
      submit: () => {
        calls++;
        return Effect.fail(toFinError(Object.assign(new Error("busy"), { finNode: true, code: "JOB_QUEUE_FULL" })));
      },
      wait: () => Effect.die("unreachable"),
    };
    const caps = mediaCapabilities({ engine: failing });
    expect((await failure(caps(OWNER))).code).toBe("JOB_QUEUE_FULL");
    expect((await failure(caps(OWNER))).code).toBe("JOB_QUEUE_FULL");
    expect(calls).toBe(2);
  });
});

describe("media diagnostics (Task 14)", () => {
  test("with diagnostics on, a look's job.done carries media and the worker's stats are forwarded", async () => {
    const s = await setup({ diagnostics: () => true });
    const r = await rendered(s, { at: ["0", "1/10"], size: { width: 80, height: 45 } });
    if (r === undefined) return;
    const done = await until(() => s.t.sink.done.find((d) => d.kind === "media.look"));
    const m = done.extra?.["media"] as Record<string, unknown>;
    expect(m).toMatchObject({ codec: "av1", backend: "dav1d", hw: false, resolution: "sd", frames: 2, copyPath: "copy" });
    expect(typeof m["rtFactorMilli"]).toBe("number");
    expect(typeof m["cacheHit"]).toBe("boolean");
    const stats = await until(() => (s.t.sink.forwarded.some((f) => f.kind === "render.stats") ? s.t.sink.forwarded.filter((f) => f.kind === "render.stats") : undefined));
    const phases = stats.map((f) => (f.payload as { phase: string }).phase).sort();
    expect(phases).toEqual(expect.arrayContaining(["composite", "decode", "readback"]));
    expect(stats.every((f) => f.origin.family === "media")).toBe(true);
    // Nothing a user wrote, and no adapter name, is in what the family sent.
    const sent = JSON.stringify([m, stats.map((f) => f.payload)]);
    expect(sent).not.toContain(s.p.dir);
    expect(sent).not.toContain(r.adapter.name);
  });

  test("media.capabilities is forwarded once per server run, with numeric GPU ids", async () => {
    const s = await setup({ diagnostics: () => true });
    const caps = mediaCapabilities(s.deps);
    const first = await Effect.runPromise(caps(OWNER)).catch((e: FinError) => {
      if (noGpu(e)) return undefined;
      throw e;
    });
    if (first === undefined) return;
    const sent = await until(() => (s.t.sink.forwarded.some((f) => f.kind === "media.capabilities") ? s.t.sink.forwarded.filter((f) => f.kind === "media.capabilities") : undefined));
    await Effect.runPromise(caps(OWNER));
    expect(s.t.sink.forwarded.filter((f) => f.kind === "media.capabilities")).toHaveLength(1);
    const event = sent[0]?.payload as { gpu?: Record<string, unknown>; sandbox: string; copyPath: string };
    expect(event).toMatchObject({ sandbox: "none", copyPath: "copy" });
    const gpu = first.gpu;
    if (gpu != null) {
      expect(event.gpu).toMatchObject({ vendorId: gpu.vendor, deviceId: gpu.device, software: gpu.software });
      expect(JSON.stringify(event)).not.toContain(gpu.name);
    }
  });

  test("with diagnostics off nothing media-specific is built or forwarded", async () => {
    const s = await setup();
    const r = await rendered(s, { size: { width: 80, height: 45 } });
    if (r === undefined) return;
    await Effect.runPromise(mediaCapabilities(s.deps)(OWNER));
    const done = await until(() => s.t.sink.done.find((d) => d.kind === "media.look"));
    expect(done.extra ?? {}).not.toHaveProperty("media");
    expect(s.t.sink.forwarded).toEqual([]);
    expect(r.stats.media).toBeUndefined();
  });
});
