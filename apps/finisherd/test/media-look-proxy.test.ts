// `media.look` from proxies (6B-1 plan Task 16), through a real `fin-workers media`, the media
// cache and a real proxy made by `media.proxy`: the manifest lists the `1080` proxy the cache
// holds, pinned for the job; a frame says where it was read from and is named for it; `originals`
// reads originals only and pins nothing; a proxy gone between the manifest and the open falls
// back to the original, naming no path; the job's pins end with the job (the wiring's hook).
// The worker draws on the GPU (`FIN_GPU`; skipped without an adapter unless `FIN_REQUIRE_GPU=1`).
import { mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { LookParams, LookResult } from "@finisher/core-types";
import { Engine, MediaCache } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { toFinError } from "../src/errors.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { mediaCapabilities } from "../src/media/caps.ts";
import type { MediaCacheService } from "../src/media/cache.ts";
import { makeProxyService } from "../src/media/proxy.ts";
import { mediaWiring } from "../src/media/wiring.ts";
import type { ProjectRegistryShape } from "../src/projects.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";
import { lookProject, makeAv1Fixture } from "./support/media.ts";

const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
const OWNER = { kind: "local", id: "local" } as const;

let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-look-proxy-")));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media, 12);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);

/** A project (asset `a1` on the clip, fingerprint `fp-a1`), the cache, the media kinds, a proxy service. */
async function setup() {
  const serverData = realpathSync(mkdtempSync(join(work, "sd-")));
  const p = await lookProject(serverData, join(realpathSync(mkdtempSync(join(work, "proj-"))), "p"), media);
  engines.push(p.engine);
  const cache = await MediaCache.open(serverData);
  let caps: ReturnType<typeof mediaCapabilities> | undefined;
  const onBranch = <A>(projectId: string, branch: string, f: (e: Engine, b: string) => Promise<string>) =>
    projectId !== p.engine.projectId ? Effect.fail(toFinError(new Error("unknown project"))) : Effect.tryPromise({ try: async () => JSON.parse(await f(p.engine, branch)) as A, catch: toFinError });
  const projects: Pick<ProjectRegistryShape, "prepareMediaJob" | "dirOf" | "onBranch"> = {
    onBranch,
    prepareMediaJob: (projectId, branch, timeline, at, inputDir, fontCache, paramsJson) =>
      projectId !== p.engine.projectId
        ? Effect.fail(toFinError(new Error("unknown project")))
        : Effect.tryPromise({ try: async () => JSON.parse(await p.engine.prepareMediaJob(branch, timeline, [...at], inputDir, { fontCache: fontCache ?? null, paramsJson: paramsJson ?? null })), catch: toFinError }),
    dirOf: (projectId) => (projectId === p.engine.projectId ? p.dir : undefined),
  };
  const t = await testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects, proxy: { cache: () => cache, report: () => caps?.current() } }) });
  caps = mediaCapabilities({ engine: t.engine });
  const registry = {
    ensure: () => Effect.void,
    resolveBranch: (_: string, b: string) => Effect.succeed(b),
    snapshot: (_: string, b: string) => Effect.tryPromise({ try: async () => JSON.parse(await p.engine.snapshot(b)) as { rev: number; project: unknown }, catch: toFinError }),
    dirOf: projects.dirOf,
  };
  const proxies = makeProxyService({ registry, engine: t.engine, capabilities: caps, cache: () => cache });
  const params = (over: Partial<LookParams> = {}): LookParams => ({ timeline: "tl1", at: ["0", "1/10"] as never, size: { width: 80, height: 45 }, outName: "look", projectId: p.engine.projectId, branch: "main", rev: 1, ...over });
  const look = (over?: Partial<LookParams>) => t.runJob("media.look", params(over), { project: p.engine.projectId });
  return { p, t, cache, proxies, serverData, look, params, pid: p.engine.projectId, projects, registry };
}

/** The look's result, or `undefined` (the test stops) when this host has no GPU adapter. */
function resultOf(v: JobView): LookResult | undefined {
  if (v.state === "failed" && v.error?.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) {
    console.warn("media.look: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead");
    return undefined;
  }
  expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
  return v.result as LookResult;
}

/** Makes the `1080` proxy of `a1`; the entry's key, or `undefined` without a GPU. */
async function proxied(s: Awaited<ReturnType<typeof setup>>): Promise<string | undefined> {
  const made = await Effect.runPromise(Effect.result(s.proxies.request(OWNER, { projectId: s.pid, asset: "a1", tier: "1080" })));
  if (made._tag === "Failure") {
    if (made.failure.code === "GPU_UNAVAILABLE" && !GPU_REQUIRED) return undefined;
    throw made.failure;
  }
  return made.success.key;
}

describe("a look reads the proxy the cache holds", () => {
  test("look_from_proxy: the frames say so, are named for it, and the proxy is pinned for the job", async () => {
    const s = await setup();
    const key = await proxied(s);
    if (key === undefined) return;
    const dest = join(s.p.dir, "workspace", "shared", "look");
    const originalsOnly = resultOf(await s.look({ originals: true }));
    const original = originalsOnly?.frames[0];
    if (original === undefined) return;
    expect(original.source).toBe("original");
    expect(basename(original.file)).not.toContain("-proxy");
    const v = await s.look();
    const res = resultOf(v);
    expect(res).toBeDefined();
    expect(res!.frames.map((f) => f.source)).toEqual(["proxy", "proxy"]);
    expect(res!.skipped).toEqual([]);
    for (const [i, f] of res!.frames.entries()) {
      expect(f).toMatchObject({ width: 80, height: 45 });
      expect(f.proxyFallbacks ?? []).toEqual([]);
      // A frame from the proxy never replaces the original's file of the same time.
      expect(dirname(f.file)).toBe(dest);
      expect(basename(f.file)).toMatch(/-proxy\.png$/);
      expect(f.file).not.toBe(originalsOnly!.frames[i]!.file);
      expect(readFileSync(f.file).subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
    }
    // The same plan either way.
    expect(res!.frames.map((f) => f.plan)).toEqual(originalsOnly!.frames.map((f) => f.plan));
    // Pinned for the job (no wiring here to release it) and for nothing else.
    expect(await s.cache.pinsOf(key)).toEqual([`job:${v.id}`]);
  }, 180_000);

  test("originals_flag_bypasses_proxies: nothing is listed and nothing pinned", async () => {
    const s = await setup();
    const key = await proxied(s);
    if (key === undefined) return;
    const v = await s.look({ originals: true });
    const res = resultOf(v);
    if (res === undefined) return;
    expect(res.frames.map((f) => f.source)).toEqual(["original", "original"]);
    expect(await s.cache.pinsOf(key)).toEqual([]);
    // Originals and proxies are different looks: they do not coalesce.
    const a = await s.t.submit("media.look", s.params({ originals: true }), { project: s.pid });
    const b = await s.t.submit("media.look", s.params(), { project: s.pid });
    expect(a.id).not.toBe(b.id);
    await s.t.wait(a.id);
    await s.t.wait(b.id);
  }, 180_000);

  test("a look with no proxy in the cache reads the original, and says so", async () => {
    const s = await setup();
    const res = resultOf(await s.look());
    if (res === undefined) return;
    expect(res.frames.map((f) => f.source)).toEqual(["original", "original"]);
    expect(res.frames.every((f) => (f.proxyFallbacks ?? []).length === 0)).toBe(true);
  }, 180_000);

  test("the job's pins end with the job (the wiring's hook)", async () => {
    const s = await setup();
    const key = await proxied(s);
    if (key === undefined) return;
    const wiring = mediaWiring({ cache: s.cache } as unknown as MediaCacheService, { onDrop: () => {} }, () => {});
    wiring.start(s.t.engine, { registerStage: () => {}, registry: { list: () => [] } } as never, s.registry as never);
    const v = await s.look();
    if (resultOf(v) === undefined) return;
    await vi.waitFor(async () => expect(await s.cache.pinsOf(key)).toEqual([]), { timeout: 10_000 });
  }, 180_000);

  test("a proxy that was evicted between the manifest and the open: the original, no path in the answer", async () => {
    const s = await setup();
    const key = await proxied(s);
    if (key === undefined) return;
    const dir = s.cache.entryDir(key)!;
    // A job that lists the proxy (prepare pinned it), then the directory goes before the worker opens it.
    const slow = await s.t.submit("media.look", s.params(), { project: s.pid });
    rmSync(dir, { recursive: true, force: true });
    const v = await s.t.wait(slow.id);
    const res = resultOf(v);
    if (res === undefined) return;
    // Either the entry was gone before `prepare` (no proxy listed) or after it (listed, missing):
    // in both the look is drawn from the original and the answer names no path.
    expect(res.frames.map((f) => f.source)).toEqual(["original", "original"]);
    for (const f of res.frames) for (const fb of f.proxyFallbacks ?? []) expect(fb).toEqual({ asset: "a1", reason: "missing" });
    expect(JSON.stringify(res)).not.toContain(dir);
  }, 180_000);
});
