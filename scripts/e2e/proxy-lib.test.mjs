import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { h264Guidance, h264Mode, h264Row, presentDiagnostics, task19Diagnostics, writeH264Stub } from "./proxy-lib.mjs";

const v1 = (decode, encode) => ({ codecs: { video: [{ codec: "av1", decode: "software", encode: "software", backends: ["dav1d"] }, { codec: "h264", decode, encode, backends: [] }] } });

test("the H.264 row of today's report and of a v2 one", () => {
  assert.deepEqual(h264Row(v1("none", "none")), { decode: false, encode: false, guidance: [] });
  assert.deepEqual(h264Row(v1("os", "os")), { decode: true, encode: true, guidance: [] });
  const later = { code: "laterBatch", batch: "6B-2", fix: "arrives with VA-API" };
  const v2 = { codecs: [{ codec: "h264", profile: "high", decode: "none", encode: "none", backends: [], guidance: [later] }] };
  assert.deepEqual(h264Row(v2), { decode: false, encode: false, guidance: [later] });
  assert.deepEqual(h264Row({}), { decode: false, encode: false, guidance: [] });
});

test("the H.264 mode by platform: encode where reported, refuse on Linux, skip elsewhere unless required", () => {
  const none = h264Row(v1("none", "none"));
  const os = h264Row(v1("os", "os"));
  assert.equal(h264Mode(os, "darwin", {}), "encode");
  assert.equal(h264Mode(os, "win32", {}), "encode");
  assert.equal(h264Mode(none, "linux", { FIN_REQUIRE_PLATFORM_CODECS: "1" }), "refuse");
  assert.equal(h264Mode(none, "darwin", {}), "skip");
  assert.equal(h264Mode(none, "win32", { FIN_REQUIRE_PLATFORM_CODECS: "0" }), "skip");
  assert.throws(() => h264Mode(none, "win32", { FIN_REQUIRE_PLATFORM_CODECS: "1" }), /FIN_REQUIRE_PLATFORM_CODECS=1/);
  // A decoder without an encoder cannot write the clip.
  assert.throws(() => h264Mode(h264Row(v1("os", "none")), "darwin", { FIN_REQUIRE_PLATFORM_CODECS: "1" }), /no H\.264 encoder/);
});

test("the container-only H.264 file: an avc1 entry and a chunk offset at the mdat payload", () => {
  const dir = mkdtempSync(join(tmpdir(), "proxy-lib-"));
  try {
    const path = join(dir, "h.mp4");
    writeH264Stub(path, { frames: 3 });
    const b = readFileSync(path);
    const at = (kind) => b.indexOf(Buffer.from(kind, "latin1"));
    assert.ok(at("avc1") > 0 && at("avcC") > at("avc1"));
    const stco = at("stco");
    const offset = b.readUInt32BE(stco + 12);
    assert.equal(b.toString("latin1", offset - 4, offset), "mdat");
    assert.equal(b.length - offset, 3 * 8);
    // Box sizes add up to the file.
    let pos = 0;
    while (pos < b.length) pos += b.readUInt32BE(pos);
    assert.equal(pos, b.length);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

const done = (kind, extra = {}) => ({ kind: "job.done", kv: 2, payload: { kind, outcome: "succeeded", ...extra } });
const media = (frames) => ({ codec: "av1", backend: "svt-av1", hw: false, resolution: "960x540", frames, rtFactorMilli: 4000, cacheHit: false, copyPath: "copy" });

test("today's diagnostics: capabilities, proxy and thumbs jobs, no path", () => {
  const caps = { kind: "media.capabilities", kv: 1, payload: {} };
  const ok = [caps, done("media.proxy"), done("media.proxy"), done("media.thumbs")];
  assert.equal(presentDiagnostics(ok, { proxyJobs: 2 }).length, 2);
  assert.throws(() => presentDiagnostics(ok, { proxyJobs: 3 }), /expected at least 3/);
  assert.throws(() => presentDiagnostics(ok.slice(1), { proxyJobs: 2 }), /no media.capabilities/);
  assert.throws(() => presentDiagnostics([...ok, done("media.proxy", { note: "/home/x/a.mp4" })], { proxyJobs: 2 }), /names an adapter or a path/);
});

test("the Task 19 assertions hold on the shape the plan names and fail on today's", () => {
  const later = { code: "laterBatch", batch: "6B-2", fix: "arrives with VA-API" };
  const caps = { kind: "media.capabilities", kv: 2, payload: { sandbox: "landlock", codecs: [{ codec: "h264", guidance: [later] }] } };
  const good = [caps, done("media.proxy", { media: media(90) }), done("media.proxy", { media: media(60) }), done("media.thumbs", { media: media(3) })];
  task19Diagnostics(good, { platform: "linux", proxyFrames: [90, 60] });
  assert.throws(() => task19Diagnostics([{ ...caps, kv: 1 }, ...good.slice(1)], { platform: "linux", proxyFrames: [90] }), /no media.capabilities v2/);
  assert.throws(() => task19Diagnostics([caps, done("media.proxy")], { platform: "linux", proxyFrames: [90] }), /no media object/);
  assert.throws(() => task19Diagnostics(good, { platform: "linux", proxyFrames: [30] }), /with 30 frames/);
  h264Guidance({ guidance: [later] }, { error: { code: "CODEC_UNAVAILABLE", details: { guidance: [later] } } });
  assert.throws(() => h264Guidance({ guidance: [] }, {}), /no laterBatch/);
  assert.throws(() => h264Guidance({ guidance: [later] }, { error: { details: { codec: "h264" } } }), /CODEC_UNAVAILABLE without/);
});
