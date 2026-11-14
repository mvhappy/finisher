// `job.done`'s `media` object for the audio kinds (6C Task 22): a batch kind keeps the worker's
// `stats` through `combine`, and `mediaJournal` passes the object on, only if it is an object.
import type { JobView } from "@finisher/contracts";
import { describe, expect, it } from "vitest";
import { combineWithStats, mediaJournal } from "../src/jobs/kinds/media.journal.ts";

const media = { codec: "aac", backend: "symphonia", hw: false, resolution: "sd", frames: 47, rtFactorMilli: 90000, cacheHit: false, copyPath: "copy" };
const view = (result: unknown): JobView => ({ result }) as unknown as JobView;

describe("media.journal", () => {
  it("keeps the last run's stats beside the default batch result", () => {
    const items = new Map([["fp", { value: { key: "k" } }]]);
    const out = combineWithStats([{ made: 1, stats: { media: { frames: 1 } } }, { made: 1, stats: { media } }], items) as Record<string, unknown>;
    expect(out).toEqual({ items: { fp: { key: "k" } }, itemErrors: [], stats: { media } });
    expect(combineWithStats([{ made: 1 }], items)).toEqual({ items: { fp: { key: "k" } }, itemErrors: [] });
  });

  it("passes the media object on, and nothing else", () => {
    expect(mediaJournal(view({ stats: { media } }))).toEqual({ media });
    expect(mediaJournal(view({ items: {}, stats: { media } }))).toEqual({ media });
    for (const bad of [null, {}, { stats: {} }, { stats: { media: "x" } }, { stats: { media: [1] } }]) expect(mediaJournal(view(bad))).toEqual({});
  });
});
