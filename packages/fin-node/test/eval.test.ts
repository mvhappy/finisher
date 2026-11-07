// `evalFrame`, `evalAudio` and `fontsPlan` (plan 4C Task 22).
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AudioPlan, FontsPlan, FramePlan } from "@finisher/core-types";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError } from "../index.js";

let tmp: string;
let engine: Engine | undefined;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "fin-node-eval-"));
});

afterEach(async () => {
  await engine?.close();
  engine = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

const tx = (txId: string, baseRev: number, ops: unknown[]) =>
  JSON.stringify({ txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops });

const asset = {
  op: "asset.add",
  id: "$a",
  path: "/media/a.mov",
  duration: "600",
  video: { width: 1920, height: 1080, rate: "30" },
  audio: { channels: 2, sampleRate: 48000 },
  fingerprint: "fp",
};
const clip = { op: "clip.insert", id: "$c", track: "t1", at: "0", asset: "a1", sourceIn: "0", duration: "5" };
const captions = [
  { op: "track.add", id: "$k", timeline: "tl1", kind: "caption" },
  {
    op: "item.insert",
    id: "$w",
    track: "$k",
    at: "0",
    duration: "4",
    mode: "ripple",
    kind: "caption",
    style: "preset:bold",
    words: [
      { text: "Hello", start: "0", end: "1" },
      { text: "world", start: "1", end: "2" },
    ],
  },
];

async function rejection(p: Promise<unknown>): Promise<FinNodeError> {
  try {
    await p;
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a rejection");
}

async function project(): Promise<Engine> {
  engine = await Engine.create(join(tmp, "sd"), join(tmp, "p"), "Demo", "30");
  await engine.commit(tx("t1", 0, [asset, clip, ...captions]));
  return engine;
}

describe("eval", () => {
  test("evalFrame plans the main timeline; captions are unmeasured without pinned fonts", async () => {
    const e = await project();
    const plan = JSON.parse(await e.evalFrame("main", null, "1")) as FramePlan;
    expect(plan.timeline).toBe("tl1");
    expect(plan.t).toBe("1");
    const kinds = plan.layers.map((l) => l.kind);
    expect(kinds).toEqual(["clip", "caption"]);
    const caption = plan.layers[1]!.caption!;
    expect(caption.layout).toBeNull();
    expect(caption.unmeasured?.reason).toBe("fontUnpinned");
    expect(caption.words.map((w) => [w.text, w.active])).toEqual([
      ["Hello", false],
      ["world", true],
    ]);
    // The same plan by timeline id, and an unknown timeline.
    expect(JSON.parse(await e.evalFrame("main", "tl1", "1"))).toEqual(plan);
    expect((await rejection(e.evalFrame("main", "tl9", "1"))).code).toBe("NOT_FOUND");
    expect((await rejection(e.evalFrame("main", null, "soon"))).code).toBe("INVALID_TIME");
    expect((await rejection(e.evalFrame("nope", null, "1"))).code).toBe("BRANCH_NOT_FOUND");
  });

  test("evalAudio plans a range", async () => {
    const e = await project();
    const plan = JSON.parse(await e.evalAudio("main", null, "0", "10")) as AudioPlan;
    expect(plan.timeline).toBe("tl1");
    expect((await rejection(e.evalAudio("main", null, "3", "1"))).code).toBe("INVALID_RANGE");
  });

  test("fontsPlan applies the ops to a copy and plans no pins before captions have styles", async () => {
    const e = await project();
    const plan = JSON.parse(await e.fontsPlan("main", JSON.stringify([{ ...clip, at: "5" }]))) as FontsPlan;
    expect(plan).toEqual({ ops: [], notes: [] });
    // Nothing was committed.
    expect(await e.head("main")).toBe(1);
    expect((await rejection(e.fontsPlan("main", JSON.stringify([{ op: "clip.delete", clip: "c9" }])))).code).toBe("NOT_FOUND");
    expect((await rejection(e.fontsPlan("main", "{"))).code).toBe("INVALID_OP");
  });
});
