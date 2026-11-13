// `asset.reprobe` (6B-1 plan Task 11): the RPC over a real project engine and the real probe
// `asset.add` uses (fin-node's `probe`), with a fake job engine for `deep`. An asset added as
// schema 4 stored it (coded size and rate only) gains its display geometry from one committed
// `asset.reprobe` record; a changed file, an undecodable codec (with `deep`), an unknown or
// non-media asset and a dry run commit nothing.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, JobView, Principal } from "@finisher/contracts";
import { Engine, probe, testHooks } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, describe, expect, test } from "vitest";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { toFinError } from "../src/errors.ts";
import { type ReprobeDeps, type ReprobeRequest, reprobe } from "../src/media/reprobe.ts";

let work: string;
const engines: Engine[] = [];
const OWNER = { kind: "person", id: "p1" } as const;
/** An unrestricted principal: the probe opens the path as `asset.probe` does for it. */
const LOCAL = { kind: "local", id: "local", human: true, projects: "*", scopes: [] } as unknown as Principal;
const META = { actor: { kind: "local", id: "local" } } as never;

beforeAll(() => {
  work = mkdtempSync(join(tmpdir(), "fin-reprobe-"));
});
afterEach(async () => {
  for (const e of engines.splice(0)) await e.close();
});
afterAll(() => rmSync(work, { recursive: true, force: true }));

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 3, ops });

interface Asset {
  readonly kind: string;
  readonly video?: { width: number; height: number; rate: string; display?: unknown; rotation?: number; pixelAspect?: string };
}

/** A project whose a1 is a portrait clip added the schema 4 way (no display, rotation or pixel aspect) and a2 an image. */
async function setup() {
  const serverData = mkdtempSync(join(work, "sd-"));
  const dir = join(mkdtempSync(join(work, "proj-")), "p");
  const clip = join(work, `portrait-${engines.length}.mp4`);
  testHooks!.writeFixture("mp4-r90", clip);
  const png = join(work, `still-${engines.length}.png`);
  testHooks!.writeFixture("png", png);
  const p = JSON.parse(await probe(clip));
  const i = JSON.parse(await probe(png));
  const { width, height, rate } = p.video;
  const engine = await Engine.create(serverData, dir, "P", "25");
  engines.push(engine);
  const add = { op: "asset.add", id: "$a", kind: "media", path: clip, fingerprint: p.fingerprint, duration: p.duration, video: { width, height, rate }, audio: p.audio };
  const still = { op: "asset.add", id: "$i", kind: "image", path: png, fingerprint: i.fingerprint, image: i.image };
  await engine.commit(tx("t-add", 0, [add, still]));
  const jobs = { submitted: [] as unknown[], result: { decodable: true, codec: "h264" } as unknown };
  const projects = { isOpen: () => true, dirOf: () => dir, resolveBranch: (_: string, b: string) => Effect.succeed(b), branches: () => Effect.succeed([]) } as ChainProjects;
  const deps: ReprobeDeps = {
    registry: {
      ensure: () => Effect.void,
      resolveBranch: (_: string, b: string) => Effect.succeed(b),
      snapshot: (_: string, b: string) => Effect.tryPromise({ try: async () => JSON.parse(await engine.snapshot(b)), catch: toFinError }),
      txGet: (_: string, id: string) => Effect.tryPromise({ try: async () => JSON.parse((await engine.txGet(id)) ?? "null"), catch: toFinError }),
      commit: (_: string, t: unknown) => Effect.tryPromise({ try: async () => JSON.parse(await engine.commit(JSON.stringify(t))), catch: toFinError }),
      dirOf: () => dir,
    },
    engine: {
      submit: (_owner: unknown, req: unknown) => {
        jobs.submitted.push(req);
        return Effect.succeed({ id: "j1" } as JobView);
      },
      wait: () => Effect.succeed([{ id: "j1", state: "succeeded", result: jobs.result } as unknown as JobView]),
    },
    capabilities: () => Effect.succeed({ worker: { renderVersion: 1 } }),
    projects,
  } as unknown as ReprobeDeps;
  const asset = async (id: string): Promise<Asset> => (JSON.parse(await engine.snapshot("main")).project.assets as Record<string, Asset>)[id]!;
  const rev = async (): Promise<number> => JSON.parse(await engine.snapshot("main")).rev;
  const run = (over: Partial<ReprobeRequest> = {}) =>
    Effect.runPromise(reprobe(deps, LOCAL, OWNER, META, { projectId: engine.projectId, asset: "a1", branch: "main", txId: "01reprobe0000000000000001", author: "test", ...over }));
  const fail = (over: Partial<ReprobeRequest> = {}) =>
    Effect.runPromise(Effect.flip(reprobe(deps, LOCAL, OWNER, META, { projectId: engine.projectId, asset: "a1", branch: "main", txId: "01reprobe0000000000000002", author: "test", ...over }))) as Promise<FinError>;
  return { engine, clip, jobs, asset, rev, run, fail };
}

describe("asset.reprobe", () => {
  test("reprobe_fills_rotation_of_portrait_clip", async () => {
    const { engine, asset, rev, run } = await setup();
    const before = await asset("a1");
    expect(before.video).toEqual({ width: 320, height: 180, rate: "25" });
    const r0 = await rev();
    const r = (await run()) as { asset: string; video: Asset["video"]; rev: number; txId: string };
    expect(r).toMatchObject({ asset: "a1", rev: r0 + 1, txId: "01reprobe0000000000000001" });
    const want = { width: 320, height: 180, rate: "25", display: { width: 180, height: 320 }, rotation: 90, pixelAspect: "1" };
    expect(r.video).toEqual(want);
    expect((await asset("a1")).video).toEqual(want);
    // One record, of the one op, at the current schema (6: the audio effect kinds).
    const records = JSON.parse(await engine.opsSince("main", r0)) as { tx: { schemaVersion: number; ops: { op: string }[] } }[];
    expect(records).toHaveLength(1);
    expect(records[0]!.tx.schemaVersion).toBe(6);
    expect(records[0]!.tx.ops.map((o) => o.op)).toEqual(["asset.reprobe"]);
  }, 60_000);

  test("a retry with the same txId answers from the log, even after the file changed", async () => {
    const { clip, jobs, rev, run } = await setup();
    const first = (await run({ deep: true })) as { rev: number; video: unknown };
    expect(jobs.submitted).toHaveLength(1);
    // The file moves on and the job queue would refuse: the retry neither probes nor submits.
    testHooks!.writeFixtureMp4(clip, { seconds: 3 });
    const again = (await run({ deep: true })) as { rev: number; txId: string; video: unknown };
    expect(again).toMatchObject({ rev: first.rev, txId: "01reprobe0000000000000001", video: first.video });
    expect(jobs.submitted).toHaveLength(1);
    expect(await rev()).toBe(first.rev);
  }, 60_000);

  test("a reprobe that finds the streams recorded commits nothing", async () => {
    const { rev, run } = await setup();
    const first = (await run()) as { rev: number };
    const same = (await run({ txId: "01reprobe0000000000000003" })) as { unchanged: boolean; rev: number; txId?: string };
    expect(same).toMatchObject({ unchanged: true, rev: first.rev });
    expect(same.txId).toBeUndefined();
    expect(await rev()).toBe(first.rev);
  }, 60_000);

  test("a changed file is ASSET_CHANGED and commits nothing", async () => {
    const { clip, rev, fail } = await setup();
    const r0 = await rev();
    testHooks!.writeFixtureMp4(clip, { seconds: 3 });
    const e = await fail();
    expect(e).toMatchObject({ code: "ASSET_CHANGED", details: { asset: "a1", reason: "fingerprint" } });
    expect(await rev()).toBe(r0);
  }, 60_000);

  test("deep: an undecodable asset is CODEC_UNAVAILABLE; a decodable one commits with the answer", async () => {
    const { jobs, rev, run, fail } = await setup();
    const r0 = await rev();
    jobs.result = { decodable: false, codec: "h264", failure: { code: "CODEC_UNAVAILABLE", message: "no decoder" } };
    const e = await fail({ deep: true });
    expect(e).toMatchObject({ code: "CODEC_UNAVAILABLE", details: { asset: "a1", codec: "h264", direction: "decode" } });
    expect(await rev()).toBe(r0);
    expect(jobs.submitted).toHaveLength(1);
    jobs.result = { decodable: true, codec: "h264", backend: "x" };
    const ok = (await run({ deep: true })) as { deep: unknown; rev: number };
    expect(ok.deep).toEqual(jobs.result);
    expect(ok.rev).toBe(r0 + 1);
  }, 60_000);

  test("an unknown or non-media asset is refused, and a dry run commits nothing", async () => {
    const { asset, rev, run, fail } = await setup();
    const r0 = await rev();
    expect((await fail({ asset: "a9" })).code).toBe("ASSET_NOT_FOUND");
    expect((await fail({ asset: "a2" })).code).toBe("ASSET_TYPE_UNSUPPORTED");
    expect((await fail({ asset: "constructor" })).code).toBe("ASSET_NOT_FOUND");
    const dry = (await run({ dryRun: true })) as { dryRun: boolean; video: Asset["video"] };
    expect(dry.dryRun).toBe(true);
    expect(dry.video?.rotation).toBe(90);
    expect(await rev()).toBe(r0);
    expect((await asset("a1")).video?.rotation).toBeUndefined();
  }, 60_000);
});
