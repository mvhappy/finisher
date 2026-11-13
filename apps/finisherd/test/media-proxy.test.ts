// `media.proxy` (6B-1 plan Task 14): the kind's shape (background, cold, resources and codec by
// the capability report, the cache key as its identity, `rememberPoison`), the whole job through a
// real `fin-workers media` and the media cache (the RPC's answer, a hit on the second ask, the
// status of each tier, `PROXY_UNAVAILABLE` while one is made), a library file through the
// `proxy540` stage's params, the first-use `1080` proxy, and the typed refusals. The worker scales
// on the GPU (`FIN_GPU`; skipped without an adapter unless `FIN_REQUIRE_GPU=1`).
import { mkdtempSync, realpathSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { EntryMeta, ProxyParams, ProxyResult } from "@finisher/core-types";
import { Engine, MediaCache, mediaProxyPlan, mediaVersions, resolveMemoryRule, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import { makeKindRegistry } from "../src/jobs/kinds.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_PROXY, decodersOf, hardwareEncoder, mediaProxyKind, proxyCodec, proxyDedupe, proxyResources } from "../src/jobs/kinds/media.proxy.ts";
import { mediaCapabilities } from "../src/media/caps.ts";
import { BATCH_MEMORY, BATCH_MEMORY_LARGE } from "../src/media/memory.ts";
import { MEDIA_SYSTEM_OWNER, makeProxyService, proxyUnavailable } from "../src/media/proxy.ts";
import { PROXY540_STAGE, proxy540Stage } from "../src/media/stages.ts";
import type { StageFile } from "../src/library/api.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";
import { makeAv1Fixture } from "./support/media.ts";

const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
const OWNER = { kind: "local", id: "local" } as const;
const GiB = 1024 ** 3;

let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-proxy-")));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media, 12);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

/** A v1 report: dav1d decodes AV1, SVT-AV1 encodes it (no hardware). */
const V1 = { codecs: { video: [{ codec: "av1", decode: "software", encode: "software", backends: ["dav1d", "svt-av1"] }, { codec: "h264", decode: "none", encode: "none", backends: [] }] }, gpu: null, worker: { version: "0", renderVersion: 2 } };
/** A v2 report with a hardware H.264 encoder (a Mac's VideoToolbox). */
const V2_HW = {
  codecs: [
    { codec: "av1", decode: "hardware", encode: "none", backends: [{ name: "videotoolbox", class: "os", direction: "decode", hw: true }] },
    { codec: "h264", decode: "hardware", encode: "hardware", backends: [{ name: "videotoolbox", class: "os", direction: "decode", hw: true }, { name: "videotoolbox", class: "os", direction: "encode", hw: true }] },
  ],
  encoders: [{ backend: "videotoolbox", class: "os", codec: "h264", hw: true, intraOnly: false }],
};

function keyOf(fp: string, tier: "540" | "1080" = "540", codec: "av1" | "h264" = "av1"): string {
  return mediaProxyPlan({ fingerprint: fp, tier, codec, sourceCodec: "av1", decoders: ["dav1d"] }).key;
}

function params(over: { [K in keyof ProxyParams]?: ProxyParams[K] | undefined } = {}): ProxyParams {
  const p = { fingerprint: "fp-a1", tier: "540", codec: "av1", decoderClass: "software", key: keyOf("fp-a1"), asset: "a1", ...over } as Record<string, unknown>;
  for (const k of Object.keys(p)) if (p[k] === undefined) delete p[k];
  return p as unknown as ProxyParams;
}

describe("the kind", () => {
  const kind = (report: unknown = V1) => mediaProxyKind({ serverData: "/sd", projects: { onBranch: () => Effect.die(0) }, cache: () => undefined, report: () => report });

  test("background, cold, a batch of the fingerprint, identity by the cache key, poison remembered", () => {
    const k = kind();
    expect(k).toMatchObject({ kind: MEDIA_PROXY, version: 1, family: "media", warm: false, defaultPriority: "background", qos: "background", rememberPoison: true });
    expect(k.batch!.items(params())).toEqual(["fp-a1"]);
    expect(k.dedupe!(params())).toBe(proxyDedupe(params()));
    expect(k.dedupe!(params())).toBe(JSON.stringify([MEDIA_PROXY, keyOf("fp-a1")]));
    // Another fingerprint (or RENDER_VERSION, which is in the key) is another job.
    expect(k.dedupe!(params({ fingerprint: "fp-2", key: keyOf("fp-2") }))).not.toBe(k.dedupe!(params()));
    const memory = k.memory as (p: ProxyParams) => unknown;
    expect(memory(params())).toEqual(BATCH_MEMORY);
    expect(memory(params({ display: { width: 3840, height: 2160 } }))).toEqual(BATCH_MEMORY_LARGE);
    expect(resolveMemoryRule(BATCH_MEMORY, 8 * GiB)).toBe(2 * GiB);
    // A project asset reads its manifest; a library file needs none.
    const files = k.files as (p: ProxyParams) => ReadonlyArray<{ name: string }>;
    expect(files(params()).map((f) => f.name)).toEqual(["in/manifest.json", "out/"]);
    expect(files(params({ asset: undefined, file: { library: "l1", root: "/r", rel: { components: [] }, expect: {} } })).map((f) => f.name)).toEqual(["out/"]);
  });

  test("codec and resources follow the capability report (Decisions 5, 18)", () => {
    expect(proxyCodec(V1)).toBe("av1");
    expect(proxyCodec(V2_HW)).toBe("h264");
    expect(proxyCodec(undefined)).toBe("av1");
    expect(hardwareEncoder(V1, "av1")).toBe(false);
    expect(proxyResources(V1, "av1")).toEqual(["gpu", "cpu"]);
    expect(proxyResources(V2_HW, "h264")).toEqual(["gpu", "hwenc"]);
    expect(proxyResources(V2_HW, "av1")).toEqual(["gpu", "cpu"]);
    const resources = kind(V2_HW).resources as (p: ProxyParams) => unknown;
    expect(resources(params({ codec: "h264" }))).toEqual(["gpu", "hwenc"]);
    // v1 lists both directions' backends; v2 only its decode rows count.
    expect(decodersOf(V1, "av1")).toEqual(["dav1d", "svt-av1"]);
    expect(decodersOf(V2_HW, "h264")).toEqual(["videotoolbox"]);
    expect(decodersOf(V1, "vp9")).toEqual([]);
  });

  test("params are fin-media's ProxyParams with one source and a key", () => {
    expect(validateJobParams(MEDIA_PROXY, 1, params())).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_PROXY, 1, params({ asset: undefined }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PROXY, 1, params({ key: "x" }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PROXY, 1, { ...params(), path: "/etc/passwd" })).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PROXY, 1, { ...params(), tier: "720" })).toMatchObject({ ok: false });
  });

  test("the key is planned with the report's decoder; a codec nothing decodes is CODEC_UNAVAILABLE", () => {
    const sw = mediaProxyPlan({ fingerprint: "fp", tier: "540", codec: "av1", sourceCodec: "av1", decoders: decodersOf(V1, "av1") });
    expect(sw).toMatchObject({ decoder: "dav1d", decoderClass: "software" });
    const os = mediaProxyPlan({ fingerprint: "fp", tier: "540", codec: "av1", sourceCodec: "av1", decoders: decodersOf(V2_HW, "av1") });
    expect(os.decoderClass).toBe("os");
    expect(os.key).not.toBe(sw.key);
    let err: unknown;
    try {
      mediaProxyPlan({ fingerprint: "fp", tier: "540", codec: "av1", sourceCodec: "h264", decoders: decodersOf(V1, "h264") });
    } catch (e) {
      err = e;
    }
    expect(err).toMatchObject({ code: "CODEC_UNAVAILABLE", details: { direction: "decode" } });
  });

  test("PROXY_UNAVAILABLE names the asset, the tier and the job", () => {
    expect(proxyUnavailable("a1", "1080", "j9")).toMatchObject({ code: "PROXY_UNAVAILABLE", details: { asset: "a1", tier: "1080", job: "j9" } });
  });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

/** A project with asset `a1` (fingerprint `fp`) on the clip, and `a2` audio-only. */
async function project(serverData: string, fp: string): Promise<Engine> {
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const engine = await Engine.create(serverData, dir, "Proxy", "30");
  engines.push(engine);
  await engine.commit(
    tx("t1", 0, [
      { op: "asset.add", id: "$a1", path: media, duration: "2/5", video: { width: 160, height: 90, rate: "30" }, fingerprint: fp },
      { op: "asset.add", id: "$a2", path: join(work, "sound.wav"), duration: "1", audio: { sampleRate: 48000, channels: 2 }, fingerprint: "fp-sound" },
    ]),
  );
  return engine;
}

/** A real engine with the media kinds, the cache and the proxy service over one project. */
async function setup(fp = "fp-a1") {
  const serverData = realpathSync(mkdtempSync(join(work, "sd-")));
  const engine = await project(serverData, fp);
  const cache = await MediaCache.open(serverData);
  let caps: ReturnType<typeof mediaCapabilities> | undefined;
  const projects = {
    onBranch: <A>(projectId: string, branch: string, f: (e: Engine, b: string) => Promise<string>) =>
      projectId !== engine.projectId ? Effect.fail(toFinError(new Error("unknown project"))) : Effect.tryPromise({ try: async () => JSON.parse(await f(engine, branch)) as A, catch: toFinError }),
    prepareMediaJob: () => Effect.die("unused"),
    dirOf: (projectId: string) => (projectId === engine.projectId ? dirname(media) : undefined),
  };
  const t = await testEngine({
    serverData,
    register: (r) => registerMediaKinds(r, { serverData, projects, proxy: { cache: () => cache, report: () => caps?.current() } }),
  });
  caps = mediaCapabilities({ engine: t.engine });
  const registry = {
    ensure: () => Effect.void,
    resolveBranch: (_: string, b: string) => Effect.succeed(b),
    snapshot: (_: string, b: string) => Effect.tryPromise({ try: async () => JSON.parse(await engine.snapshot(b)) as { rev: number; project: unknown }, catch: toFinError }),
    dirOf: projects.dirOf,
  };
  const proxies = makeProxyService({ registry, engine: t.engine, capabilities: caps, cache: () => cache });
  return { t, cache, engine, proxies, caps, serverData, pid: engine.projectId };
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const fail = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(Effect.flip(e));

/** Whether a failed job is the GPU's absence (the test then stops, unless the GPU is required). */
function noGpu(e: { code: string } | null | undefined): boolean {
  if (e?.code !== "GPU_UNAVAILABLE") return false;
  if (GPU_REQUIRED) throw new Error("FIN_REQUIRE_GPU=1 but the worker has no GPU adapter");
  console.warn("media.proxy: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead");
  return true;
}

describe("the job and the RPCs", () => {
  test("a proxy is made, committed and served; the second ask is a cache hit", async () => {
    const { cache, proxies, pid } = await setup();
    const made = await Effect.runPromise(Effect.result(proxies.request(OWNER, { projectId: pid, asset: "a1" })));
    if (made._tag === "Failure" && noGpu(made.failure)) return;
    if (made._tag === "Failure") throw made.failure;
    const a = made.success;
    expect(a).toMatchObject({ asset: "a1", tier: "540", codec: "av1", state: "ready", key: keyOf("fp-a1") });
    const r = a.result as ProxyResult;
    expect(r).toMatchObject({ key: a.key, tier: "540", codec: "av1", backend: "svt-av1", class: "software", decoder: "dav1d", frames: 12, size: { width: 160, height: 90 } });
    // Committed for the project that asked.
    const meta = (await cache.lookup(a.key)) as EntryMeta;
    expect(meta).toMatchObject({ recipe: "proxy-540", fingerprint: "fp-a1", decoderClass: "software", encoder: { backend: "svt-av1", class: "software" } });
    expect(meta.files.map((f) => f.name)).toEqual(["proxy.mp4", "table.bin"]);
    expect(await cache.projectsOf(a.key)).toEqual([pid]);
    expect(statSync(join(cache.entryDir(a.key)!, "proxy.mp4")).size).toBe(meta.files[0]!.bytes);
    // Asked again: answered from the cache, no job.
    expect(await run(proxies.request(OWNER, { projectId: pid, asset: "a1" }))).toEqual({ asset: "a1", tier: "540", codec: "av1", key: a.key, state: "ready", cached: true });
    // The status: 540 ready, 1080 none (the preferred codec's key).
    const s = await run(proxies.status(OWNER, { projectId: pid, asset: "a1" }));
    expect(s).toEqual({
      asset: "a1",
      fingerprint: "fp-a1",
      tiers: [
        { tier: "540", state: "ready", key: a.key, codec: "av1" },
        { tier: "1080", state: "none", key: keyOf("fp-a1", "1080"), codec: "av1" },
      ],
    });
  }, 180_000);

  test("a consumer needing a proxy being made gets PROXY_UNAVAILABLE, then the entry", async () => {
    const { t, proxies, pid } = await setup();
    const e = await fail(proxies.ensure(OWNER, { projectId: pid, asset: "a1", tier: "1080" }));
    expect(e).toMatchObject({ code: "PROXY_UNAVAILABLE", details: { asset: "a1", tier: "1080" } });
    const job = (e.details as { job: string }).job;
    const s = await run(proxies.status(OWNER, { projectId: pid, asset: "a1" }));
    expect(s.tiers[1]).toMatchObject({ tier: "1080", job });
    const done = await t.wait(job, 120_000);
    if (done.state === "failed" && noGpu(done.error)) return;
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    const meta = await run(proxies.ensure(OWNER, { projectId: pid, asset: "a1", tier: "1080" }));
    expect(meta).toMatchObject({ recipe: "proxy-1080", fingerprint: "fp-a1" });
  }, 180_000);

  test("dedupe_coalesces_identical_requests", async () => {
    const { t, pid } = await setup();
    const p = params({ projectId: pid, branch: "main" });
    const [a, b] = await Promise.all([t.submit(MEDIA_PROXY, p, { project: pid }), t.submit(MEDIA_PROXY, p, { project: pid })]);
    expect(b.id).toBe(a.id);
    // Another tier is another key, so another job.
    const c = await t.submit(MEDIA_PROXY, params({ projectId: pid, tier: "1080", key: keyOf("fp-a1", "1080") }), { project: pid });
    expect(c.id).not.toBe(a.id);
    await t.wait(a.id, 120_000);
    await t.wait(c.id, 120_000);
  }, 180_000);

  test("poison_remembered_until_fingerprint_changes", async () => {
    // Test builds of the worker abort on a fingerprint starting `test-crash-`.
    const fp = "test-crash-a1";
    const { t, pid } = await setup(fp);
    const p = params({ fingerprint: fp, key: keyOf(fp), projectId: pid });
    const first = await t.runJob(MEDIA_PROXY, p, { project: pid });
    expect(first.state).toBe("failed");
    expect(first.error?.code).toBe("POISON_ITEM");
    // The same item again: the poisoned job answers, nothing runs.
    const again = await t.submit(MEDIA_PROXY, p, { project: pid });
    expect(again.id).toBe(first.id);
    // Another fingerprint (the content changed) is another key, and runs.
    const fp2 = "test-crash-a2";
    const other = await t.submit(MEDIA_PROXY, params({ fingerprint: fp2, key: keyOf(fp2), projectId: pid }), { project: pid });
    expect(other.id).not.toBe(first.id);
    await t.wait(other.id, 120_000);
  }, 180_000);

  test("an H.264 proxy without a hardware encoder fails CODEC_UNAVAILABLE with guidance", async () => {
    if (process.platform !== "linux") return;
    const { proxies, pid } = await setup();
    const e = await fail(proxies.request(OWNER, { projectId: pid, asset: "a1", codec: "h264" }));
    if (noGpu(e)) return;
    expect(e).toMatchObject({ code: "CODEC_UNAVAILABLE", details: { direction: "encode" } });
    expect(e.fix).toBeTruthy();
  }, 180_000);

  test("an audio asset, an unknown asset and a moved fingerprint are refused", async () => {
    const { t, proxies, pid } = await setup();
    expect((await fail(proxies.request(OWNER, { projectId: pid, asset: "a2" }))).code).toBe("ASSET_TYPE_UNSUPPORTED");
    expect((await fail(proxies.request(OWNER, { projectId: pid, asset: "a404" }))).code).toBe("ASSET_NOT_FOUND");
    const v = await t.runJob(MEDIA_PROXY, params({ projectId: pid, fingerprint: "fp-old", key: keyOf("fp-old") }), { project: pid });
    expect(v.error).toMatchObject({ code: "ASSET_CHANGED", details: { asset: "a1", reason: "fingerprint" } });
  }, 180_000);
});

/** The clip as a library catalogue would hand it to the stage. */
function stageFile(probe: unknown): StageFile {
  const s = statSync(media, { bigint: true });
  return {
    library: "l1",
    fileId: 1,
    root: dirname(media),
    rel: { components: [Buffer.from(basename(media)).toString("base64")] },
    display: basename(media),
    expect: { size: Number(s.size), mtimeNs: s.mtimeNs.toString(), fileId: Buffer.from(new BigUint64Array([s.ino]).buffer).reverse().toString("base64") },
    hydrate: false,
    fp: "fp-lib",
    size: Number(s.size),
    kind: "media",
    probe,
    storage: "local",
  } as StageFile;
}

describe("the proxy540 stage", () => {
  const video = { media: { video: { codec: "av1", display: { width: 160, height: 90 } } } };

  test("it applies to video files this machine decodes, at <recipe>.<RENDER_VERSION>", () => {
    const s = proxy540Stage({ report: () => V1, cache: () => undefined });
    const v = mediaVersions();
    expect(s).toMatchObject({ name: PROXY540_STAGE, jobKind: MEDIA_PROXY, batch: 1, version: `${v.recipes["proxy-540"]}.${v.renderVersion}` });
    expect(s.appliesTo(stageFile(video))).toBe(true);
    // Audio-only, not media, or a codec nothing here decodes: skipped.
    expect(s.appliesTo(stageFile({ media: { audio: { codec: "aac" } } }))).toBe(false);
    expect(s.appliesTo({ ...stageFile(video), kind: "image" })).toBe(false);
    expect(s.appliesTo(stageFile({ media: { video: { codec: "h264" } } }))).toBe(false);
    const p = s.params([stageFile(video)], { library: "l1", pools: {} as never }) as ProxyParams;
    expect(p).toMatchObject({ fingerprint: "fp-lib", tier: "540", codec: "av1", decoderClass: "software", key: keyOf("fp-lib"), display: { width: 160, height: 90 } });
    expect(p.file).toMatchObject({ library: "l1", root: dirname(media), hydrate: false });
    expect(p.asset).toBeUndefined();
  });

  test("the stage runs for a library file: the entry is committed and isDone sees it", async () => {
    const { t, cache } = await setup();
    const s = proxy540Stage({ report: () => V1, cache: () => cache });
    expect(await Effect.runPromise(s.isDone!("fp-lib"))).toBe(false);
    const p = s.params([stageFile(video)], { library: "l1", pools: {} as never }) as ProxyParams;
    const v: JobView = await t.runJob(MEDIA_PROXY, p, { library: "l1" });
    if (v.state === "failed" && noGpu(v.error)) return;
    expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
    expect(((v.result as { items: Record<string, ProxyResult> }).items["fp-lib"])?.frames).toBe(12);
    expect(await cache.projectsOf(p.key)).toEqual([]);
    expect(await Effect.runPromise(s.isDone!("fp-lib"))).toBe(true);
  }, 180_000);
});

describe("first use", () => {
  test("each video asset gets one background 1080 proxy per run, audio none", async () => {
    const submitted: Array<{ owner: unknown; req: { params: ProxyParams; priority?: string } }> = [];
    const snapshot = {
      rev: 1,
      project: {
        assets: {
          a1: { kind: "media", path: media, fingerprint: "fp-a1", video: { width: 160, height: 90 } },
          a2: { kind: "media", path: "/x.wav", fingerprint: "fp-sound", audio: {} },
        },
      },
    };
    const proxies = makeProxyService({
      registry: { ensure: () => Effect.void, resolveBranch: (_, b) => Effect.succeed(b), snapshot: () => Effect.succeed(snapshot), dirOf: () => work },
      engine: {
        submit: (owner, req) => {
          submitted.push({ owner, req: req as never });
          return Effect.succeed({ id: `j${submitted.length}` } as JobView);
        },
        wait: () => Effect.die(0),
        get: () => Effect.die(0),
      },
      capabilities: () => Effect.succeed(V1),
      cache: () => undefined,
    });
    proxies.used("p1");
    proxies.used("p1");
    await new Promise((r) => setTimeout(r, 200));
    expect(submitted).toHaveLength(1);
    expect(submitted[0]!.owner).toEqual(MEDIA_SYSTEM_OWNER);
    expect(submitted[0]!.req.params).toMatchObject({ tier: "1080", codec: "av1", fingerprint: "fp-a1", asset: "a1", key: keyOf("fp-a1", "1080") });
    // The kind's background priority (no explicit one).
    expect(submitted[0]!.req.priority).toBeUndefined();
  }, 60_000);
});

describe("registration", () => {
  test("media.proxy registers with the media kinds when the proxy deps are given", () => {
    const r = makeKindRegistry();
    registerMediaKinds(r, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) } });
    expect(r.kind(MEDIA_PROXY)).toBeUndefined();
    const r2 = makeKindRegistry();
    registerMediaKinds(r2, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) }, proxy: { cache: () => undefined, report: () => undefined } });
    expect(r2.kind(MEDIA_PROXY)?.warm).toBe(false);
  });
});
