// `media.waveform` (6C plan Task 19): the kind's shape (background, cold, `cpu`, the cache key as
// its identity, the `pcm48` entry granted read only), the key planned by `mediaWavePlan`, the
// whole job through a real `fin-workers media` and the media cache (made from the decode, or from
// the asset's `pcm48` entry with the same bytes; a hit on the second ask), and the waveform that
// follows a first-use `pcm48`'s commit. Fixtures are 16-bit WAV files written here.
import { mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, JobView } from "@finisher/contracts";
import { finError, toFinError } from "../src/errors.ts";
import type { EntryMeta, WaveParams, WaveResult } from "@finisher/core-types";
import { Engine, MediaCache, mediaPcmPlan, mediaWavePlan, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { MEDIA_WAVEFORM, mediaWaveformKind, waveDedupe } from "../src/jobs/kinds/media.waveform.ts";
import { BATCH_MEMORY } from "../src/media/memory.ts";
import { makePcmService } from "../src/media/pcm.ts";
import { makeWaveService } from "../src/media/wave.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";

const OWNER = { kind: "local", id: "local" } as const;

let work: string;
/** 3 s at 44.1 kHz stereo (the resampler runs). */
let stereo: string;
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
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-wave-")));
  stereo = join(work, "stereo.wav");
  writeWav(stereo, 132_300, 44_100, 2);
});

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const pcmPlan = (fp: string) => mediaPcmPlan({ fingerprint: fp, recipe: "pcm48", channels: 2 });
const wavePlan = (fp: string) => mediaWavePlan({ fingerprint: fp, decoderClass: pcmPlan(fp).decoderClass });

function params(over: { [K in keyof WaveParams]?: WaveParams[K] | undefined } = {}): WaveParams {
  const fp = over.fingerprint ?? "fp-a1";
  const out = { fingerprint: fp, decoderClass: "software", key: wavePlan(fp).key, asset: "a1", ...over } as Record<string, unknown>;
  for (const k of Object.keys(out)) if (out[k] === undefined) delete out[k];
  return out as unknown as WaveParams;
}

describe("the kind and the plan", () => {
  const kind = () => mediaWaveformKind({ serverData: "/sd", projects: { onBranch: () => Effect.die(0) }, cache: () => undefined });

  test("background, cold, cpu, a batch of the fingerprint, identity by the cache key", () => {
    const k = kind();
    expect(k).toMatchObject({ kind: MEDIA_WAVEFORM, version: 1, family: "media", warm: false, defaultPriority: "background", qos: "background", rememberPoison: true, crashRestarts: true, resources: ["cpu"], memory: BATCH_MEMORY });
    expect(k.resumable).toBeUndefined();
    expect(k.batch!.items(params())).toEqual(["fp-a1"]);
    expect(k.dedupe!(params())).toBe(waveDedupe(params()));
    expect(k.dedupe!(params())).toBe(JSON.stringify([MEDIA_WAVEFORM, wavePlan("fp-a1").key]));
    expect(k.dedupe!(params({ fingerprint: "fp-b" }))).not.toBe(k.dedupe!(params()));
    const files = k.files as (p: WaveParams) => ReadonlyArray<{ name: string; mode: string }>;
    expect(files(params()).map((f) => f.name)).toEqual(["in/manifest.json", "out/"]);
    // The `pcm48` entry's directory is a read-only input.
    expect(files(params({ pcmKey: pcmPlan("fp-a1").key }))).toEqual([
      { name: "in/manifest.json", mode: "read" },
      { name: "in/pcm.json", mode: "read" },
      { name: "out/", mode: "write" },
    ]);
  });

  test("params are fin-media's WaveParams with an asset and keys", () => {
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params())).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params({ pcmKey: pcmPlan("fp-a1").key }))).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params({ asset: undefined }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params({ asset: "" }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params({ key: "x" }))).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_WAVEFORM, 1, params({ pcmKey: "x" }))).toMatchObject({ ok: false });
  });

  test("the key follows the pcm48 plan's decoder class and the audio version", () => {
    expect(wavePlan("fp").levels).toEqual([256, 1024, 4096, 16384, 65536]);
    expect(wavePlan("fp").key).toMatch(/^[0-9a-f]{64}$/);
    expect(wavePlan("fp").key).not.toBe(pcmPlan("fp").key);
    expect(wavePlan("fp").key).not.toBe(wavePlan("fp2").key);
    expect(mediaWavePlan({ fingerprint: "fp", decoderClass: "os" }).key).not.toBe(wavePlan("fp").key);
  });

  test("registers with the media kinds when the cache deps are given", () => {
    const r = { kinds: [] as string[], registerFamily: () => {}, registerKind: (k: { kind: string }) => void r.kinds.push(k.kind) };
    registerMediaKinds(r as never, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) }, proxy: { cache: () => undefined, report: () => undefined } });
    expect(r.kinds).toContain(MEDIA_WAVEFORM);
  });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

/** A project with asset `a1` (fingerprint `fp`) on the stereo WAV, optionally with an audio track using it. */
async function project(serverData: string, fp: string, onTrack: boolean): Promise<Engine> {
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const engine = await Engine.create(serverData, dir, "Wave", "30");
  engines.push(engine);
  await engine.commit(tx("t1", 0, [{ op: "asset.add", id: "$a1", path: stereo, duration: "3", audio: { sampleRate: 44100, channels: 2 }, fingerprint: fp }]));
  if (onTrack) {
    const snap = JSON.parse(await engine.snapshot("main")) as { rev: number; project: { timelines: Array<{ id: string }>; assets: object } };
    const asset = Object.keys(snap.project.assets)[0]!;
    await engine.commit(
      tx("t2", snap.rev, [
        { op: "track.add", id: "$tr", timeline: snap.project.timelines[0]!.id, kind: "audio" },
        { op: "item.insert", id: "$i1", track: "$tr", at: "0", duration: "1", mode: "overwrite", kind: "clip", asset, sourceIn: "0" },
      ]),
    );
  }
  return engine;
}

/** A real engine with the media kinds, the cache, and the PCM and waveform services over one project. */
async function setup(onTrack = false, fp = "fp-a1") {
  const serverData = realpathSync(mkdtempSync(join(work, "sd-")));
  const engine = await project(serverData, fp, onTrack);
  const cache = await MediaCache.open(serverData);
  const projects = {
    onBranch: <A>(projectId: string, branch: string, f: (e: Engine, b: string) => Promise<string>) =>
      projectId !== engine.projectId ? Effect.fail(toFinError(new Error("unknown project"))) : Effect.tryPromise({ try: async () => JSON.parse(await f(engine, branch)) as A, catch: toFinError }),
    prepareMediaJob: () => Effect.die("unused"),
    dirOf: () => work,
  };
  const t = await testEngine({ serverData, register: (r) => registerMediaKinds(r, { serverData, projects, proxy: { cache: () => cache, report: () => undefined } }) });
  const registry = {
    ensure: () => Effect.void,
    resolveBranch: (_: string, b: string) => Effect.succeed(b),
    snapshot: (_: string, b: string) => Effect.tryPromise({ try: async () => JSON.parse(await engine.snapshot(b)) as { rev: number; project: unknown }, catch: toFinError }),
    dirOf: () => work,
  };
  let wave: ReturnType<typeof makeWaveService> | undefined;
  const pcm = makePcmService({ registry, engine: t.engine, cache: () => cache, afterPcm48: (p, job) => wave?.afterPcm48(p, job) });
  wave = makeWaveService({ registry, pcm, engine: t.engine, cache: () => cache });
  const asset = Object.keys((JSON.parse(await engine.snapshot("main")) as { project: { assets: object } }).project.assets)[0]!;
  return { t, cache, engine, pcm, wave, pid: engine.projectId, asset };
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);

const peaksOf = (cache: MediaCache, key: string): Buffer => readFileSync(join(cache.entryDir(key)!, "peaks.bin"));

describe("the job and the RPC", () => {
  test("a waveform is made from the decode, committed and served; the second ask is a cache hit", async () => {
    const { cache, wave, pid, asset } = await setup();
    const w = wavePlan("fp-a1");
    const a = await run(wave.request(OWNER, { projectId: pid, asset }));
    expect(a).toMatchObject({ asset, key: w.key, levels: w.levels, state: "ready" });
    // 132,300 frames at 44.1 kHz → 144,000 at 48 kHz.
    expect(a.result).toMatchObject({ key: w.key, levels: 5, frames: 144_000, from: "decode" });
    const meta = (await cache.lookup(w.key)) as EntryMeta;
    expect(meta).toMatchObject({ recipe: "wave", fingerprint: "fp-a1", decoderClass: "software", params: { rate: 48000 } });
    expect(meta.files.map((f) => f.name)).toEqual(["peaks.bin"]);
    const bytes = peaksOf(cache, w.key);
    expect(bytes.subarray(0, 8).toString()).toBe("FINWAVE1");
    expect(bytes.readUInt32LE(8)).toBe(5);
    expect(bytes.readBigUInt64LE(16)).toBe(144_000n);
    expect(await cache.projectsOf(w.key)).toEqual([pid]);
    expect(await run(wave.request(OWNER, { projectId: pid, asset }))).toEqual({ asset, key: w.key, levels: w.levels, state: "ready", cached: true });
    expect((await Effect.runPromise(Effect.flip(wave.request(OWNER, { projectId: pid, asset: "a404" })))).code).toBe("ASSET_NOT_FOUND");
  }, 180_000);

  test("with the asset's pcm48 in the cache the job reads it, and the bytes are the same", async () => {
    const direct = await setup();
    const want = await run(direct.wave.request(OWNER, { projectId: direct.pid, asset: direct.asset }));
    expect(want.result).toMatchObject({ from: "decode" });
    const wantBytes = peaksOf(direct.cache, want.key);

    const { cache, pcm, wave, pid, asset, t } = await setup();
    const fed = await run(pcm.request(OWNER, { projectId: pid, asset }));
    expect(fed.state).toBe("ready");
    const a = await run(wave.request(OWNER, { projectId: pid, asset }));
    expect(a.key).toBe(want.key);
    expect(a.result).toMatchObject({ from: "pcm48", frames: 144_000 });
    expect(peaksOf(cache, a.key).equals(wantBytes)).toBe(true);
    const job = t.engine.list({}).jobs.find((j) => j.kind === MEDIA_WAVEFORM)!;
    expect((job.params as WaveParams).pcmKey).toBe(fed.key);
  }, 240_000);

  test("same_key_dedupes", async () => {
    const { t, pid, asset } = await setup();
    const p = params({ asset, projectId: pid, branch: "main" });
    const [a, b] = await Promise.all([t.submit(MEDIA_WAVEFORM, p, { project: pid }), t.submit(MEDIA_WAVEFORM, p, { project: pid })]);
    expect(b.id).toBe(a.id);
    const done: JobView = await t.wait(a.id, 120_000);
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    expect((done.result as { items: Record<string, WaveResult> }).items["fp-a1"]).toMatchObject({ key: p.key, levels: 5 });
  }, 180_000);

  test("a fingerprint that moved since the ask fails the job ASSET_CHANGED", async () => {
    const { t, pid, asset } = await setup();
    const stale = params({ fingerprint: "fp-other", asset, projectId: pid, branch: "main" });
    const done: JobView = await t.runJob(MEDIA_WAVEFORM, stale, { project: pid });
    expect(done.state).toBe("failed");
    expect(done.error?.code).toBe("ASSET_CHANGED");
  }, 60_000);
});

describe("a crashing worker", () => {
  test("a crash mid-job is retried and committed, not poisoned", async () => {
    // Test builds of the worker abort on the first attempt of `test-crashonce-…`.
    const fp = "test-crashonce-a1";
    const { t, cache, wave, pid, asset } = await setup(false, fp);
    const w = mediaWavePlan({ fingerprint: fp, decoderClass: pcmPlan(fp).decoderClass });
    const p = params({ fingerprint: fp, key: w.key, asset, projectId: pid, branch: "main" });
    const v = await t.runJob(MEDIA_WAVEFORM, p, { project: pid });
    expect(v.state, JSON.stringify(v.error)).toBe("succeeded");
    expect(v.attempts).toBe(2);
    expect(await cache.lookup(p.key)).not.toBeNull();
    // No poison is left: asking again (dedupe would answer a poisoned job) is a cache hit.
    expect(await run(wave.request(OWNER, { projectId: pid, asset }))).toMatchObject({ key: p.key, state: "ready", cached: true });
  }, 240_000);
});

describe("after the pcm48 commit", () => {
  test("a pcm48 that outlasts the wait still gets its waveform once it commits", async () => {
    const pcmPlanned = { ...pcmPlan("fp-slow"), fingerprint: "fp-slow", asset: "a1", projectId: "p1", branch: "main" } as unknown as Parameters<ReturnType<typeof makeWaveService>["afterPcm48"]>[0];
    const submitted: Array<{ kind: string; params: WaveParams }> = [];
    let waits = 0;
    const engine = {
      // The pcm48 job is still running for the first two waits.
      wait: () => (++waits <= 2 ? Effect.fail(finError("WAIT_TIMEOUT", "still running", {})) : Effect.succeed([{ id: "j1", state: "succeeded" } as unknown as JobView])),
      submit: (_: unknown, req: { kind: string; params: WaveParams }) => Effect.sync(() => (submitted.push(req), { id: "w1", state: "queued" } as unknown as JobView)),
    };
    const held = new Set([pcmPlanned.key]);
    const cache = { lookup: async (k: string) => (held.has(k) ? ({ key: k } as unknown) : null), serve: async () => undefined } as unknown as MediaCache;
    const wave = makeWaveService({ registry: {} as never, pcm: {} as never, engine: engine as never, cache: () => cache, waitMs: 10 });
    wave.afterPcm48(pcmPlanned, "j1");
    await vi.waitFor(() => expect(submitted).toHaveLength(1), { timeout: 10_000 });
    expect(waits).toBe(3);
    expect(submitted[0]).toMatchObject({ kind: MEDIA_WAVEFORM, params: { fingerprint: "fp-slow", pcmKey: pcmPlanned.key, asset: "a1" } });
  });

  test("wave_after_pcm48_commit: the first-use pcm48 of an audio clip is followed by its waveform, read from that entry", async () => {
    const { cache, pcm, wave, t, pid, engine } = await setup(true);
    const feed = pcmPlan("fp-a1");
    const w = wavePlan("fp-a1");
    await pcm.used(pid);
    // The pcm48 job is submitted first; the waveform only once its entry is committed.
    await vi.waitFor(async () => expect(await cache.lookup(w.key)).not.toBeNull(), { timeout: 120_000, interval: 50 });
    expect(await cache.lookup(feed.key)).not.toBeNull();
    const jobs = t.engine.list({}).jobs;
    const kinds = jobs.map((j) => j.kind);
    expect(kinds.filter((k) => k === "media.pcm")).toHaveLength(1);
    expect(kinds.filter((k) => k === MEDIA_WAVEFORM)).toHaveLength(1);
    const pcmJob = jobs.find((j) => j.kind === "media.pcm")!;
    const waveJob = jobs.find((j) => j.kind === MEDIA_WAVEFORM)!;
    expect(pcmJob.state).toBe("succeeded");
    expect((waveJob.params as WaveParams).pcmKey).toBe(feed.key);
    const done = await t.wait(waveJob.id, 60_000);
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    expect(((done.result as { items: Record<string, WaveResult> }).items["fp-a1"])).toMatchObject({ from: "pcm48", frames: 144_000 });
    // The entry is the one a decode makes.
    expect((await cache.lookup(w.key)) as EntryMeta | null).toMatchObject({ recipe: "wave" });
    // Asked for again: a cache hit, and no further waveform job.
    const asset = Object.keys((JSON.parse(await engine.snapshot("main")) as { project: { assets: object } }).project.assets)[0]!;
    expect(await run(wave.request(OWNER, { projectId: pid, asset }))).toMatchObject({ key: w.key, state: "ready", cached: true });
    expect(t.engine.list({}).jobs.filter((j) => j.kind === MEDIA_WAVEFORM)).toHaveLength(1);
  }, 240_000);
});
