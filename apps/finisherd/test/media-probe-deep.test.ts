// The `media.probeDeep` kind and RPC (6B-1 plan Task 11): the kind's shape (normal priority,
// `cpu`, cold, the batch memory rule, identity by fingerprint + render version + capability
// digest), `prepare` over a real project engine (the one-asset manifest, `ASSET_CHANGED`), the
// whole job through a real `fin-workers media` (dav1d decodes the AV1 fixture; no GPU needed),
// and the RPC's checks and params over a fake job engine.
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { ProbeDeepParams, ProbeDeepResult, RenderManifest } from "@finisher/core-types";
import { type Engine, resolveMemoryRule, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import { makeKindRegistry } from "../src/jobs/kinds.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_PROBE_DEEP, mediaProbeDeepKind, probeDeepDedupe } from "../src/jobs/kinds/media.probeDeep.ts";
import { BATCH_MEMORY, BATCH_MEMORY_LARGE } from "../src/media/memory.ts";
import { capsDigest, probeDeep } from "../src/media/probe-deep.ts";
import type { ProjectRegistryShape } from "../src/projects.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";
import { type LookProject, lookProject, makeAv1Fixture } from "./support/media.ts";

let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  work = mkdtempSync(join(tmpdir(), "fin-probe-deep-"));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const OWNER = { kind: "person", id: "p1" } as const;
const GiB = 1024 ** 3;

const params = (over: Partial<ProbeDeepParams> = {}): ProbeDeepParams => ({ asset: "a1", fingerprint: "fp-a1", ...over });

/** `onBranch` over one project's engine. */
function projectsOf(p: LookProject): Pick<ProjectRegistryShape, "onBranch" | "prepareMediaJob" | "dirOf"> {
  return {
    onBranch: <A>(projectId: string, branch: string, f: (e: Engine, b: string) => Promise<string>) =>
      projectId !== p.engine.projectId
        ? Effect.fail(toFinError(new Error("unknown project")))
        : Effect.tryPromise({ try: async () => JSON.parse(await f(p.engine, branch)) as A, catch: toFinError }),
    prepareMediaJob: () => Effect.die("unused"),
    dirOf: (projectId) => (projectId === p.engine.projectId ? p.dir : undefined),
  };
}

describe("the kind", () => {
  test("it is the plan's kind: normal priority, cpu, cold, batch memory", () => {
    const k = mediaProbeDeepKind({ serverData: "/sd", projects: { onBranch: () => Effect.die("unused") } });
    expect(k).toMatchObject({
      kind: MEDIA_PROBE_DEEP,
      version: 1,
      family: "media",
      scopes: ["project.read"],
      requiresProject: true,
      resources: ["cpu"],
      warm: false,
      defaultPriority: "normal",
      files: [{ name: "in/manifest.json", mode: "read" }],
    });
    const memory = k.memory as (p: ProbeDeepParams) => unknown;
    expect(memory(params())).toEqual(BATCH_MEMORY);
    expect(memory(params({ display: { width: 1920, height: 1080 } }))).toEqual(BATCH_MEMORY);
    expect(memory(params({ display: { width: 3840, height: 2160 } }))).toEqual(BATCH_MEMORY_LARGE);
    expect(resolveMemoryRule(BATCH_MEMORY_LARGE, 64 * GiB)).toBe(6 * GiB);
    // Registered with the media kinds when the registry can reach a project's engine.
    const r = makeKindRegistry();
    registerMediaKinds(r, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) } });
    expect(r.kind(MEDIA_PROBE_DEEP)?.warm).toBe(false);
  });

  test("dedupe is the project, the asset, the fingerprint, the render version and the capability digest", () => {
    const withProject = (over: Partial<ProbeDeepParams>) => params({ projectId: "p1", ...over });
    const a = probeDeepDedupe(withProject({ renderVersion: 1, capsDigest: "c1" }));
    expect(a).toBe(JSON.stringify(["media.probeDeep", "p1", "a1", "fp-a1", 1, "c1"]));
    // The result names its asset and prepare reads that project's manifest: another asset or
    // project with the same content does not coalesce, nor does another report or render version.
    expect(probeDeepDedupe(withProject({ asset: "a9", renderVersion: 1, capsDigest: "c1" }))).not.toBe(a);
    expect(probeDeepDedupe(withProject({ projectId: "p-other", renderVersion: 1, capsDigest: "c1" }))).not.toBe(a);
    expect(probeDeepDedupe(withProject({ renderVersion: 2, capsDigest: "c1" }))).not.toBe(a);
    expect(probeDeepDedupe(withProject({ renderVersion: 1, capsDigest: "c2" }))).not.toBe(a);
    expect(probeDeepDedupe(params())).toBeUndefined();
  });

  test("params are fin-media's ProbeDeepParams", () => {
    const ok = params({ display: { width: 1, height: 2 } });
    expect(validateJobParams(MEDIA_PROBE_DEEP, 1, ok)).toMatchObject({ ok: true, params: ok });
    expect(validateJobParams(MEDIA_PROBE_DEEP, 1, { asset: "a1", fingerprint: "f", path: "/etc/passwd" })).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PROBE_DEEP, 1, { asset: "a1" })).toMatchObject({ ok: false });
  });
});

describe("the job", () => {
  async function setup() {
    const serverData = mkdtempSync(join(work, "sd-"));
    const p = await lookProject(serverData, `${mkdtempSync(join(work, "proj-"))}/p`, media);
    engines.push(p.engine);
    const t = await testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects: projectsOf(p) }) });
    return { p, t, serverData };
  }

  test("probe_deep_reports_backend_and_class", async () => {
    const { p, t } = await setup();
    const v = await t.runJob(MEDIA_PROBE_DEEP, params({ projectId: p.engine.projectId, renderVersion: 1, capsDigest: "c" }), { project: p.engine.projectId });
    expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
    const r = v.result as ProbeDeepResult;
    expect(r).toMatchObject({ asset: "a1", fingerprint: "fp-a1", codec: "av1", decodable: true, backend: "dav1d", class: "software", hw: false });
    expect(r.firstFrame).toEqual({ width: 160, height: 90, format: "i420" });
    expect(r.hdr).toBeUndefined();
  }, 120_000);

  test("prepare writes the one-asset manifest and grants the worker that file only", async () => {
    const { p, serverData } = await setup();
    const jobsRoot = join(serverData, "jobs");
    mkdirSync(join(jobsRoot, "j1"), { recursive: true });
    const k = mediaProbeDeepKind({ serverData, jobsRoot, projects: projectsOf(p) });
    const job = { id: "j1", params: params({ projectId: p.engine.projectId }) };
    const out = (await Effect.runPromise(k.prepare!({ job, attempt: 1, open: () => Effect.die(0) } as never) as Effect.Effect<{ extra: unknown }, FinError>)).extra;
    expect(out).toEqual({ rev: 1, reads: [realpathSync(media)] });
    expect(k.reads!(out, job.params)).toEqual([realpathSync(media)]);
    const m = JSON.parse(readFileSync(join(jobsRoot, "j1", "in", "manifest.json"), "utf8")) as RenderManifest;
    expect(m.assets).toHaveLength(1);
    expect(m.assets[0]).toMatchObject({ asset: "a1", kind: "media", fingerprint: "fp-a1", original: { path: media } });
  }, 60_000);

  test("a fingerprint that changed is ASSET_CHANGED, before any worker runs", async () => {
    const { p, t, serverData } = await setup();
    const v = await t.runJob(MEDIA_PROBE_DEEP, params({ projectId: p.engine.projectId, fingerprint: "fp-old" }), { project: p.engine.projectId });
    expect(v.state).toBe("failed");
    expect(v.error).toMatchObject({ code: "ASSET_CHANGED", details: { asset: "a1", reason: "fingerprint" } });
    // `prepare` wrote the one-asset manifest it compared against.
    const m = JSON.parse(readFileSync(join(serverData, "jobs", v.id, "in", "manifest.json"), "utf8")) as RenderManifest;
    expect(m.assets.map((a) => [a.asset, a.fingerprint])).toEqual([["a1", "fp-a1"]]);
  }, 60_000);

  test("an unknown asset is ASSET_NOT_FOUND; a missing file is ORIGINALS_MISSING", async () => {
    const { p, t } = await setup();
    const unknown = await t.runJob(MEDIA_PROBE_DEEP, params({ projectId: p.engine.projectId, asset: "a404" }), { project: p.engine.projectId });
    expect(unknown.error?.code).toBe("ASSET_NOT_FOUND");
    const gone = await t.runJob(MEDIA_PROBE_DEEP, params({ projectId: p.engine.projectId, asset: "a2", fingerprint: "fp-a2" }), { project: p.engine.projectId });
    expect(gone.error).toMatchObject({ code: "ORIGINALS_MISSING", details: { asset: "a2", reason: "notFound" } });
  }, 120_000);
});

describe("the RPC", () => {
  const project = {
    rev: 3,
    project: {
      assets: {
        a1: { id: "a1", kind: "media", fingerprint: "fp-a1", video: { width: 3840, height: 2160, rate: "30" } },
        f1: { id: "f1", kind: "font", fingerprint: "fp-f" },
      },
    },
  };
  const report = { codecs: [], encoders: [], gpu: null, worker: { version: "0", renderVersion: 1 } };

  function deps() {
    const submitted: unknown[] = [];
    const d = {
      registry: {
        ensure: () => Effect.void,
        resolveBranch: (_: string, b: string) => Effect.succeed(b),
        snapshot: () => Effect.succeed(project),
      },
      engine: {
        submit: (_owner: unknown, req: unknown) => {
          submitted.push(req);
          return Effect.succeed({ id: "j1" } as JobView);
        },
        wait: () => Effect.succeed([{ id: "j1", state: "succeeded", result: { decodable: true } } as unknown as JobView]),
      },
      capabilities: () => Effect.succeed(report as unknown),
    };
    return { d, submitted };
  }

  test("it submits the asset's fingerprint, size, render version and report digest", async () => {
    const { d, submitted } = deps();
    const r = await Effect.runPromise(probeDeep(d as never, OWNER, { projectId: "p1", asset: "a1" }));
    expect(r).toEqual({ decodable: true });
    expect(submitted).toEqual([
      {
        kind: MEDIA_PROBE_DEEP,
        project: "p1",
        params: { asset: "a1", fingerprint: "fp-a1", projectId: "p1", branch: "main", display: { width: 3840, height: 2160 }, renderVersion: 1, capsDigest: capsDigest(report) },
      },
    ]);
    expect(capsDigest(report)).toMatch(/^[0-9a-f]{64}$/);
    expect(capsDigest({ ...report, worker: { ...report.worker, renderVersion: 2 } })).not.toBe(capsDigest(report));
  });

  test("an unknown or non-media asset is refused before any job", async () => {
    const { d, submitted } = deps();
    const fail = (asset: string) => Effect.runPromise(Effect.flip(probeDeep(d as never, OWNER, { projectId: "p1", asset }))) as Promise<FinError>;
    expect((await fail("a9")).code).toBe("ASSET_NOT_FOUND");
    expect((await fail("f1")).code).toBe("ASSET_TYPE_UNSUPPORTED");
    // An own-property check: a prototype name is not an asset.
    expect((await fail("constructor")).code).toBe("ASSET_NOT_FOUND");
    expect(submitted).toEqual([]);
  });
});
