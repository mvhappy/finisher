// The `media.look` kind (6A plan Task 12): its shape, its params, `prepare` and `onSucceeded`
// over a real project engine, and the whole job through a real `fin-workers media` (lavapipe
// with `FIN_GPU=software`; without any adapter the worker tests skip unless `FIN_REQUIRE_GPU=1`).
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { LookFrame, LookParams, LookResult } from "@finisher/core-types";
import { Engine, resolveMemoryRule, validateJobParams, workersBinaryPath } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { passEnv } from "../src/env.ts";
import { toFinError } from "../src/errors.ts";
import { makeKindRegistry } from "../src/jobs/kinds.ts";
import { finalName, lookDedupe, lookDestination, MEDIA_MEMORY, mediaLookKind, registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import type { ProjectRegistryShape } from "../src/projects.ts";
import { closeTestEngines, type TestEngine, testEngine, until } from "./support/jobs.ts";
import { type LookProject, lookProject, makeAv1Fixture } from "./support/media.ts";

const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = mkdtempSync(join(tmpdir(), "fin-media-kind-"));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const fail = (e: unknown): never => {
  throw e;
};

/** The registry reads the kind needs, over one project's engine. */
function projectsOf(p: LookProject): Pick<ProjectRegistryShape, "prepareMediaJob" | "dirOf"> {
  return {
    prepareMediaJob: (projectId, branch, timeline, at, inputDir, fontCache, paramsJson) =>
      projectId !== p.engine.projectId
        ? Effect.fail(toFinError(new Error("unknown project")))
        : Effect.tryPromise({
            try: async () => JSON.parse(await p.engine.prepareMediaJob(branch, timeline, [...at], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null })),
            catch: toFinError,
          }),
    dirOf: (projectId) => (projectId === p.engine.projectId ? p.dir : undefined),
  };
}

interface Setup {
  readonly p: LookProject;
  readonly t: TestEngine;
  readonly serverData: string;
  params(over?: Partial<LookParams>): LookParams;
  look(over?: Partial<LookParams>): Promise<JobView>;
}

async function setup(): Promise<Setup> {
  const serverData = mkdtempSync(join(work, "sd-"));
  const p = await lookProject(serverData, mkdtempSync(join(work, "proj-")) + "/p", media);
  engines.push(p.engine);
  const t = await testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects: projectsOf(p) }) });
  const params = (over: Partial<LookParams> = {}): LookParams => ({
    timeline: "tl1",
    at: ["0", "1/10"] as never,
    size: { width: 80, height: 45 },
    outName: "look",
    projectId: p.engine.projectId,
    branch: "main",
    rev: 1,
    ...over,
  });
  return { p, t, serverData, params, look: (over) => t.runJob("media.look", params(over), { project: p.engine.projectId }) };
}

/** Skips a rendering test where there is no GPU (unless required). */
function noGpu(v: JobView): boolean {
  if (v.state === "failed" && v.error?.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) {
    console.warn("media.look: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead");
    return true;
  }
  return false;
}

const png = (path: string): { width: number; height: number } => {
  const b = readFileSync(path);
  expect(b.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
  return { width: b.readUInt32BE(16), height: b.readUInt32BE(20) };
};

describe("the kind", () => {
  test("it is the plan's kind on the media family", () => {
    const k = mediaLookKind({ serverData: "/sd", projects: { prepareMediaJob: () => fail(0), dirOf: () => undefined } });
    expect(k).toMatchObject({
      kind: "media.look",
      version: 1,
      family: "media",
      scopes: ["project.read"],
      resources: [],
      warm: true,
      defaultPriority: "interactive",
      timeoutS: 60,
      stallS: 30,
      maxAttempts: 2,
      files: [
        { name: "in/snapshot.json", mode: "read" },
        { name: "in/manifest.json", mode: "read" },
        { name: "out/", mode: "write" },
      ],
    });
    const r = makeKindRegistry();
    registerMediaKinds(r, { serverData: "/sd", projects: { prepareMediaJob: () => fail(0), dirOf: () => undefined } });
    expect(r.family("media")).toMatchObject({ profile: "worker", env: ["FIN_GPU", "VK_ICD_FILENAMES"] });
    expect(r.program("media", { params: {} } as never, "warm")).toEqual({ binary: "fin-workers", args: ["media", "--warm"] });
    expect(r.program("media", { params: {} } as never, "cold")).toEqual({ binary: "fin-workers", args: ["media"] });
  });

  test("the media group's limit allows 8 sources of 64 frames", () => {
    const GiB = 1024 ** 3;
    // About 200 MB per 1080p source, 8 of them, with room for the GPU and the encoder.
    for (const ram of [8, 16, 32]) expect(resolveMemoryRule(MEDIA_MEMORY, ram * GiB)).toBeGreaterThanOrEqual(2 * GiB);
    expect(resolveMemoryRule(MEDIA_MEMORY, 128 * GiB)).toBe(6 * GiB);
  });

  // Plan 7C Task 7: the files the worker opens are persisted as `extra.reads` (links
  // resolved, missing ones left out) with the font cache, and `reads` answers them.
  test("prepare_persists_the_worker_reads_and_reads_answers_them", async () => {
    const sd = realpathSync(mkdtempSync(join(work, "reads-sd-")));
    mkdirSync(join(sd, "fonts"));
    const src = join(realpathSync(mkdtempSync(join(work, "reads-src-"))), "clip.mp4");
    writeFileSync(src, "c");
    const link = join(dirname(src), "link.mp4");
    symlinkSync(src, link);
    const jobsRoot = join(sd, "jobs");
    mkdirSync(join(jobsRoot, "j1"), { recursive: true });
    const k = mediaLookKind({
      serverData: sd,
      jobsRoot,
      projects: { prepareMediaJob: () => Effect.succeed({ rev: 3, planSamples: [], sources: [link, join(dirname(src), "gone.mp4"), "relative.mp4"] }), dirOf: () => undefined },
    });
    const params = { timeline: "tl1", at: ["0"], outName: "look", projectId: "p", branch: "main" };
    const out = (await Effect.runPromise(k.prepare!({ job: { id: "j1", params }, attempt: 1, open: () => fail(0) } as never) as Effect.Effect<{ extra: unknown }, FinError>)).extra;
    expect(out).toMatchObject({ rev: 3, reads: [src, join(sd, "fonts")].sort() });
    expect(k.reads!(out, params as never)).toEqual([src, join(sd, "fonts")].sort());
    expect(k.reads!(undefined, params as never)).toEqual([]);
  });

  test("its params are the generated LookParams", () => {
    const ok = { timeline: "tl1", at: ["0", "1/2"], outName: "look", projectId: "p", branch: "main", rev: 3, size: { width: 8, height: 8 }, outDir: "shots" };
    expect(validateJobParams("media.look", 1, ok)).toMatchObject({ ok: true, params: { ...ok, planSamples: [] } });
    for (const bad of [{ ...ok, surprise: 1 }, { ...ok, outName: "../x" }, { ...ok, at: [] }, { ...ok, rev: -1 }, { timeline: "tl1" }]) {
      expect(validateJobParams("media.look", 1, bad), JSON.stringify(bad)).toMatchObject({ ok: false });
    }
    const schema = JSON.parse(readFileSync(join(import.meta.dirname, "../../../packages/core-types/schema/fin-core.schema.json"), "utf8")) as {
      "$defs": Record<string, { properties: Record<string, unknown>; required?: string[] }>;
    };
    const look = schema.$defs.LookParams!;
    expect(Object.keys(look.properties).sort()).toEqual(["at", "branch", "outDir", "outName", "planSamples", "projectId", "rev", "size", "timeline"]);
    expect([...look.required!].sort()).toEqual(["at", "outName", "timeline"]);
  });

  test("equal looks share a dedupe key; a different rev, time, size or place does not", () => {
    const base: LookParams = { timeline: "tl1", at: ["0"] as never, outName: "look", projectId: "p", branch: "main", rev: 1 };
    const key = lookDedupe(base);
    expect(key).toBeDefined();
    expect(lookDedupe({ ...base })).toBe(key);
    for (const other of [{ rev: 2 }, { at: ["1"] }, { size: { width: 8, height: 8 } }, { timeline: "tl2" }, { branch: "b" }, { projectId: "q" }, { outDir: "x" }, { outName: "other" }]) {
      expect(lookDedupe({ ...base, ...other } as LookParams)).not.toBe(key);
    }
    expect(lookDedupe({ timeline: "tl1", at: ["0"] as never, outName: "look" })).toBeUndefined();
  });

  test("frames are named for their timeline, time and plan", () => {
    const h = (c: string) => c.repeat(64);
    const hd = { width: 1920, height: 1080 };
    expect(finalName("tl1", "37/5", hd, "0123456789abcdef".repeat(4))).toBe("tl1-37_5-1920x1080-01234567.png");
    expect(finalName("tl1", "12", hd, h("f"))).toBe("tl1-12_1-1920x1080-ffffffff.png");
    expect(finalName("a b/../c", "0", { width: 8, height: 8 }, h("a"))).toBe("a_b____c-0_1-8x8-aaaaaaaa.png");
    // The plan hash does not cover the size, so the name must: same time and plan, two sizes.
    expect(finalName("tl1", "0", hd, h("a"))).not.toBe(finalName("tl1", "0", { width: 1280, height: 720 }, h("a")));
    // Review B2: neither the time nor the hash may carry a path, whatever called this.
    for (const at of ["..\\..\\x", "/../x", "1/../2", "a", "", "1/2/3"]) expect(() => finalName("tl1", at, hd, h("a")), at).toThrow();
    for (const plan of ["../../x", "abc", h("g"), `${h("a")}/`, h("A")]) expect(() => finalName("tl1", "0", hd, plan), plan).toThrow();
    for (const [width, height] of [[0, 8], [8, 0], [-1, 8], [8.5, 8], [8193, 8], [8, Number.NaN]] as const) expect(() => finalName("tl1", "0", { width, height }, h("a")), `${width}x${height}`).toThrow();
  });
});

describe("destinations", () => {
  let dir: string;
  beforeAll(() => {
    dir = realpathSync(mkdtempSync(join(work, "dest-"))); // destinations answer resolved (macOS: `/var` is a link)
    mkdirSync(join(dir, ".finisher"));
    mkdirSync(join(dir, "assets"));
  });

  test("the default is workspace/shared/look; an own folder is made", async () => {
    expect(await lookDestination(dir, undefined)).toBe(join(dir, "workspace", "shared", "look"));
    expect(existsSync(join(dir, "workspace", "shared", "look"))).toBe(true);
    expect(await lookDestination(dir, "shots/a")).toBe(join(dir, "shots", "a"));
    const outside = realpathSync(mkdtempSync(join(work, "outside-")));
    expect(await lookDestination(dir, outside)).toBe(outside);
  });

  test("inside .finisher/ or assets/ is EXPORT_DESTINATION, however it is spelled", async () => {
    const code = async (out: string): Promise<string> => ((await lookDestination(dir, out).catch((e: FinError) => e)) as FinError).code;
    for (const out of [".finisher", ".finisher/x/y", "assets", "assets/frames", join(dir, "assets", "z"), "./assets/../assets/q", "shots/../assets/x"]) {
      expect(await code(out), out).toBe("EXPORT_DESTINATION");
    }
    expect(existsSync(join(dir, "assets", "frames"))).toBe(false);
    if (process.platform !== "win32") {
      symlinkSync(join(dir, "assets"), join(dir, "sneaky"));
      expect(await code("sneaky/inner")).toBe("EXPORT_DESTINATION");
    }
    // Not writable: a file where a folder belongs.
    writeFileSync(join(dir, "afile"), "x");
    expect(await code("afile/sub")).toBe("EXPORT_DESTINATION");
  });
});

describe("destinations, by identity (review M1)", () => {
  let dir: string;
  beforeAll(() => {
    dir = realpathSync(mkdtempSync(join(work, "dest2-")));
    mkdirSync(join(dir, ".finisher"));
    mkdirSync(join(dir, "assets"));
  });

  const code = async (out: string, protect: string[] = []): Promise<string> => ((await lookDestination(dir, out, protect).catch((e: FinError) => e)) as FinError).code;

  test("a folder named .finisher is refused at any depth and in any case, and nothing is made", async () => {
    for (const out of [".FINISHER/x", "a/.Finisher/b", ".finisher", join(dir, "sub", ".FiniSher")]) {
      expect(await code(out), out).toBe("EXPORT_DESTINATION");
    }
    // By name, not `existsSync`: the test's own `.finisher` answers to `.FINISHER` on a
    // case-insensitive volume (NTFS, APFS by default).
    expect(readdirSync(dir).sort()).toEqual([".finisher", "assets"]);
  });

  test.skipIf(process.platform === "win32")("a link to a protected folder or to any folder inside it is refused by identity", async () => {
    mkdirSync(join(dir, "assets", "deep"));
    symlinkSync(join(dir, "assets", "deep"), join(dir, "to-deep"));
    symlinkSync(join(dir, "assets"), join(dir, "to-assets"));
    symlinkSync(join(dir, ".finisher"), join(dir, "to-finisher"));
    for (const out of ["to-deep/x", "to-assets", "to-assets/y/z", "to-finisher/q"]) expect(await code(out), out).toBe("EXPORT_DESTINATION");
    expect(existsSync(join(dir, "assets", "y"))).toBe(false);
    // A folder with the same name elsewhere is not the protected one.
    mkdirSync(join(dir, "other", "assets"), { recursive: true });
    expect(await code("other/assets")).not.toBe("EXPORT_DESTINATION");
  });

  test("the server's data and everything inside it is refused", async () => {
    const sd = realpathSync(mkdtempSync(join(work, "sd-protected-")));
    mkdirSync(join(sd, "jobs"));
    expect(await code(join(sd, "looks"), [sd])).toBe("EXPORT_DESTINATION");
    expect(await code(join(sd, "jobs", "x"), [sd])).toBe("EXPORT_DESTINATION");
    expect(await code(sd, [sd])).toBe("EXPORT_DESTINATION");
    if (process.platform !== "win32") {
      symlinkSync(sd, join(dir, "to-sd"));
      expect(await code("to-sd/looks", [sd])).toBe("EXPORT_DESTINATION");
    }
    expect(existsSync(join(sd, "looks"))).toBe(false);
    // Unprotected, the same folder is just a folder.
    expect(await code(join(sd, "looks"))).not.toBe("EXPORT_DESTINATION");
  });

  // UNC paths are Windows'; here their spellings stay harmless (Windows rows: Task 16).
  test("UNC spellings are a folder in the project or an unwritable path, never a way around", async () => {
    expect(await code("//no-such-server/share/looks")).toBe("EXPORT_DESTINATION");
    const dest = await lookDestination(dir, "\\\\no-such-server\\share\\looks").catch((e: FinError) => e);
    if (process.platform !== "win32") expect(typeof dest === "string" ? dirname(dest) : "").toBe(dir);
  });
});

describe("onSucceeded believes the host, not the worker (review B2)", () => {
  const h = (c: string): string => c.repeat(64);

  /** A kind over a fake job directory holding `out/look-0.png`, and the hook's ctx. */
  function fixture(over: { at?: string[]; frames?: Array<Partial<LookFrame>>; samples?: string[]; size?: { width: number; height: number } } = {}) {
    const dir = realpathSync(mkdtempSync(join(work, "b2-proj-")));
    const jobsRoot = realpathSync(mkdtempSync(join(work, "b2-jobs-")));
    mkdirSync(join(jobsRoot, "j1", "out"), { recursive: true });
    writeFileSync(join(jobsRoot, "j1", "out", "look-0.png"), "png");
    const k = mediaLookKind({ serverData: join(work, "sd-none"), jobsRoot, projects: { prepareMediaJob: () => fail(0), dirOf: () => dir } });
    const at = over.at ?? ["0"];
    const params = { timeline: "tl1", at, outName: "look", projectId: "p", branch: "main", rev: 1, planSamples: [], ...(over.size ? { size: over.size } : {}) };
    const frames = (over.frames ?? [{}]).map((f) => ({ at: "0", file: "look-0.png", width: 8, height: 8, plan: h("a"), ...f }));
    const extra = { planSamples: (over.samples ?? [h("a")]).map((blake3, i) => ({ at: at[i] ?? "0", blake3 })), rev: 1 };
    const ctx = { job: { id: "j1", params }, result: { frames, skipped: [], adapter: {}, stats: {} }, extra, items: new Map(), rerun: false, attempt: 1, open: () => fail(0) };
    const run = () => Effect.runPromise(Effect.flip(k.onSucceeded!(ctx as never) as Effect.Effect<LookResult, FinError>));
    const ok = () => Effect.runPromise(k.onSucceeded!(ctx as never) as Effect.Effect<LookResult, FinError>);
    return { dir, jobsRoot, run, ok, look: join(dir, "workspace", "shared", "look") };
  }

  test("a hostile time in the params builds no path and moves nothing", async () => {
    for (const at of ["..\\..\\x", "/../x", "1/../2"]) {
      const f = fixture({ at: [at] });
      expect(await f.run(), at).toMatchObject({ code: "JOB_FAILED" });
      expect(readdirSync(f.look), at).toEqual([]);
      expect(existsSync(join(f.jobsRoot, "j1", "out", "look-0.png"))).toBe(true);
    }
  });

  test("a hostile time in the worker's answer is ignored: the name comes from the params", async () => {
    const f = fixture({ at: ["7/2"], frames: [{ at: "..\\..\\x" }] });
    const r = await f.ok();
    expect(r.frames).toHaveLength(1);
    expect(r.frames[0]).toMatchObject({ at: "7/2", file: join(f.look, `tl1-7_2-8x8-${h("a").slice(0, 8)}.png`) });
    expect(readdirSync(f.look)).toEqual([`tl1-7_2-8x8-aaaaaaaa.png`]);
  });

  test("a frame of another size than the named one fails the job; an unnamed size is the frame's own", async () => {
    const named = fixture();
    // The fixture's params name no size: the frame's own is used (here 8 × 8).
    expect((await named.ok()).frames[0]!.file).toBe(join(named.look, "tl1-0_1-8x8-aaaaaaaa.png"));
    for (const frames of [[{ width: 9 }], [{ height: 9 }], [{ width: 0, height: 0 }]]) {
      const f = fixture({ size: { width: 8, height: 8 }, frames });
      expect(await f.run(), JSON.stringify(frames)).toMatchObject({ code: "JOB_FAILED" });
      expect(readdirSync(f.look), JSON.stringify(frames)).toEqual([]);
    }
    const f = fixture({ size: { width: 16, height: 9 }, frames: [{ width: 16, height: 9 }] });
    expect((await f.ok()).frames[0]!.file).toBe(join(f.look, "tl1-0_1-16x9-aaaaaaaa.png"));
    // Not a pixel count, whatever the worker says: nothing is moved.
    const odd = fixture({ frames: [{ width: 1e9 }] });
    expect(await odd.run()).toMatchObject({ code: "JOB_FAILED" });
    expect(readdirSync(odd.look)).toEqual([]);
  });

  test("a plan hash that is not the one the host sampled, or not a digest, fails the job", async () => {
    for (const plan of ["../../x", "/../x", "aaaa", h("A"), `${h("a")}/`, h("b")]) {
      const f = fixture({ frames: [{ plan }] });
      expect(await f.run(), plan).toMatchObject({ code: "PLAN_MISMATCH" });
      expect(readdirSync(f.look), plan).toEqual([]);
    }
    // No samples at all (the job lost its extra): not trusted either.
    const lost = fixture({ samples: [] });
    expect(await lost.run()).toMatchObject({ code: "PLAN_MISMATCH" });
  });

  test("a file name that is not the worker's own scheme, or is out of order, fails the job", async () => {
    for (const file of ["../look-0.png", "look-0.png/../../x", "look-1.png", "look-0.png.png", "other-0.png", "look--1.png", "look-9007199254740993.png"]) {
      const f = fixture({ frames: [{ file }] });
      expect(await f.run(), file).toMatchObject({ code: "JOB_FAILED" });
      expect(readdirSync(f.look), file).toEqual([]);
    }
  });
});

describe("media.look jobs", () => {
  test("renders frames to workspace/shared/look under their final names", async () => {
    const s = await setup();
    const v = await s.look();
    if (noGpu(v)) return;
    expect(v.error).toBeUndefined();
    expect(v.state).toBe("succeeded");
    const result = v.result as LookResult;
    expect(result.frames).toHaveLength(2);
    const dest = join(s.p.dir, "workspace", "shared", "look");
    expect(readdirSync(dest).sort()).toEqual(result.frames.map((f) => basename(f.file)).sort());
    result.frames.forEach((f: LookFrame, i) => {
      expect(dirname(f.file)).toBe(dest);
      expect(basename(f.file)).toBe(`tl1-${i === 0 ? "0_1" : "1_10"}-80x45-${f.plan.slice(0, 8)}.png`);
      expect({ ...png(f.file) }).toEqual({ width: 80, height: 45 });
    });
    // `out/` was emptied by the moves, and 5C deleted the job directory after the hook.
    await until(() => !existsSync(join(s.serverData, "jobs", v.id)));
    // The plan hashes the worker checked are the ones `prepare` computed from the snapshot.
    expect(result.frames[0]!.plan).not.toBe(result.frames[1]!.plan);
  });

  test("two looks of the same time at different sizes into one folder keep both files", async () => {
    const s = await setup();
    const small = await s.look({ at: ["0"] as never, size: { width: 80, height: 45 } });
    if (noGpu(small)) return;
    const large = await s.look({ at: ["0"] as never, size: { width: 160, height: 90 } });
    expect(small.state).toBe("succeeded");
    expect(large.state).toBe("succeeded");
    const a = (small.result as LookResult).frames[0]!;
    const b = (large.result as LookResult).frames[0]!;
    // The plan (and its hash) does not depend on the size; the names must.
    expect(a.plan).toBe(b.plan);
    expect(a.file).not.toBe(b.file);
    expect(basename(a.file)).toBe(`tl1-0_1-80x45-${a.plan.slice(0, 8)}.png`);
    expect(basename(b.file)).toBe(`tl1-0_1-160x90-${b.plan.slice(0, 8)}.png`);
    expect(readdirSync(join(s.p.dir, "workspace", "shared", "look")).sort()).toEqual([basename(b.file), basename(a.file)].sort());
    expect(png(a.file)).toEqual({ width: 80, height: 45 });
    expect(png(b.file)).toEqual({ width: 160, height: 90 });
  });

  test("the second look of the same state coalesces and reuses its files", async () => {
    const s = await setup();
    const a = await s.t.submit("media.look", s.params(), { project: s.p.engine.projectId });
    const b = await s.t.submit("media.look", s.params(), { project: s.p.engine.projectId });
    expect(b.id).toBe(a.id);
    const done = await s.t.wait(a.id);
    if (noGpu(done)) return;
    expect(done.state).toBe("succeeded");
    const first = (done.result as LookResult).frames.map((f) => f.file);
    // Again, after it ended: a new job, the same file names (overwritten in place).
    const again = await s.look();
    expect((again.result as LookResult).frames.map((f) => f.file)).toEqual(first);
    expect(again.id).not.toBe(a.id);
  });

  test("a missing original is skipped, its frame still drawn and moved", async () => {
    const s = await setup();
    const v = await s.look({ at: ["3/5"] as never });
    if (noGpu(v)) return;
    expect(v.state).toBe("succeeded");
    const r = v.result as LookResult;
    expect(r.skipped.map((k) => k.code)).toEqual(["ORIGINALS_MISSING"]);
    expect(png(r.frames[0]!.file)).toEqual({ width: 80, height: 45 });
  });

  test("--out names a folder of the project, or anywhere writable", async () => {
    const s = await setup();
    const v = await s.look({ at: ["0"] as never, outDir: "shots" });
    if (noGpu(v)) return;
    expect(dirname((v.result as LookResult).frames[0]!.file)).toBe(join(s.p.dir, "shots"));
    const elsewhere = mkdtempSync(join(work, "elsewhere-"));
    const w = await s.look({ at: ["0"] as never, outDir: elsewhere });
    expect(dirname((w.result as LookResult).frames[0]!.file)).toBe(elsewhere);
  });

  test("a destination inside assets/ fails the job with EXPORT_DESTINATION and keeps the frames out", async () => {
    const s = await setup();
    const v = await s.look({ at: ["0"] as never, outDir: "assets/looks" });
    if (noGpu(v)) return;
    expect(v).toMatchObject({ state: "failed", error: { code: "EXPORT_DESTINATION" } });
    expect(existsSync(join(s.p.dir, "assets", "looks"))).toBe(false);
  });

  test("a failure in prepare ends the job without a worker", async () => {
    const s = await setup();
    const v = await s.look({ timeline: "tl9" });
    expect(v).toMatchObject({ state: "failed", attempts: 1, error: { code: "NOT_FOUND" } });
    expect(s.t.engine.workers().warm.every((w) => w.served === 0)).toBe(true);
    const w = await s.t.runJob("media.look", { ...s.params(), projectId: "nope" }, {});
    expect(w).toMatchObject({ state: "failed", error: { code: "INTERNAL" } });
    const x = await s.t.runJob("media.look", { timeline: "tl1", at: ["0"], outName: "look" }, {});
    expect(x).toMatchObject({ state: "failed", error: { code: "JOB_PARAMS_INVALID" } });
  });

  test("a rerun of onSucceeded after the files moved finishes the job", async () => {
    const s = await setup();
    const v = await s.look();
    if (noGpu(v)) return;
    const result = v.result as LookResult;
    const k = mediaLookKind({ serverData: s.serverData, projects: projectsOf(s.p) });
    // As the engine passes it: the worker's `out/`-relative names, the job gone from `jobs/`.
    const worker: LookResult = { ...result, frames: result.frames.map((f, i) => ({ ...f, file: `look-${i}.png` })) };
    const extra = { planSamples: result.frames.map((f) => ({ at: f.at, blake3: f.plan })), rev: 1 };
    const ctx = { job: { id: "j-gone", params: s.params() }, result: worker, extra, items: new Map(), rerun: true, attempt: 1, open: () => fail(0) };
    const again = await Effect.runPromise(k.onSucceeded!(ctx as never) as Effect.Effect<LookResult, FinError>);
    expect(again.frames.map((f) => f.file)).toEqual(result.frames.map((f) => f.file));
    // Not a rerun: a missing output is a failure, SHM_REFUSED with a job-relative file.
    const first = await Effect.runPromise(Effect.flip(k.onSucceeded!({ ...ctx, rerun: false } as never) as Effect.Effect<LookResult, FinError>));
    expect(first).toMatchObject({ code: "SHM_REFUSED", details: { reason: "missing", file: "j-gone/out/look-0.png".replace("j-gone/", "") } });
  });
});

describe("the worker's environment", () => {
  test("only the family's listed variables pass beyond the allow-list", () => {
    const parent = { FIN_GPU: "software", FIN_SECRET: "x", PATH: "/bin", ELECTRON_RUN_AS_NODE: "1" };
    expect(passEnv(["FIN_GPU"], parent)).toEqual({ FIN_GPU: "software" });
    expect(passEnv(["FIN_GPU", "FIN_UNSET"], {})).toEqual({});
    expect(passEnv(["ELECTRON_RUN_AS_NODE"], parent)).toEqual({});
    expect(passEnv(undefined, parent)).toEqual({});
  });

  test("FIN_GPU reaches the worker: an adapter nobody has fails the look", async () => {
    const s = await setup();
    const was = process.env.FIN_GPU;
    process.env.FIN_GPU = "no-such-backend";
    try {
      const v = await s.look({ at: ["0"] as never });
      expect(v).toMatchObject({ state: "failed" });
      expect(v.error?.code).toMatch(/^GPU_|^INVALID/);
    } finally {
      process.env.FIN_GPU = was;
    }
  });
});

describe("replay", () => {
  test("a job directory prepare filled runs without a host, with the same plans", async () => {
    const s = await setup();
    const jobDir = mkdtempSync(join(work, "replay-"));
    for (const d of ["in", "out", "tmp"]) mkdirSync(join(jobDir, d));
    const params = s.params({ at: ["0", "3/5"] as never });
    const done = JSON.parse(await s.p.engine.prepareMediaJob("main", "tl1", ["0", "3/5"], join(jobDir, "in"), { fontCache: join(s.serverData, "fonts"), paramsJson: JSON.stringify(params) })) as {
      planSamples: { at: string; blake3: string }[];
    };
    const run = spawnSync(workersBinaryPath(), ["media", "--replay", jobDir, "--originals-anywhere"], { env: { FIN_GPU: process.env.FIN_GPU ?? "software" }, encoding: "utf8" });
    const answer = JSON.parse(run.stdout) as { result?: LookResult; error?: { code: string } };
    if (answer.error?.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) return;
    expect(answer.error).toBeUndefined();
    expect(run.status).toBe(0);
    expect(answer.result!.frames.map((f) => f.plan)).toEqual(done.planSamples.map((p) => p.blake3));
    expect(readdirSync(join(jobDir, "out")).sort()).toEqual(["look-0.png", "look-1.png"]);
  });
});
