// `media.pcm` (6C plan Task 18): the kind's shape (background, cold, `cpu`, the cache key as its
// identity), the key planned by `mediaPcmPlan`, the whole job through a real `fin-workers media`
// and the media cache (made, committed, a hit on the second ask), dedupe by key, a `finisherd`
// that dies mid-job (the attempt is interrupted and retried from sample 0, nothing is indexed in
// between, the entry has the same bytes), the `pcm16m` library stage, and the first-use `pcm48` of an asset put on an audio track.
// Fixtures are 16-bit WAV files written here.
import { mkdtempSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import type { EntryMeta, PcmParams, PcmResult } from "@finisher/core-types";
import { Engine, MediaCache, mediaPcmPlan, mediaVersions, validateJobParams } from "@finisher/fin-node";
import { Effect, Exit, Scope } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { toFinError } from "../src/errors.ts";
import { makeKindRegistry } from "../src/jobs/kinds.ts";
import { MEDIA_CAPABILITIES } from "../src/jobs/kinds/media.capabilities.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_PCM, mediaPcmKind, pcmDedupe } from "../src/jobs/kinds/media.pcm.ts";
import type { StageFile, StageSpec } from "../src/library/api.ts";
import type { MediaCacheService } from "../src/media/cache.ts";
import { BATCH_MEMORY } from "../src/media/memory.ts";
import { PCM16M_STAGE, audioAssetsOf, makePcmService, pcm16mStage } from "../src/media/pcm.ts";
import { MEDIA_SYSTEM_OWNER } from "../src/media/proxy.ts";
import { mediaWiring } from "../src/media/wiring.ts";
import { makeProjectRegistry } from "../src/projects.ts";
import { closeTestEngines, testEngine, until } from "./support/jobs.ts";

const OWNER = { kind: "local", id: "local" } as const;

let work: string;
/** 10 s at 48 kHz stereo: 3 chunks of 131,072 frames and one of 86,784. */
let stereo: string;
/** 1 s at 44.1 kHz mono. */
let mono: string;
const engines: Engine[] = [];

/** A 16-bit PCM WAV of a tone per channel. */
function writeWav(path: string, frames: number, rate: number, channels: number): void {
  const data = Buffer.alloc(frames * channels * 2);
  for (let i = 0; i < frames; i++) {
    for (let c = 0; c < channels; c++) {
      const x = (c === 0 ? 0.5 : 0.25) * Math.sin((2 * Math.PI * (c === 0 ? 440 : 1000) * i) / rate);
      data.writeInt16LE(Math.round(x * 32767), (i * channels + c) * 2);
    }
  }
  const h = Buffer.alloc(44);
  h.write("RIFF", 0);
  h.writeUInt32LE(36 + data.length, 4);
  h.write("WAVE", 8);
  h.write("fmt ", 12);
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20);
  h.writeUInt16LE(channels, 22);
  h.writeUInt32LE(rate, 24);
  h.writeUInt32LE(rate * channels * 2, 28);
  h.writeUInt16LE(channels * 2, 32);
  h.writeUInt16LE(16, 34);
  h.write("data", 36);
  h.writeUInt32LE(data.length, 40);
  writeFileSync(path, Buffer.concat([h, data]));
}

beforeAll(() => {
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-pcm-")));
  stereo = join(work, "stereo.wav");
  writeWav(stereo, 480_000, 48_000, 2);
  mono = join(work, "mono.wav");
  writeWav(mono, 44_100, 44_100, 1);
});

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

function plan(fp: string, recipe: "pcm48" | "pcm16m" = "pcm48", channels = 2) {
  return mediaPcmPlan({ fingerprint: fp, recipe, channels });
}

function params(over: { [K in keyof PcmParams]?: PcmParams[K] | undefined } = {}): PcmParams {
  const fp = over.fingerprint ?? "fp-a1";
  const recipe = over.recipe ?? "pcm48";
  const p = plan(fp, recipe);
  const out = { fingerprint: fp, recipe, layout: p.layout, decoderClass: p.decoderClass, key: p.key, asset: "a1", ...over } as Record<string, unknown>;
  for (const k of Object.keys(out)) if (out[k] === undefined) delete out[k];
  return out as unknown as PcmParams;
}

describe("the kind and the plan", () => {
  const kind = () => mediaPcmKind({ serverData: "/sd", projects: { onBranch: () => Effect.die(0) }, cache: () => undefined });

  test("background, cold, cpu, a batch of the fingerprint, identity by the cache key", () => {
    const k = kind();
    expect(k).toMatchObject({ kind: MEDIA_PCM, version: 1, family: "media", warm: false, defaultPriority: "background", qos: "background", rememberPoison: true, resources: ["cpu"], memory: BATCH_MEMORY });
    // Decision 23: never resumable (no checkpoint); a retry starts from sample 0.
    expect(k.resumable).toBeUndefined();
    expect(k.batch!.items(params())).toEqual(["fp-a1"]);
    expect(k.dedupe!(params())).toBe(pcmDedupe(params()));
    expect(k.dedupe!(params())).toBe(JSON.stringify([MEDIA_PCM, plan("fp-a1").key]));
    expect(k.dedupe!(params({ recipe: "pcm16m" }))).not.toBe(k.dedupe!(params()));
    const files = k.files as (p: PcmParams) => ReadonlyArray<{ name: string }>;
    expect(files(params()).map((f) => f.name)).toEqual(["in/manifest.json", "out/"]);
  });

  test("params are fin-media's PcmParams with one source and a key", () => {
    expect(validateJobParams(MEDIA_PCM, 1, params())).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_PCM, 1, params({ asset: undefined }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PCM, 1, params({ key: "x" }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PCM, 1, { ...params(), recipe: "pcm8" })).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_PCM, 1, { ...params({ recipe: "pcm16m" }), layout: "stereo" })).toMatchObject({ ok: false });
  });

  test("key_has_audio_version_and_decoder_class: the plan by codec and channels", () => {
    const v = mediaVersions();
    expect(v.audioVersion).toBeGreaterThanOrEqual(1);
    expect(v.recipes).toMatchObject({ pcm48: 1, pcm16m: 1 });
    // WAV (the probe names no codec) is symphonia's; a mono source stays mono for pcm48 only.
    expect(plan("fp")).toMatchObject({ decoder: "symphonia", decoderClass: "software", layout: "stereo" });
    expect(plan("fp", "pcm48", 1).layout).toBe("mono");
    expect(plan("fp", "pcm16m", 2).layout).toBe("mono");
    expect(mediaPcmPlan({ fingerprint: "fp", recipe: "pcm48", sourceCodec: "opus", channels: 2 })).toMatchObject({ decoder: "libopus" });
    expect(plan("fp").key).not.toBe(plan("fp", "pcm48", 1).key);
    expect(plan("fp").key).not.toBe(plan("fp2").key);
    expect(() => mediaPcmPlan({ fingerprint: "fp", recipe: "pcm48", sourceCodec: "ac3", channels: 2 })).toThrow(expect.objectContaining({ code: "CODEC_UNAVAILABLE" }));
  });

  test("registers with the media kinds when the cache deps are given", () => {
    const r = makeKindRegistry();
    registerMediaKinds(r, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) }, proxy: { cache: () => undefined, report: () => undefined } });
    expect(r.kind(MEDIA_PCM)?.warm).toBe(false);
  });

  test("the assets of audio-track items, in first-use order", () => {
    const project = {
      timelines: [
        {
          tracks: [
            { kind: "video", items: [{ id: "i1", asset: "v1" }] },
            { kind: "audio", items: [{ id: "i2", asset: "s1" }, { id: "i3", asset: "s2" }, { id: "i4", asset: "s1" }] },
          ],
        },
      ],
    };
    expect(audioAssetsOf(project)).toEqual(["s1", "s2"]);
    expect(audioAssetsOf(project, new Set(["i3", "i1"]))).toEqual(["s2"]);
  });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

/** A project with asset `a1` (fingerprint `fp`) on `path`. */
async function project(serverData: string, fp: string, path = stereo): Promise<Engine> {
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const engine = await Engine.create(serverData, dir, "Pcm", "30");
  engines.push(engine);
  await engine.commit(tx("t1", 0, [{ op: "asset.add", id: "$a1", path, duration: "10", audio: { sampleRate: 48000, channels: 2 }, fingerprint: fp }]));
  return engine;
}

/** A real engine with the media kinds, the cache and the PCM service over one project. */
async function setup(fp = "fp-a1", serverData = realpathSync(mkdtempSync(join(work, "sd-"))), cache?: MediaCache) {
  const engine = await project(serverData, fp);
  const c = cache ?? (await MediaCache.open(serverData));
  const t = await engineOver(serverData, engine, c);
  const registry = {
    ensure: () => Effect.void,
    resolveBranch: (_: string, b: string) => Effect.succeed(b),
    snapshot: (_: string, b: string) => Effect.tryPromise({ try: async () => JSON.parse(await engine.snapshot(b)) as { rev: number; project: unknown }, catch: toFinError }),
    dirOf: () => work,
  };
  const pcm = makePcmService({ registry, engine: t.engine, cache: () => c });
  // The id the project gave asset `$a1`.
  const asset = Object.keys((JSON.parse(await engine.snapshot("main")) as { project: { assets: object } }).project.assets)[0]!;
  return { t, cache: c, engine, pcm, serverData, pid: engine.projectId, asset };
}

async function engineOver(serverData: string, engine: Engine, cache: MediaCache) {
  const projects = {
    onBranch: <A>(projectId: string, branch: string, f: (e: Engine, b: string) => Promise<string>) =>
      projectId !== engine.projectId ? Effect.fail(toFinError(new Error("unknown project"))) : Effect.tryPromise({ try: async () => JSON.parse(await f(engine, branch)) as A, catch: toFinError }),
    prepareMediaJob: () => Effect.die("unused"),
    dirOf: () => work,
  };
  return testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects, proxy: { cache: () => cache, report: () => undefined } }) });
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);

describe("the job and the RPC", () => {
  test("pcm48_chunks_and_meta: a feed is made, committed and served; the second ask is a cache hit", async () => {
    const { cache, pcm, pid, asset } = await setup();
    const a = await run(pcm.request(OWNER, { projectId: pid, asset }));
    expect(a).toMatchObject({ asset, recipe: "pcm48", state: "ready", key: plan("fp-a1").key });
    const r = a.result as PcmResult;
    expect(r).toMatchObject({ key: a.key, recipe: "pcm48", layout: "stereo", frames: 480_000, chunks: 4, sourceRate: 48_000, sourceChannels: 2, decoder: { backend: "symphonia", class: "software" } });
    const meta = (await cache.lookup(a.key)) as EntryMeta;
    expect(meta).toMatchObject({ recipe: "pcm48", fingerprint: "fp-a1", decoderClass: "software", params: { rate: 48000, layout: "stereo", audioVersion: mediaVersions().audioVersion } });
    expect(meta.files.map((f) => [f.name, f.bytes])).toEqual([
      ["chunk-000000.f32", 1 << 20],
      ["chunk-000001.f32", 1 << 20],
      ["chunk-000002.f32", 1 << 20],
      ["chunk-000003.f32", 86_784 * 8],
    ]);
    expect(await cache.projectsOf(a.key)).toEqual([pid]);
    expect(statSync(join(cache.entryDir(a.key)!, "chunk-000003.f32")).size).toBe(86_784 * 8);
    expect(await run(pcm.request(OWNER, { projectId: pid, asset }))).toEqual({ asset, recipe: "pcm48", key: a.key, state: "ready", cached: true });
    // An unknown asset, and a fingerprint that moved since the ask, are refused.
    expect((await Effect.runPromise(Effect.flip(pcm.request(OWNER, { projectId: pid, asset: "a404" })))).code).toBe("ASSET_NOT_FOUND");
  }, 180_000);

  test("same_key_dedupes", async () => {
    const { t, pid, asset } = await setup();
    const p = params({ asset, projectId: pid, branch: "main" });
    const [a, b] = await Promise.all([t.submit(MEDIA_PCM, p, { project: pid }), t.submit(MEDIA_PCM, p, { project: pid })]);
    expect(b.id).toBe(a.id);
    // The other recipe is another key, so another job.
    const c = await t.submit(MEDIA_PCM, params({ asset, projectId: pid, recipe: "pcm16m" }), { project: pid });
    expect(c.id).not.toBe(a.id);
    expect((await t.wait(a.id, 120_000)).state).toBe("succeeded");
    const done = await t.wait(c.id, 120_000);
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    expect(((done.result as { items: Record<string, PcmResult> }).items["fp-a1"])).toMatchObject({ recipe: "pcm16m", layout: "mono", frames: 160_000 });
  }, 180_000);

  test("daemon_death_retry_is_byte_identical", async () => {
    // Test builds of the worker wait 300 ms after each chunk for a fingerprint `test-slow-…`.
    const fp = "test-slow-a1";
    const clean = await setup(fp);
    const want = await run(clean.pcm.request(OWNER, { projectId: clean.pid, asset: clean.asset }));
    const wantMeta = (await clean.cache.lookup(want.key)) as EntryMeta;

    const sd = realpathSync(mkdtempSync(join(work, "sd-")));
    const cache = await MediaCache.open(sd);
    const first = await setup(fp, sd, cache);
    const p = params({ fingerprint: fp, asset: first.asset, projectId: first.pid, branch: "main" });
    expect(p.key).toBe(want.key);
    const v = await first.t.submit(MEDIA_PCM, p, { project: first.pid });
    await until(() => (first.t.engine.list({}).jobs.find((j) => j.id === v.id)?.progress?.done ?? 0) > 0, 60_000);
    await first.t.crash();
    // The restarted server retries the interrupted attempt (5C: not resumable, so from the
    // start, with an emptied `out/`); nothing is indexed until it succeeds.
    const second = await engineOver(sd, first.engine, cache);
    expect(second.jobs.get(v.id)?.state).not.toBe("succeeded");
    expect(await cache.lookup(p.key)).toBeNull();
    const again = await second.wait(v.id, 120_000);
    expect(again.state, JSON.stringify(again.error)).toBe("succeeded");
    expect(again.attempts).toBe(2);
    const meta = (await cache.lookup(p.key)) as EntryMeta;
    expect(meta.files).toEqual(wantMeta.files);
    expect({ ...meta, createdAt: 0 }).toEqual({ ...wantMeta, createdAt: 0 });
  }, 240_000);
});

/** A WAV as a library catalogue would hand it to the stage. */
function stageFile(path: string, probe: unknown): StageFile {
  const s = statSync(path, { bigint: true });
  return {
    library: "l1",
    fileId: 1,
    root: dirname(path),
    rel: { components: [Buffer.from(basename(path)).toString("base64")] },
    display: basename(path),
    expect: { size: Number(s.size), mtimeNs: s.mtimeNs.toString(), fileId: Buffer.from(new BigUint64Array([s.ino]).buffer).reverse().toString("base64") },
    hydrate: false,
    fp: "fp-lib",
    size: Number(s.size),
    kind: "media",
    probe,
    storage: "local",
  } as StageFile;
}

describe("pcm16m_stage_registered_and_runs_after_probe", () => {
  const wavProbe = { kind: "media", duration: "1", audio: { sampleRate: 44100, channels: 1 } };

  test("it applies to library files with audio this machine decodes, after probe, at <recipe>.<RENDER>.<AUDIO>", () => {
    const s = pcm16mStage({ cache: () => undefined });
    const v = mediaVersions();
    expect(s).toMatchObject({ name: PCM16M_STAGE, jobKind: MEDIA_PCM, batch: 1, version: `${v.recipes.pcm16m}.${v.renderVersion}.${v.audioVersion}` });
    expect(s.dependsOn).toEqual(["probe"]);
    expect(s.appliesTo(stageFile(mono, wavProbe))).toBe(true);
    // Video only, not media, or a codec nothing here decodes: skipped.
    expect(s.appliesTo(stageFile(mono, { media: { video: { codec: "av1" } } }))).toBe(false);
    expect(s.appliesTo({ ...stageFile(mono, wavProbe), kind: "image" })).toBe(false);
    expect(s.appliesTo(stageFile(mono, { audio: { channels: 6 }, media: { audio: { codec: "ac3" } } }))).toBe(false);
    const p = s.params([stageFile(mono, wavProbe)], { library: "l1", pools: {} as never }) as PcmParams;
    expect(p).toMatchObject({ fingerprint: "fp-lib", recipe: "pcm16m", layout: "mono", decoderClass: "software", key: plan("fp-lib", "pcm16m", 1).key });
    expect(p.file).toMatchObject({ library: "l1", root: dirname(mono), hydrate: false });
    expect(p.asset).toBeUndefined();
  });

  test("the stage runs for a library file: the entry is committed and isDone sees it", async () => {
    const { t, cache } = await setup();
    const s = pcm16mStage({ cache: () => cache });
    expect(await Effect.runPromise(s.isDone!("fp-lib"))).toBe(false);
    const p = s.params([stageFile(mono, wavProbe)], { library: "l1", pools: {} as never }) as PcmParams;
    const v: JobView = await t.runJob(MEDIA_PCM, p, { library: "l1" });
    expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
    expect(((v.result as { items: Record<string, PcmResult> }).items["fp-lib"])?.frames).toBe(16_000);
    expect(await cache.projectsOf(p.key)).toEqual([]);
    expect(await Effect.runPromise(s.isDone!("fp-lib"))).toBe(true);
  }, 180_000);

  test("the media wiring registers it when the services start", () => {
    const stages: StageSpec[] = [];
    const engine = { submit: () => Effect.die(0), wait: () => Effect.die(0), get: () => Effect.die(0), onDone: () => () => {} };
    const media = mediaWiring({ cache: undefined } as unknown as MediaCacheService, { onDrop: () => {} }, () => {});
    const started = media.start(engine as never, { registerStage: (s: StageSpec) => stages.push(s), registry: { list: () => [] } } as never, {} as never);
    expect(stages.map((s) => s.name)).toEqual([PCM16M_STAGE]);
    expect(started.pcm).toBeDefined();
  });
});

describe("first use", () => {
  test("first_use_submits_pcm48: an audio clip inserted in an open project → one media.pcm job", async () => {
    const sd = mkdtempSync(join(work, "reg-"));
    const submitted: Array<{ kind: string; params: PcmParams; priority?: string }> = [];
    const owners: unknown[] = [];
    const engine = {
      submit: (owner: unknown, req: { kind: string; params: unknown; priority?: string }) =>
        Effect.sync(() => {
          if (req.kind === MEDIA_CAPABILITIES) return { id: "caps", state: "queued" } as JobView;
          submitted.push(req as never);
          owners.push(owner);
          return { id: `j${submitted.length}`, state: "queued" } as JobView;
        }),
      wait: () => Effect.succeed([{ id: "caps", state: "succeeded", result: {} } as unknown as JobView]),
      get: () => Effect.die(0),
      onDone: () => () => {},
    };
    const media = mediaWiring({ cache: undefined } as unknown as MediaCacheService, { onDrop: () => {} }, () => {});
    const scope = Effect.runSync(Scope.make());
    try {
      const r = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(sd, { onOpened: media.onOpened, onCommitted: media.onCommitted })));
      const { projectId } = await Effect.runPromise(r.create(join(sd, "p"), "Reg", "30"));
      media.start(engine as never, { registerStage: () => {}, registry: { list: () => [] } } as never, r);
      const snap = (await Effect.runPromise(r.snapshot(projectId, "main"))) as { rev: number; project: { timelines: Array<{ id: string }> } };
      const timeline = snap.project.timelines[0]!.id;
      const commit = (txId: string, baseRev: number, ops: unknown[]) => Effect.runPromise(r.commit(projectId, { txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops }));
      // An asset that is in the project but on no audio track: nothing yet.
      await commit("t1", snap.rev, [{ op: "asset.add", id: "$s1", path: stereo, duration: "10", audio: { sampleRate: 48000, channels: 2 }, fingerprint: "fp-s1" }]);
      await new Promise((res) => setTimeout(res, 100));
      expect(submitted).toHaveLength(0);
      const s1 = Object.keys(((await Effect.runPromise(r.snapshot(projectId, "main"))) as { project: { assets: object } }).project.assets)[0]!;
      await commit("t2", snap.rev + 1, [
        { op: "track.add", id: "$tr", timeline, kind: "audio" },
        { op: "item.insert", id: "$i1", track: "$tr", at: "0", duration: "1", mode: "overwrite", kind: "clip", asset: s1, sourceIn: "0" },
      ]);
      await vi.waitFor(() => expect(submitted).toHaveLength(1), { timeout: 10_000 });
      expect(submitted[0]).toMatchObject({ kind: MEDIA_PCM, params: { recipe: "pcm48", fingerprint: "fp-s1", asset: s1, layout: "stereo", key: plan("fp-s1").key, projectId, branch: "main" } });
      // The kind's background priority (no explicit one), owned by the server.
      expect(submitted[0]!.priority).toBeUndefined();
      expect(owners[0]).toEqual(MEDIA_SYSTEM_OWNER);
      // The same asset used again: asked for once this run.
      const tr = ((await Effect.runPromise(r.snapshot(projectId, "main"))) as { project: { timelines: Array<{ tracks: Array<{ id: string; kind: string }> }> } }).project.timelines[0]!.tracks.find((t) => t.kind === "audio")!.id;
      await commit("t3", snap.rev + 2, [{ op: "item.insert", id: "$i2", track: tr, at: "2", duration: "1", mode: "overwrite", kind: "clip", asset: s1, sourceIn: "0" }]);
      await new Promise((res) => setTimeout(res, 300));
      expect(submitted).toHaveLength(1);
    } finally {
      await Effect.runPromise(Scope.close(scope, Exit.void));
    }
  }, 60_000);
});
