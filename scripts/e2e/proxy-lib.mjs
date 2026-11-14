// Helpers of the "proxy" scenario (6B-1 Task 20): the H.264 rows by platform, a container-only
// H.264 MP4 for the Linux refusal, the HTTP range check and the diagnostics assertions (the part
// of them that 6B-1 Task 19 adds is in `task19Diagnostics`, off until that task lands).
import { request } from "node:http";
import { writeFileSync } from "node:fs";
import { assertEqual } from "./lib.mjs";

/**
 * Whether 6B-1 Task 19 (`media.capabilities` v2 with its guidance codes, the `job.done.media`
 * object of `media.proxy` and `media.thumbs`) is on this branch: the scenario asserts
 * `task19Diagnostics`.
 */
export const TASK19 = true;

/**
 * Whether 6B-1 Task 8's capability probe answers the v2 report (guidance on its rows and on a
 * `CODEC_UNAVAILABLE` refusal). Task 19 journals v2 but the job still answers v1, so
 * `h264Guidance` waits on this. Turn it on in the commit that merges Task 8.
 */
export const TASK8 = false;

/**
 * The Linux H.264 guidance (plan Global Constraints: `CODEC_UNAVAILABLE` until 6B-2, with
 * guidance): the report's H.264 row and the refusal both carry `laterBatch` for 6B-2. Today no
 * code puts a guidance line in the answer (the worker answers the v1 report), so this waits on `TASK8`.
 */
export function h264Guidance(row, refusal) {
  const later = (lines) => Array.isArray(lines) && lines.some((g) => g?.code === "laterBatch" && g?.batch === "6B-2");
  if (!later(row.guidance)) throw new Error(`the report's H.264 row has no laterBatch 6B-2 guidance: ${JSON.stringify(row.guidance)}`);
  const error = refusal?.error;
  if (error?.code !== "CODEC_UNAVAILABLE" || !later(error.details?.guidance)) throw new Error(`CODEC_UNAVAILABLE without laterBatch 6B-2 guidance: ${JSON.stringify(error ?? {})}`);
}

const isObj = (v) => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The H.264 rows of this machine's capability report (`fin media caps`): `codecs.video[]` today,
 * `codecs[]` once the report is v2 (6B-1 Tasks 8 and 19). `{decode, encode, guidance}`: `decode`
 * and `encode` are true when a backend serves them (a level other than `none`); `guidance` the
 * rows' lines (only v2 has them).
 */
export function h264Row(caps) {
  const rows = Array.isArray(caps?.codecs) ? caps.codecs : Array.isArray(caps?.codecs?.video) ? caps.codecs.video : [];
  const h264 = rows.filter((c) => c.codec === "h264");
  const serves = (level) => typeof level === "string" && level !== "none" && level !== "unavailable";
  return {
    decode: h264.some((c) => serves(c.decode)),
    encode: h264.some((c) => serves(c.encode)),
    guidance: h264.flatMap((c) => c.guidance ?? []),
  };
}

/**
 * What the scenario does with H.264 here: `encode` (an OS encoder writes the clip and proxies it:
 * macOS VideoToolbox, Windows Media Foundation, once 6B-1 Tasks 5 and 7 report it), `refuse`
 * (Linux before 6B-2: a container-only H.264 file is `CODEC_UNAVAILABLE` with `laterBatch`), or
 * `skip` (macOS or Windows without an H.264 encoder in the report; `FIN_REQUIRE_PLATFORM_CODECS=1`
 * fails instead).
 */
export function h264Mode(row, platform = process.platform, env = process.env) {
  if (row.decode && row.encode) return "encode";
  if (platform === "linux") return "refuse";
  if (env.FIN_REQUIRE_PLATFORM_CODECS === "1") {
    throw new Error(`FIN_REQUIRE_PLATFORM_CODECS=1 but this ${platform} machine reports no H.264 ${row.decode ? "encoder" : "decoder"}`);
  }
  return "skip";
}

const u8 = (n) => Buffer.from([n & 0xff]);
const u16 = (n) => {
  const b = Buffer.alloc(2);
  b.writeUInt16BE(n);
  return b;
};
const u32 = (n) => {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n >>> 0);
  return b;
};
const box = (kind, ...body) => {
  const payload = Buffer.concat(body);
  return Buffer.concat([u32(8 + payload.length), Buffer.from(kind, "latin1"), payload]);
};
const fullBox = (kind, version, flags, ...body) => box(kind, u8(version), u8(flags >> 16), u16(flags & 0xffff), ...body);
const zeros = (n) => Buffer.alloc(n);
/** The unity matrix of `mvhd` and `tkhd`. */
const MATRIX = Buffer.concat([0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000].map(u32));

/**
 * A container-only H.264 MP4 at `path`: one `avc1` track (Baseline 3.0, an SPS and a PPS in its
 * `avcC`) of `frames` samples at 30 fps, `width`×`height`, whose samples are not H.264 slices. It
 * probes as H.264 and no decoder ever reads it: the Linux refusal needs only the sample entry, and
 * no media file is committed (plan Global Constraints).
 */
export function writeH264Stub(path, { width = 320, height = 180, frames = 30 } = {}) {
  const timescale = 30_000;
  const delta = 1_000;
  const sample = Buffer.from([0, 0, 0, 4, 0x65, 0x88, 0x84, 0x00]);
  const sps = Buffer.from([0x67, 0x42, 0x00, 0x1e, 0x95, 0xa8]);
  const pps = Buffer.from([0x68, 0xce, 0x3c, 0x80]);
  const avcC = box("avcC", Buffer.from([1, 0x42, 0x00, 0x1e, 0xff, 0xe1]), u16(sps.length), sps, u8(1), u16(pps.length), pps);
  const avc1 = box(
    "avc1",
    zeros(6), u16(1), // reserved, data reference index
    zeros(16), u16(width), u16(height),
    u32(0x00480000), u32(0x00480000), zeros(4), u16(1), // 72 dpi, reserved, one frame per sample
    zeros(32), u16(0x18), u16(0xffff), // compressor name, depth, pre-defined
    avcC,
  );
  const duration = frames * delta;
  const ftyp = box("ftyp", Buffer.from("isom", "latin1"), u32(0x200), Buffer.from("isomiso2avc1mp41", "latin1"));
  const stblWith = (chunkOffset) =>
    box(
      "stbl",
      fullBox("stsd", 0, 0, u32(1), avc1),
      fullBox("stts", 0, 0, u32(1), u32(frames), u32(delta)),
      fullBox("stss", 0, 0, u32(1), u32(1)),
      fullBox("stsc", 0, 0, u32(1), u32(1), u32(frames), u32(1)),
      fullBox("stsz", 0, 0, u32(sample.length), u32(frames)),
      fullBox("stco", 0, 0, u32(1), u32(chunkOffset)),
    );
  const moovWith = (chunkOffset) =>
    box(
      "moov",
      fullBox("mvhd", 0, 0, u32(0), u32(0), u32(timescale), u32(duration), u32(0x10000), u16(0x100), zeros(10), MATRIX, zeros(24), u32(2)),
      box(
        "trak",
        fullBox("tkhd", 0, 3, u32(0), u32(0), u32(1), zeros(4), u32(duration), zeros(8), u16(0), u16(0), u16(0), zeros(2), MATRIX, u32(width << 16), u32(height << 16)),
        box(
          "mdia",
          fullBox("mdhd", 0, 0, u32(0), u32(0), u32(timescale), u32(duration), u16(0x55c4), u16(0)),
          fullBox("hdlr", 0, 0, u32(0), Buffer.from("vide", "latin1"), zeros(12), Buffer.from("VideoHandler\0", "latin1")),
          box(
            "minf",
            fullBox("vmhd", 0, 1, zeros(8)),
            box("dinf", fullBox("dref", 0, 0, u32(1), fullBox("url ", 0, 1))),
            stblWith(chunkOffset),
          ),
        ),
      ),
    );
  // The chunk offset is the mdat payload's: the moov's size does not depend on its value.
  const offset = ftyp.length + moovWith(0).length + 8;
  const mdat = box("mdat", ...Array.from({ length: frames }, () => sample));
  writeFileSync(path, Buffer.concat([ftyp, moovWith(offset), mdat]));
}

/** `GET` (or `HEAD`) of an origin-relative `path` on the server's loopback listener: `{status, headers, body}`. */
export function httpGet(port, path, headers = {}, method = "GET") {
  return new Promise((resolve, reject) => {
    const req = request({ host: "127.0.0.1", port, path, method, headers: { host: `127.0.0.1:${port}`, ...headers }, agent: false }, (res) => {
      const chunks = [];
      res.on("data", (c) => chunks.push(c));
      res.on("end", () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks) }));
      res.on("error", reject);
    });
    req.on("error", reject);
    req.end();
  });
}

/** The `job.done` entries of `kind`, oldest first. */
export const jobDone = (entries, kind) => entries.filter((e) => e.kind === "job.done" && e.payload?.kind === kind);

/**
 * What the journal holds today (before Task 19): a `media.capabilities` entry, and a succeeded
 * `job.done` for each proxy job the scenario waited for (`proxyJobs`, at least) and for the strip.
 */
export function presentDiagnostics(entries, { proxyJobs }) {
  if (!entries.some((e) => e.kind === "media.capabilities")) throw new Error("no media.capabilities entry in the journal");
  const proxies = jobDone(entries, "media.proxy").filter((e) => e.payload.outcome === "succeeded");
  if (proxies.length < proxyJobs) throw new Error(`the journal holds ${proxies.length} succeeded media.proxy job.done entries, expected at least ${proxyJobs}`);
  if (!jobDone(entries, "media.thumbs").some((e) => e.payload.outcome === "succeeded")) throw new Error("no succeeded media.thumbs job.done in the journal");
  // No path, file name or adapter name in any media entry (plan Global Constraints, owner decision 7).
  const media = entries.filter((e) => e.kind === "media.capabilities" || (e.kind === "job.done" && String(e.payload?.kind).startsWith("media.")));
  const text = JSON.stringify(media);
  if (/llvmpipe|swiftshader|vulkan device|\/dev\/|\.mp4|\.png/i.test(text)) throw new Error(`a media diagnostics entry names an adapter or a path: ${text.slice(0, 400)}`);
  return proxies;
}

/**
 * The assertions 6B-1 Task 19 makes true (plan Task 19 and 20): `media.capabilities` is kind
 * version 2 with per-codec rows, their guidance codes and the sandbox state; each succeeded
 * `media.proxy` and `media.thumbs` `job.done` carries `media {codec, backend, hw, resolution,
 * frames, rtFactorMilli, cacheHit, copyPath}`; on Linux the H.264 row carries `laterBatch` 6B-2.
 * `proxyFrames`: the presented frame count of each AV1 proxy source (90 and 60 here).
 */
export function task19Diagnostics(entries, { platform = process.platform, proxyFrames }) {
  const caps = entries.filter((e) => e.kind === "media.capabilities");
  if (!caps.some((e) => e.kv === 2)) throw new Error(`no media.capabilities v2 entry (versions ${JSON.stringify(caps.map((e) => e.kv))})`);
  const v2 = caps.filter((e) => e.kv === 2).at(-1).payload;
  if (!Array.isArray(v2.codecs) || v2.codecs.length === 0) throw new Error(`media.capabilities v2 without codec rows: ${JSON.stringify(v2).slice(0, 300)}`);
  if (typeof v2.sandbox !== "string" && !isObj(v2.sandbox)) throw new Error("media.capabilities v2 without its sandbox state");
  if (platform === "linux") {
    const h264 = v2.codecs.filter((c) => c.codec === "h264").flatMap((c) => c.guidance ?? []);
    if (!h264.some((g) => g.code === "laterBatch" && g.batch === "6B-2")) throw new Error(`the journal's H.264 row has no laterBatch 6B-2 guidance: ${JSON.stringify(h264)}`);
  }
  // `rtFactorMilli` is 0 for thumbnails, which play nothing through (Task 19's `JobMedia`).
  const shape = (m, what, played = true) => {
    if (!isObj(m)) throw new Error(`${what}: no media object`);
    for (const [k, t] of [["codec", "string"], ["backend", "string"], ["hw", "boolean"], ["resolution", "string"], ["frames", "number"], ["rtFactorMilli", "number"], ["cacheHit", "boolean"], ["copyPath", "string"]]) {
      if (typeof m[k] !== t) throw new Error(`${what}: media.${k} is ${JSON.stringify(m[k])}, not a ${t}`);
    }
    if (!Number.isInteger(m.rtFactorMilli) || m.rtFactorMilli < (played ? 1 : 0)) throw new Error(`${what}: media.rtFactorMilli ${m.rtFactorMilli}`);
  };
  const proxies = jobDone(entries, "media.proxy").filter((e) => e.payload.outcome === "succeeded");
  for (const e of proxies) shape(e.payload.media, `media.proxy job.done ${e.payload.job ?? ""}`);
  const frames = proxies.map((e) => e.payload.media.frames).sort((a, b) => a - b);
  for (const n of new Set(proxyFrames)) {
    if (!frames.includes(n)) throw new Error(`no media.proxy job.done with ${n} frames (got ${JSON.stringify(frames)})`);
  }
  for (const e of proxies) {
    if (e.payload.media.cacheHit !== false) throw new Error("a media.proxy job that ran reports a cache hit");
  }
  for (const e of jobDone(entries, "media.thumbs").filter((x) => x.payload.outcome === "succeeded")) shape(e.payload.media, "media.thumbs job.done", false);
  assertEqual(proxies.every((e) => ["av1", "h264"].includes(e.payload.media.codec)), true, "media.proxy job.done codecs");
}
