// Fixtures for the media kinds (6A plan Task 12): an AV1 clip written by `fin-codecs`'
// `make_fixture` example (no media file is committed), and a project over it.
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Engine } from "@finisher/fin-node";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

/** Writes a 160×90 AV1 pattern clip of `frames` frames at 30 fps to `out` (built on first use). */
export function makeAv1Fixture(out: string, frames = 12): void {
  mkdirSync(dirname(out), { recursive: true });
  execFileSync("cargo", ["run", "-q", "-p", "fin-codecs", "--features", "testing", "--example", "make_fixture", "--", out, "--frames", String(frames), "--size", "160x90", "--rate", "30"], {
    cwd: ROOT,
    stdio: ["ignore", "ignore", "inherit"],
    timeout: 600_000,
  });
}

const tx = (txId: string, baseRev: number, ops: unknown[]) => JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

export interface LookProject {
  readonly engine: Engine;
  readonly dir: string;
  /** The asset files: `a1` is the clip (present), `a2` is missing. */
  readonly media: string;
  readonly missing: string;
}

/**
 * A project in `dir` over `serverData`: a1 (the AV1 file at `media`) on track t1 from 0 to
 * 0.4 s, and a2 (no file) from 2/5 to 4/5 s. Timeline `tl1` is 1080p by default.
 */
export async function lookProject(serverData: string, dir: string, media: string): Promise<LookProject> {
  const engine = await Engine.create(serverData, dir, "Look", "30");
  const missing = join(dir, "..", "gone.mp4");
  const video = { width: 160, height: 90, rate: "30" };
  const asset = (id: string, path: string) => ({ op: "asset.add", id, path, duration: "2/5", video, fingerprint: `fp-${id.slice(1)}` });
  const clip = (id: string, a: string, at: string) => ({ op: "clip.insert", id, track: "t1", at, asset: a, sourceIn: "0", duration: "2/5" });
  await engine.commit(tx("t1", 0, [asset("$a1", media), asset("$a2", missing), clip("$c1", "a1", "0"), clip("$c2", "a2", "2/5")]));
  writeFileSync(join(dir, ".keep"), "");
  return { engine, dir, media, missing };
}
