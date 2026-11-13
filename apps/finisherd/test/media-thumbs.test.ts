// `media.thumbs` (6B-1 plan Task 15, Decision 17): the kind, its params and plan, and the job
// through the real worker and cache: thumbnails and a filmstrip made, committed and served, the
// `1080` proxy handed to the worker when its entry exists, items restricted for a retry, and the
// refusals. The GPU scale pass runs on SwiftShader here (`FIN_GPU=software`); a host without any
// adapter skips (`FIN_REQUIRE_GPU=1` fails instead).
import { mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { EntryMeta, StripIndex, ThumbsParams } from "@finisher/core-types";
import { Engine, MediaCache, mediaThumbsPlan, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import { toFinError } from "../src/errors.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_THUMBS, mediaThumbsKind, thumbsDedupe, thumbsItems } from "../src/jobs/kinds/media.thumbs.ts";
import { mediaCapabilities } from "../src/media/caps.ts";
import { BATCH_MEMORY, BATCH_MEMORY_LARGE } from "../src/media/memory.ts";
import { makeProxyService } from "../src/media/proxy.ts";
import { makeThumbsService } from "../src/media/thumbs.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";
import { makeAv1Fixture } from "./support/media.ts";

const GPU_REQUIRED = process.env.FIN_REQUIRE_GPU === "1";
const OWNER = { kind: "local", id: "local" } as const;

let work: string;
let media: string;
const engines: Engine[] = [];

beforeAll(() => {
  process.env.FIN_GPU ??= "software";
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-thumbs-")));
  media = join(work, "clip.mp4");
  makeAv1Fixture(media, 12);
}, 600_000);

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const planOf = (fp: string, mode: Parameters<typeof mediaThumbsPlan>[0]["mode"], duration?: string) =>
  mediaThumbsPlan({ fingerprint: fp, mode, sourceCodec: "av1", decoders: ["dav1d"], ...(duration === undefined ? {} : { duration }) });

const THUMBS_MODE = { thumbs: { times: ["0", "1/10", "1/5"], maxSide: 128 } } as const;

function params(over: Record<string, unknown> = {}): ThumbsParams {
  const planned = planOf("fp-a1", THUMBS_MODE);
  const p = { fingerprint: "fp-a1", asset: "a1", mode: planned.mode, decoderClass: planned.decoderClass, key: planned.key, ...over } as Record<string, unknown>;
  for (const k of Object.keys(p)) if (p[k] === undefined) delete p[k];
  return p as unknown as ThumbsParams;
}

describe("the kind", () => {
  const kind = () => mediaThumbsKind({ serverData: "/sd", projects: { onBranch: () => Effect.die(0) }, cache: () => undefined });

  test("background, cold, gpu, items per thumbnail or sprite, identity by the cache key, poison remembered", () => {
    const k = kind();
    expect(k).toMatchObject({ kind: MEDIA_THUMBS, version: 1, family: "media", warm: false, defaultPriority: "background", qos: "background", rememberPoison: true });
    expect((k.resources as () => unknown)()).toEqual(["gpu"]);
    expect(k.batch!.items(params())).toEqual(["0", "1", "2"]);
    expect(thumbsItems(params())).toEqual(["0", "1", "2"]);
    const strip = planOf("fp-a1", { strip: { tileHeight: 90 } }, "25");
    expect(strip.mode).toEqual({ strip: { tileHeight: 90, sprites: 3 } });
    expect(strip.items).toEqual(["0", "1", "2"]);
    expect(k.batch!.items(params({ mode: strip.mode, key: strip.key }))).toEqual(["0", "1", "2"]);
    // A retry restricts the params to the items not acknowledged: the worker makes those only.
    expect(k.batch!.restrict(params(), ["1", "2"])).toEqual({ ...params(), only: ["1", "2"] });
    expect(k.dedupe!(params())).toBe(thumbsDedupe(params()));
    expect(k.dedupe!(params())).toBe(JSON.stringify([MEDIA_THUMBS, params().key]));
    expect(k.dedupe!(params({ key: planOf("fp-a1", { thumbs: { times: ["0"], maxSide: 64 } }).key }))).not.toBe(k.dedupe!(params()));
    const memory = k.memory as (p: ThumbsParams) => unknown;
    expect(memory(params())).toEqual(BATCH_MEMORY);
    expect(memory(params({ display: { width: 3840, height: 2160 } }))).toEqual(BATCH_MEMORY_LARGE);
    const files = k.files as (p: ThumbsParams) => ReadonlyArray<{ name: string }>;
    expect(files(params()).map((f) => f.name)).toEqual(["in/manifest.json", "out/"]);
  });

  test("params are fin-media's ThumbsParams: counts, sizes and the key are checked", () => {
    expect(validateJobParams(MEDIA_THUMBS, 1, params())).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_THUMBS, 1, params({ key: "x" }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_THUMBS, 1, params({ mode: { thumbs: { times: [], maxSide: 128 } } }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_THUMBS, 1, params({ mode: { thumbs: { times: ["0"], maxSide: 4096 } } }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_THUMBS, 1, params({ only: ["9"] }))).toMatchObject({ ok: false });
    // What a retry submits: the params restricted to the items left.
    expect(validateJobParams(MEDIA_THUMBS, 1, params({ only: ["1", "2"] }))).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_THUMBS, 1, { ...params(), path: "/etc/passwd" })).toMatchObject({ ok: false });
  });

  test("the key is planned with the report's decoder and a filmstrip's sprites from the duration", () => {
    expect(planOf("fp", THUMBS_MODE)).toMatchObject({ decoder: "dav1d", decoderClass: "software" });
    expect(planOf("fp", { strip: { tileHeight: 90 } }, "101/10").mode).toEqual({ strip: { tileHeight: 90, sprites: 2 } });
    const other = planOf("fp", { thumbs: { times: ["0", "1/10", "1/5"], maxSide: 64 } });
    expect(other.key).not.toBe(planOf("fp", THUMBS_MODE).key);
    let err: unknown;
    try {
      mediaThumbsPlan({ fingerprint: "fp", mode: THUMBS_MODE, sourceCodec: "h264", decoders: ["dav1d"] });
    } catch (e) {
      err = e;
    }
    expect(err).toMatchObject({ code: "CODEC_UNAVAILABLE", details: { direction: "decode" } });
    // A filmstrip with no duration cannot be planned.
    err = undefined;
    try {
      planOf("fp", { strip: { tileHeight: 90 } });
    } catch (e) {
      err = e;
    }
    expect(err).toMatchObject({ code: "INVALID_REQUEST" });
  });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

/** A project with asset `a1` (fingerprint `fp`) on the clip, and `a2` audio-only. */
async function project(serverData: string, fp: string): Promise<Engine> {
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const engine = await Engine.create(serverData, dir, "Thumbs", "30");
  engines.push(engine);
  await engine.commit(
    tx("t1", 0, [
      { op: "asset.add", id: "$a1", path: media, duration: "2/5", video: { width: 160, height: 90, rate: "30" }, fingerprint: fp },
      { op: "asset.add", id: "$a2", path: join(work, "sound.wav"), duration: "1", audio: { sampleRate: 48000, channels: 2 }, fingerprint: "fp-sound" },
    ]),
  );
  return engine;
}

/** A real engine with the media kinds, the cache and the proxy and thumbs services over one project. */
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
  const thumbs = makeThumbsService({ engine: t.engine, capabilities: caps, proxies, cache: () => cache });
  return { t, cache, engine, proxies, thumbs, serverData, pid: engine.projectId };
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const fail = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(Effect.flip(e));

/** Whether a failure is the GPU's absence (the test then stops, unless the GPU is required). */
function noGpu(e: { code: string } | null | undefined): boolean {
  if (e?.code !== "GPU_UNAVAILABLE") return false;
  if (GPU_REQUIRED) throw new Error("FIN_REQUIRE_GPU=1 but the worker has no GPU adapter");
  console.warn("media.thumbs: no GPU adapter here; set FIN_REQUIRE_GPU=1 to fail instead");
  return true;
}

describe("the job and the RPC", () => {
  test("thumbnails are made, committed and served; the second ask is a cache hit", async () => {
    const { cache, thumbs, pid } = await setup();
    const made = await Effect.runPromise(Effect.result(thumbs.request(OWNER, { projectId: pid, asset: "a1", at: ["0", "1/10", "1/5"], maxSide: 128 })));
    if (made._tag === "Failure" && noGpu(made.failure)) return;
    if (made._tag === "Failure") throw made.failure;
    const a = made.success;
    expect(a).toMatchObject({ asset: "a1", mode: "thumbs", state: "ready", key: planOf("fp-a1", THUMBS_MODE).key });
    expect(a.files?.map((f) => f.name)).toEqual(["t0.png", "t1.png", "t2.png"]);
    const meta = (await cache.lookup(a.key)) as EntryMeta;
    expect(meta).toMatchObject({ recipe: "thumbs", fingerprint: "fp-a1", decoderClass: "software" });
    expect(meta.files.map((f) => f.name)).toEqual(["t0.png", "t1.png", "t2.png"]);
    expect(await cache.projectsOf(a.key)).toEqual([pid]);
    // A PNG at 128 px longest side (the clip is 160 × 90).
    const png = readFileSync(join(cache.entryDir(a.key)!, "t0.png"));
    expect(png.subarray(1, 4).toString()).toBe("PNG");
    expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([128, 72]);
    // Asked again: answered from the cache, no job.
    const again = await run(thumbs.request(OWNER, { projectId: pid, asset: "a1", at: ["0", "1/10", "1/5"], maxSide: 128 }));
    expect(again).toEqual({ asset: "a1", mode: "thumbs", key: a.key, state: "ready", cached: true, files: a.files });
    // Other times are another entry.
    const other = await run(thumbs.request(OWNER, { projectId: pid, asset: "a1", at: ["0"], maxSide: 64, wait: false }));
    expect(other.key).not.toBe(a.key);
    expect(other.state).toBe("making");
  }, 240_000);

  test("a filmstrip is made with its index", async () => {
    const { cache, thumbs, pid } = await setup();
    const made = await Effect.runPromise(Effect.result(thumbs.request(OWNER, { projectId: pid, asset: "a1", strip: true })));
    if (made._tag === "Failure" && noGpu(made.failure)) return;
    if (made._tag === "Failure") throw made.failure;
    const a = made.success;
    expect(a).toMatchObject({ mode: "strip", state: "ready" });
    expect(a.files?.map((f) => f.name)).toEqual(["s0.png", "strip.json"]);
    const index = JSON.parse(readFileSync(join(cache.entryDir(a.key)!, "strip.json"), "utf8")) as StripIndex;
    expect(index).toMatchObject({ version: 1, tileWidth: 160, tileHeight: 90, tileSeconds: 1, spriteSeconds: 10 });
    expect(index.sprites).toEqual([{ file: "s0.png", tiles: [{ at: "0", frame: 0 }] }]);
  }, 240_000);

  test("reads_proxy_when_present: the 1080 proxy entry is handed to the job, the key is the same", async () => {
    const { t, thumbs, proxies, pid } = await setup();
    const withoutProxy = await Effect.runPromise(Effect.result(thumbs.request(OWNER, { projectId: pid, asset: "a1", at: ["0"], maxSide: 64 })));
    if (withoutProxy._tag === "Failure" && noGpu(withoutProxy.failure)) return;
    if (withoutProxy._tag === "Failure") throw withoutProxy.failure;
    const first = withoutProxy.success;
    const firstJob = await t.engine.get(first.job!).pipe(Effect.runPromise);
    expect((firstJob.params as ThumbsParams).proxy).toBeUndefined();
    // Make the 1080 proxy.
    const proxy = await run(proxies.request(OWNER, { projectId: pid, asset: "a1", tier: "1080" }));
    expect(proxy.state).toBe("ready");
    expect((await run(proxies.peek(OWNER, { projectId: pid, asset: "a1" })))?.key).toBe(proxy.key);
    // Other thumbnails of the same asset now read it; the answer is the same shape.
    const made = await run(thumbs.request(OWNER, { projectId: pid, asset: "a1", at: ["0", "1/10"], maxSide: 64 }));
    expect(made.state).toBe("ready");
    const job = await t.engine.get(made.job!).pipe(Effect.runPromise);
    expect((job.params as ThumbsParams).proxy).toMatchObject({ key: proxy.key, codec: "av1" });
    expect(made.files?.map((f) => f.name)).toEqual(["t0.png", "t1.png"]);
  }, 300_000);

  test("bad requests, an audio asset, an unknown asset and a moved fingerprint are refused", async () => {
    const { t, thumbs, pid } = await setup();
    const bad = async (req: Record<string, unknown>) => (await fail(thumbs.request(OWNER, { projectId: pid, asset: "a1", ...req } as never))).code;
    expect(await bad({})).toBe("INVALID_REQUEST");
    expect(await bad({ at: ["0"], strip: true })).toBe("INVALID_REQUEST");
    expect(await bad({ at: [] })).toBe("INVALID_REQUEST");
    expect(await bad({ at: Array.from({ length: 65 }, () => "0") })).toBe("INVALID_REQUEST");
    expect(await bad({ strip: true, maxSide: 64 })).toBe("INVALID_REQUEST");
    expect(await bad({ at: ["0"], tileHeight: 64 })).toBe("INVALID_REQUEST");
    expect(await bad({ asset: "a2", at: ["0"] })).toBe("ASSET_TYPE_UNSUPPORTED");
    expect(await bad({ asset: "a404", at: ["0"] })).toBe("ASSET_NOT_FOUND");
    const p = planOf("fp-old", THUMBS_MODE);
    const v = await t.runJob(MEDIA_THUMBS, params({ fingerprint: "fp-old", key: p.key, projectId: pid }), { project: pid });
    expect(v.error).toMatchObject({ code: "ASSET_CHANGED", details: { asset: "a1", reason: "fingerprint" } });
  }, 240_000);
});
