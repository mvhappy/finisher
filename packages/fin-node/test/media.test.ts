// Media job inputs (6A plan Task 12): `Engine.prepareMediaJob` writes a job's snapshot,
// manifest and params, and `commitJobOutput` moves a finished file out of a job directory.
import { closeSync, linkSync, mkdirSync, openSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { blake3Hex, commitJobOutput, Engine, type FinNodeError } from "../index.js";

let tmp: string;
let engine: Engine | undefined;

beforeEach(() => {
  // Resolved: no segment of a job path may be a link, and macOS answers `/var/…` for `/private/var/…`.
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "fin-node-media-")));
  process.env.FIN_FONTS_OFFLINE = "1";
});

afterEach(async () => {
  await engine?.close();
  engine = undefined;
  delete process.env.FIN_FONTS_OFFLINE;
  rmSync(tmp, { recursive: true, force: true });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) =>
  JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

const video = { width: 160, height: 90, rate: "30" };
const addAsset = (id: string, path: string) => ({ op: "asset.add", id, path, duration: "10", video, fingerprint: `fp-${id.slice(1)}` });
const clip = (id: string, asset: string, at: string) => ({ op: "clip.insert", id, track: "t1", at, asset, sourceIn: "0", duration: "5" });

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

interface Manifest {
  version: number;
  assets: { asset: string; kind: string; fingerprint: string; original: null | { path: string; size: number; mtimeNs: string; fileId?: string } }[];
  fonts: { cache?: string; files: unknown[] };
}

/** A project: a1 (a file) at 0–5 s, a2 (no file) at 5–10 s, a3 (a file, never used), a4 (a relative path). */
async function project(): Promise<{ e: Engine; dir: string; a1: string; inDir: string }> {
  const dir = join(tmp, "p");
  const e = (engine = await Engine.create(join(tmp, "sd"), dir, "Demo", "30"));
  const a1 = join(tmp, "a1.mp4");
  writeFileSync(a1, "twelve bytes");
  writeFileSync(join(tmp, "a3.mp4"), "unused");
  mkdirSync(join(dir, "assets"), { recursive: true });
  writeFileSync(join(dir, "assets", "rel.mp4"), "relative");
  await e.commit(
    tx("t1", 0, [
      addAsset("$a1", a1),
      addAsset("$a2", join(tmp, "missing.mp4")),
      addAsset("$a3", join(tmp, "a3.mp4")),
      addAsset("$a4", "assets/rel.mp4"),
      clip("$c1", "a1", "0"),
      clip("$c2", "a2", "5"),
    ]),
  );
  const inDir = join(tmp, "job", "in");
  mkdirSync(inDir, { recursive: true });
  return { e, dir, a1, inDir };
}

const read = <T>(dir: string, name: string): T => JSON.parse(readFileSync(join(dir, name), "utf8")) as T;
const prepared = (json: string) => JSON.parse(json) as { rev: number; planSamples: { at: string; blake3: string }[]; sources: string[] };

describe("prepareMediaJob", () => {
  test("the manifest lists the assets the plans reference and nothing else", async () => {
    const { e, inDir, a1 } = await project();
    const first = prepared(await e.prepareMediaJob("main", "tl1", ["1"], inDir, { fontCache: join(tmp, "sd", "fonts") }));
    expect(first.rev).toBe(1);
    // The files the worker opens (plan 7C Task 7): the referenced assets' originals.
    expect(first.sources).toEqual([a1]);
    let manifest = read<Manifest>(inDir, "manifest.json");
    expect(manifest.version).toBe(1);
    expect(manifest.assets.map((a) => a.asset)).toEqual(["a1"]);
    expect(manifest.assets[0]).toMatchObject({ kind: "media", fingerprint: "fp-a1", original: { path: a1, size: 12 } });
    expect(manifest.fonts).toEqual({ cache: join(tmp, "sd", "fonts"), files: [] });
    // Both clips: the file that is not there is `null`, not an error.
    const both = prepared(await e.prepareMediaJob("main", "tl1", ["1", "6"], inDir));
    // A missing original has no path to grant.
    expect(both.sources).toEqual([a1]);
    manifest = read<Manifest>(inDir, "manifest.json");
    expect(manifest.assets.map((a) => [a.asset, a.original === null ? null : a.original.size])).toEqual([
      ["a1", 12],
      ["a2", null],
    ]);
    expect(manifest.fonts).toEqual({ files: [] });
    const original = manifest.assets[0]!.original!;
    expect(original.mtimeNs).toMatch(/^\d+$/);
    expect(statSync(a1).mtimeMs).toBeCloseTo(Number(BigInt(original.mtimeNs) / 1_000_000n), -1);
  });

  test("a relative stored path is read from the project folder", async () => {
    const { e, dir, inDir } = await project();
    await e.commit(tx("t2", 1, [clip("$c3", "a4", "10")]));
    await e.prepareMediaJob("main", "tl1", ["11"], inDir);
    const manifest = read<Manifest>(inDir, "manifest.json");
    expect(manifest.assets.map((a) => [a.asset, a.original?.path, a.original?.size])).toEqual([["a4", join(dir, "assets", "rel.mp4"), 8]]);
  });

  test("plan samples equal the plan evalFrame answers", async () => {
    const { e, inDir } = await project();
    const at = ["0", "7/2", "6", "19/2"];
    const { planSamples } = prepared(await e.prepareMediaJob("main", "tl1", at, inDir));
    expect(planSamples.map((s) => s.at)).toEqual(at);
    for (const s of planSamples) {
      const plan = await e.evalFrame("main", "tl1", s.at);
      expect(s.blake3).toBe(blake3Hex(Buffer.from(plan)));
    }
    expect(new Set(planSamples.map((s) => s.blake3)).size).toBe(4);
  });

  test("the snapshot is the engine's state and round-trips", async () => {
    const { e, inDir } = await project();
    const { rev } = prepared(await e.prepareMediaJob("main", "tl1", ["1"], inDir));
    const snap = read<{ branch: string; rev: number; project: unknown }>(inDir, "snapshot.json");
    const engineState = JSON.parse(await e.snapshot("main")) as { rev: number; project: unknown };
    expect(snap).toEqual({ branch: "main", rev, project: engineState.project });
    expect(rev).toBe(engineState.rev);
  });

  test("params.json is the job's params with the samples filled in", async () => {
    const { e, inDir } = await project();
    const params = { timeline: "tl1", at: ["1", "6"], size: { width: 64, height: 36 }, outName: "look", projectId: e.projectId, branch: "main", rev: 1 };
    const { planSamples } = prepared(await e.prepareMediaJob("main", "tl1", params.at, inDir, { paramsJson: JSON.stringify(params) }));
    expect(read(inDir, "params.json")).toEqual({ ...params, planSamples });
    expect(readdirSync(inDir).sort()).toEqual(["manifest.json", "params.json", "snapshot.json"]);
  });

  test("a time past the end is an empty frame", async () => {
    const { e, inDir } = await project();
    const { planSamples } = prepared(await e.prepareMediaJob("main", "tl1", ["99"], inDir));
    expect(JSON.parse(await e.evalFrame("main", "tl1", "99")).layers).toEqual([]);
    expect(planSamples).toHaveLength(1);
    expect(read<Manifest>(inDir, "manifest.json").assets).toEqual([]);
  });

  test("a failure writes nothing", async () => {
    const { e, inDir } = await project();
    // (`evalFrame` past the timeline's end is an empty plan, not an error.)
    expect((await rejection(e.prepareMediaJob("main", "tl9", ["1"], inDir))).code).toBe("NOT_FOUND");
    expect((await rejection(e.prepareMediaJob("main", "tl1", ["soon"], inDir))).code).toBe("INVALID_TIME");
    expect((await rejection(e.prepareMediaJob("nope", "tl1", ["1"], inDir))).code).toBe("BRANCH_NOT_FOUND");
    const bad = JSON.stringify({ timeline: "tl1", at: ["1"], outName: "../x" });
    expect((await rejection(e.prepareMediaJob("main", "tl1", ["1"], inDir, { paramsJson: bad }))).code).toBe("JOB_PARAMS_INVALID");
    expect((await rejection(e.prepareMediaJob("main", "tl1", ["1"], inDir, { paramsJson: "{" }))).code).toBe("JOB_PARAMS_INVALID");
    expect(readdirSync(inDir)).toEqual([]);
    // The directory must exist: a write failure is `SHM_REFUSED`, naming the file.
    const e2 = await rejection(e.prepareMediaJob("main", "tl1", ["1"], join(tmp, "nowhere")));
    expect([e2.code, (e2.details as { reason: string }).reason]).toEqual(["SHM_REFUSED", "missing"]);
  });

  // Review M3: the limit on a frame's side applies to the size drawn, named or not.
  test("a timeline larger than the limit needs a size", async () => {
    const { e, inDir } = await project();
    await e.commit(tx("t2", 1, [{ op: "timeline.set", timeline: "tl1", path: "resolution", value: [8200, 64] }]));
    const params = { timeline: "tl1", at: ["1"], outName: "look", projectId: e.projectId, branch: "main", rev: 2 };
    const err = await rejection(e.prepareMediaJob("main", "tl1", ["1"], inDir, { paramsJson: JSON.stringify(params) }));
    expect([err.code, (err.details as { field: string }).field]).toEqual(["JOB_PARAMS_INVALID", "size"]);
    expect(readdirSync(inDir)).toEqual([]);
    const sized = { ...params, size: { width: 640, height: 8 } };
    await e.prepareMediaJob("main", "tl1", ["1"], inDir, { paramsJson: JSON.stringify(sized) });
    expect(readdirSync(inDir).sort()).toEqual(["manifest.json", "params.json", "snapshot.json"]);
  });

  // Review L2: the inputs go in below no link.
  test.skipIf(process.platform === "win32")("an in/ that is a link is refused and nothing is written through it", async () => {
    const { e } = await project();
    const job = join(tmp, "job2");
    const elsewhere = join(tmp, "elsewhere");
    mkdirSync(job);
    mkdirSync(elsewhere);
    symlinkSync(elsewhere, join(job, "in"));
    const err = await rejection(e.prepareMediaJob("main", "tl1", ["1"], join(job, "in")));
    expect([err.code, (err.details as { reason: string }).reason]).toEqual(["SHM_REFUSED", "link"]);
    expect(readdirSync(elsewhere)).toEqual([]);
  });

  test("a second prepare replaces the files; a planted link is replaced, not followed", async () => {
    const { e, inDir } = await project();
    await e.prepareMediaJob("main", "tl1", ["1"], inDir);
    if (process.platform !== "win32") {
      const outside = join(tmp, "outside.json");
      writeFileSync(outside, "keep");
      rmSync(join(inDir, "manifest.json"));
      symlinkSync(outside, join(inDir, "manifest.json"));
      writeFileSync(join(inDir, "snapshot.json.tmp"), "junk");
      await e.prepareMediaJob("main", "tl1", ["1"], inDir);
      expect(readFileSync(outside, "utf8")).toBe("keep");
    }
    expect(read<Manifest>(inDir, "manifest.json").version).toBe(1);
    expect(readdirSync(inDir).sort()).toEqual(["manifest.json", "snapshot.json"]);
  });
});

describe("commitJobOutput", () => {
  const unix = process.platform !== "win32";

  test("moves a finished file, replacing the destination", async () => {
    const out = join(tmp, "job", "out");
    mkdirSync(out, { recursive: true });
    const dest = join(tmp, "dest");
    mkdirSync(dest);
    writeFileSync(join(out, "look-0.png"), "frame");
    writeFileSync(join(dest, "x.png"), "old");
    await commitJobOutput(join(out, "look-0.png"), join(dest, "x.png"), 1 << 20);
    expect(readFileSync(join(dest, "x.png"), "utf8")).toBe("frame");
    expect(readdirSync(out)).toEqual([]);
  });

  // Review B1: a warm worker is alive and may hold the file it wrote. `copy` gives the
  // destination a new inode, so what the worker writes later never reaches it.
  test.skipIf(!unix)("a copy commit is safe from a writer that still holds the file", async () => {
    const out = join(tmp, "job", "out");
    mkdirSync(out, { recursive: true });
    const dest = join(tmp, "dest");
    mkdirSync(dest);
    writeFileSync(join(out, "look-0.png"), "frame");
    const fd = openSync(join(out, "look-0.png"), "a");
    try {
      await commitJobOutput(join(out, "look-0.png"), join(dest, "x.png"), 1 << 20, true);
      expect(readdirSync(dest)).toEqual(["x.png"]);
      writeSync(fd, " and more");
      expect(readFileSync(join(dest, "x.png"), "utf8")).toBe("frame");
      expect(statSync(join(dest, "x.png")).nlink).toBe(1);
      // A rename (the default) hands the destination the writer's own file.
      writeFileSync(join(out, "look-1.png"), "frame");
      const fd2 = openSync(join(out, "look-1.png"), "a");
      await commitJobOutput(join(out, "look-1.png"), join(dest, "y.png"), 1 << 20);
      writeSync(fd2, " and more");
      closeSync(fd2);
      expect(readFileSync(join(dest, "y.png"), "utf8")).toBe("frame and more");
    } finally {
      closeSync(fd);
    }
    const link = join(out, "link.png");
    symlinkSync(join(dest, "x.png"), link);
    const err = await rejection(commitJobOutput(link, join(dest, "z.png"), 1 << 20, true));
    expect((err.details as { reason: string }).reason).toBe("link");
  });

  test("refuses what is not a plain file of the job, and a file over the cap", async () => {
    const out = join(tmp, "job", "out");
    mkdirSync(out, { recursive: true });
    const dest = join(tmp, "dest");
    mkdirSync(dest);
    writeFileSync(join(out, "big.png"), "0123456789");
    const over = await rejection(commitJobOutput(join(out, "big.png"), join(dest, "big.png"), 4));
    expect([over.code, (over.details as { reason: string }).reason]).toEqual(["SHM_REFUSED", "overCap"]);
    const gone = await rejection(commitJobOutput(join(out, "none.png"), join(dest, "none.png"), 4));
    expect((gone.details as { reason: string }).reason).toBe("missing");
    if (unix) {
      symlinkSync(join(out, "big.png"), join(out, "link.png"));
      linkSync(join(out, "big.png"), join(out, "hard.png"));
      for (const [name, reason] of [
        ["link.png", "link"],
        ["hard.png", "hardLinked"],
      ] as const) {
        const err = await rejection(commitJobOutput(join(out, name), join(dest, name), 1 << 20));
        expect([err.code, (err.details as { reason: string }).reason]).toEqual(["SHM_REFUSED", reason]);
      }
    }
    expect(readdirSync(dest)).toEqual([]);
    expect((await rejection(commitJobOutput("relative.png", join(dest, "r.png"), 4))).code).toBe("SHM_REFUSED");
  });
});
