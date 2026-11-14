// `media.listen` (6C plan Task 21): the kind's shape (normal, cold, `cpu`, identity by head and
// range), its params, the RPC's range checks, and the whole job through a real `fin-workers media`
// and the media cache: measured from decoded originals, then from the `pcm48` entry `media.pcm`
// committed (listed in the manifest and pinned for the job). Fixtures are 16-bit WAV files.
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { ListenResult } from "@finisher/core-types";
import { Engine, MediaCache, validateJobParams } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { toFinError } from "../src/errors.ts";
import { makeKindRegistry } from "../src/jobs/kinds.ts";
import { MEDIA_LISTEN, listenDedupe, mediaListenKind } from "../src/jobs/kinds/media.listen.ts";
import { registerMediaKinds } from "../src/jobs/kinds/media.look.ts";
import { listen } from "../src/media/listen.ts";
import { BATCH_MEMORY } from "../src/media/memory.ts";
import { makePcmService } from "../src/media/pcm.ts";
import type { MediaCacheService } from "../src/media/cache.ts";
import { mediaWiring } from "../src/media/wiring.ts";
import { closeTestEngines, testEngine } from "./support/jobs.ts";

const OWNER = { kind: "local", id: "local" } as const;

let work: string;
let tone: string;
const engines: Engine[] = [];

/** A 16-bit stereo WAV at 48 kHz of a 1 kHz tone at −20 dBFS RMS a channel. */
function writeWav(path: string, frames: number): void {
  const data = Buffer.alloc(frames * 4);
  for (let i = 0; i < frames; i++) {
    const v = Math.round(0.1414 * Math.sin((2 * Math.PI * 1000 * i) / 48_000) * 32767);
    data.writeInt16LE(v, i * 4);
    data.writeInt16LE(v, i * 4 + 2);
  }
  const h = Buffer.alloc(44);
  h.write("RIFF", 0);
  h.writeUInt32LE(36 + data.length, 4);
  h.write("WAVE", 8);
  h.write("fmt ", 12);
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20);
  h.writeUInt16LE(2, 22);
  h.writeUInt32LE(48_000, 24);
  h.writeUInt32LE(48_000 * 4, 28);
  h.writeUInt16LE(4, 32);
  h.writeUInt16LE(16, 34);
  h.write("data", 36);
  h.writeUInt32LE(data.length, 40);
  writeFileSync(path, Buffer.concat([h, data]));
}

beforeAll(() => {
  work = realpathSync(mkdtempSync(join(tmpdir(), "fin-listen-")));
  tone = join(work, "tone.wav");
  writeWav(tone, 4 * 48_000);
});

afterEach(async () => {
  await closeTestEngines();
  for (const e of engines.splice(0)) await e.close();
});

afterAll(() => rmSync(work, { recursive: true, force: true }));

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

const base = { projectId: "p1", branch: "main", rev: 3, timeline: "tl1", from: "0", to: "3", planSamples: [] };

describe("the kind", () => {
  test("normal, cold, cpu, batch memory, identity by head and range", () => {
    const k = mediaListenKind({ serverData: "/sd", projects: { onBranch: () => Effect.die(0) }, cache: () => undefined });
    expect(k).toMatchObject({ kind: MEDIA_LISTEN, version: 1, family: "media", warm: false, defaultPriority: "normal", resources: ["cpu"], memory: BATCH_MEMORY, scopes: ["project.read"] });
    expect((k.files as ReadonlyArray<{ name: string }>).map((f) => f.name)).toEqual(["in/snapshot.json", "in/manifest.json"]);
    expect(k.dedupe!(base)).toBe(listenDedupe(base));
    expect(k.dedupe!({ ...base, to: "2" })).not.toBe(k.dedupe!(base));
    expect(k.dedupe!({ ...base, rev: 4 })).not.toBe(k.dedupe!(base));
    expect(listenDedupe({ ...base, rev: undefined } as never)).toBeUndefined();
  });

  test("params are fin-media's ListenParams; the plan samples are the host's", () => {
    expect(validateJobParams(MEDIA_LISTEN, 1, base)).toMatchObject({ ok: true });
    expect(validateJobParams(MEDIA_LISTEN, 1, { ...base, to: "0" })).toMatchObject({ ok: false });
    // Review S1: a range longer than a day is refused at submit.
    expect(validateJobParams(MEDIA_LISTEN, 1, { ...base, to: "1000000000" })).toMatchObject({ ok: false });
    expect(validateJobParams(MEDIA_LISTEN, 1, { ...base, planSamples: [{ at: "0", blake3: "0".repeat(64) }] })).toMatchObject({ ok: false });
  });

  test("registers with the media kinds when the cache deps are given", () => {
    const r = makeKindRegistry();
    registerMediaKinds(r, { serverData: "/sd", projects: { prepareMediaJob: () => Effect.die(0), dirOf: () => undefined, onBranch: () => Effect.die(0) }, proxy: { cache: () => undefined, report: () => undefined } });
    expect(r.kind(MEDIA_LISTEN)?.warm).toBe(false);
  });
});

/** A real engine with the media kinds and the cache over a project with the tone on `t2` over `[0, 3)`. */
async function setup(fp = "fp-tone") {
  const serverData = realpathSync(mkdtempSync(join(work, "sd-")));
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const engine = await Engine.create(serverData, dir, "Listen", "30");
  engines.push(engine);
  await engine.commit(tx("t1", 0, [{ op: "asset.add", id: "$a1", path: tone, duration: "4", audio: { sampleRate: 48000, channels: 2 }, fingerprint: fp }]));
  const snap = JSON.parse(await engine.snapshot("main")) as { rev: number; project: { assets: object } };
  const asset = Object.keys(snap.project.assets)[0]!;
  await engine.commit(tx("t2", snap.rev, [{ op: "item.insert", id: "$i1", track: "t2", at: "0", duration: "3", mode: "overwrite", kind: "clip", asset, sourceIn: "0" }]));
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
  return { t, cache, engine, registry, pid: engine.projectId, asset };
}

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const flip = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(Effect.flip(e));

describe("the RPC and the job", () => {
  test("listen_range_past_end_invalid, and the range's other refusals", async () => {
    const { t, registry, pid } = await setup();
    const deps = { registry, engine: t.engine };
    expect((await flip(listen(deps, OWNER, { projectId: pid, to: "4" }))).code).toBe("INVALID_RANGE");
    expect((await flip(listen(deps, OWNER, { projectId: pid, from: "-1" }))).code).toBe("INVALID_RANGE");
    expect((await flip(listen(deps, OWNER, { projectId: pid, from: "2", to: "2" }))).code).toBe("INVALID_RANGE");
    expect((await flip(listen(deps, OWNER, { projectId: pid, from: "1.5" }))).code).toBe("INVALID_TIME");
    expect((await flip(listen(deps, OWNER, { projectId: pid, timeline: "tl9" }))).code).toBe("NOT_FOUND");
    // Review S1: the job's own inputs refuse a range past the timeline's end, whoever submitted it.
    const inDir = mkdtempSync(join(work, "in-"));
    const prep = (from: string, to: string) => Effect.runPromise(Effect.flip(Effect.tryPromise({ try: () => engines[0]!.prepareListenJob("main", "tl1", from, to, inDir), catch: toFinError })));
    expect((await prep("0", "1000000000")).code).toBe("INVALID_RANGE");
    expect((await prep("0", "4")).code).toBe("INVALID_RANGE");
    // `wait: false` answers the job at once.
    const { job } = (await run(listen(deps, OWNER, { projectId: pid, from: "1", to: "2", wait: false }))) as { job: { id: string } };
    expect((await t.wait(job.id, 120_000)).state).toBe("succeeded");
  }, 180_000);

  test("listen_decodes_when_missing, then listen_uses_pcm48_when_present", async () => {
    const { t, cache, registry, pid, asset } = await setup();
    const deps = { registry, engine: t.engine };
    const decoded = (await run(listen(deps, OWNER, { projectId: pid }))) as ListenResult;
    expect(decoded).toMatchObject({ timeline: "tl1", from: "0", to: "3", stats: { sourcesFromCache: 0, sourcesDecoded: 1 } });
    expect(decoded.tracks.map((x) => x.track)).toEqual(["t2"]);
    expect(Math.abs(decoded.mix.integrated! - -16.99)).toBeLessThanOrEqual(0.3);
    expect(decoded.mix.silences).toEqual([]);
    // Make the asset's pcm48 entry; the next listen reads it, with the same numbers.
    const pcm = makePcmService({ registry, engine: t.engine, cache: () => cache });
    const made = await run(pcm.request(OWNER, { projectId: pid, asset }));
    expect(made.state).toBe("ready");
    const v = await t.submit(MEDIA_LISTEN, { projectId: pid, branch: "main", rev: 2, timeline: "tl1", from: "0", to: "3" }, { project: pid });
    const done = await t.wait(v.id, 120_000);
    expect(done.state, JSON.stringify(done.error)).toBe("succeeded");
    const cached = done.result as ListenResult;
    expect(cached.stats).toEqual({ sourcesFromCache: 1, sourcesDecoded: 0 });
    expect(cached.mix).toEqual(decoded.mix);
    // The job pinned the entry it listed (the media wiring's end hook releases it).
    expect(await cache.release(`job:${v.id}`)).toBe(1);
  }, 240_000);
});

describe("the job's pins end with the job (the wiring's hook, review S4)", () => {
  /** A project whose asset has a pcm48 entry, the wiring started over its engine; `fp` picks the worker's test hook. */
  async function pinned(fp: string) {
    const s = await setup(fp);
    const pcm = makePcmService({ registry: s.registry, engine: s.t.engine, cache: () => s.cache });
    const made = await run(pcm.request(OWNER, { projectId: s.pid, asset: s.asset }));
    const wiring = mediaWiring({ cache: s.cache } as unknown as MediaCacheService, { onDrop: () => {} }, () => {});
    wiring.start(s.t.engine, { registerStage: () => {}, registry: { list: () => [] } } as never, s.registry as never);
    const jobPins = async () => (await s.cache.pinsOf(made.key)).filter((p) => p.startsWith("job:"));
    const submit = () => s.t.submit(MEDIA_LISTEN, { projectId: s.pid, branch: "main", rev: 2, timeline: "tl1", from: "0", to: "3" }, { project: s.pid });
    return { ...s, key: made.key, jobPins, submit };
  }

  test("released when it succeeds", async () => {
    const s = await pinned("test-slow-tone");
    const v = await s.submit();
    await vi.waitFor(async () => expect(await s.jobPins()).toEqual([`job:${v.id}`]), { timeout: 30_000, interval: 20 });
    expect((await s.t.wait(v.id, 120_000)).state).toBe("succeeded");
    await vi.waitFor(async () => expect(await s.jobPins()).toEqual([]), { timeout: 10_000 });
  }, 240_000);

  test("released when it is cancelled", async () => {
    const s = await pinned("test-slow-tone");
    const v = await s.submit();
    await vi.waitFor(async () => expect(await s.jobPins()).toEqual([`job:${v.id}`]), { timeout: 30_000, interval: 20 });
    await run(s.t.engine.cancel(v.id));
    expect((await s.t.wait(v.id, 120_000)).state).toBe("cancelled");
    await vi.waitFor(async () => expect(await s.jobPins()).toEqual([]), { timeout: 10_000 });
  }, 240_000);

  test("released when it fails", async () => {
    // Test builds of the worker abort on a `test-crash-` fingerprint after the first block.
    const s = await pinned("test-crash-tone");
    const v = await s.submit();
    const done = await s.t.wait(v.id, 120_000);
    expect(done.state, JSON.stringify(done)).toBe("failed");
    await vi.waitFor(async () => expect(await s.jobPins()).toEqual([]), { timeout: 10_000 });
  }, 240_000);
});
