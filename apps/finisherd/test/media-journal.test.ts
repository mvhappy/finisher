// `job.done`'s `media` object for the audio kinds (6C Task 22): `media.pcm` and `media.waveform`
// are batches, so `mediaCombine` keeps their run's `media`; `media.listen` answers
// `{…, stats: {media}}` as `media.look` does. One hook reads both, only if it is an object.
import type { JobView } from "@finisher/contracts";
import { describe, expect, it } from "vitest";
import { mediaCombine, mediaJournal } from "../src/jobs/kinds/media.journal.ts";

const media = { codec: "aac", backend: "symphonia", hw: false, resolution: "sd", frames: 47, rtFactorMilli: 90000, cacheHit: false, copyPath: "copy" };
const view = (result: unknown): JobView => ({ result }) as unknown as JobView;

describe("media.journal", () => {
  it("keeps an audio batch run's media beside the default batch result", () => {
    const items = new Map([["fp", { value: { key: "k" } }]]);
    const out = mediaCombine([{ made: 1, media }], items);
    expect(out).toEqual({ items: { fp: { key: "k" } }, itemErrors: [], media });
    expect(mediaJournal(view(out))).toEqual({ media });
  });

  it("reads a listen result's stats.media, and nothing else", () => {
    expect(mediaJournal(view({ stats: { media } }))).toEqual({ media });
    expect(mediaJournal(view({ mix: {}, tracks: [], stats: { media } }))).toEqual({ media });
    for (const bad of [null, {}, { stats: {} }, { stats: { media: "x" } }, { stats: { media: [1] } }]) expect(mediaJournal(view(bad))).toEqual({});
  });
});
