// Helpers of the "listen" e2e scenario (6C Task 22): the WAV fixture (tones and a silence), the
// Opus/AAC MP4 writer (`make_audio_fixture`), the golden and its tolerances, and the leak check.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { ROOT } from "./lib.mjs";

export const GOLDEN_FILE = fileURLToPath(new URL("./listen-golden/listen.json", import.meta.url));

/** The tolerances of the scenario (plan 6C Task 22): loudness ±0.2 LU, true peak ±0.2 dB, silences exact at the 10 ms detector. */
export const TOL = { lu: 0.2, db: 0.2 };

/** Whether the golden is rewritten: `E2E_LISTEN_BLESS=1`, refused under `CI`. */
export function blessing(env = process.env) {
  if (env.E2E_LISTEN_BLESS !== "1") return false;
  if (env.CI) throw new Error("E2E_LISTEN_BLESS is refused under CI: bless locally and commit the golden");
  return true;
}

/** The WAV's segments: a 1 kHz tone, a digital silence, a 440 Hz tone (seconds, Hz, peak amplitude; Hz 0 is silence). */
export const WAV_SEGMENTS = [
  { secs: 2, hz: 1000, peak: 0.1 },
  { secs: 1, hz: 0, peak: 0 },
  { secs: 2, hz: 440, peak: 0.2 },
];

/** Writes a 16-bit stereo 48 kHz WAV of `segments` to `path`; returns its length in seconds. */
export function writeWav(path, segments = WAV_SEGMENTS) {
  const rate = 48_000;
  const total = segments.reduce((n, s) => n + s.secs * rate, 0);
  const data = Buffer.alloc(total * 4);
  let at = 0;
  for (const s of segments) {
    for (let i = 0; i < s.secs * rate; i++) {
      const v = s.hz === 0 ? 0 : Math.round(s.peak * Math.sin((2 * Math.PI * s.hz * i) / rate) * 32767);
      data.writeInt16LE(v, (at + i) * 4);
      data.writeInt16LE(v, (at + i) * 4 + 2);
    }
    at += s.secs * rate;
  }
  const h = Buffer.alloc(44);
  h.write("RIFF", 0, "latin1");
  h.writeUInt32LE(36 + data.length, 4);
  h.write("WAVEfmt ", 8, "latin1");
  h.writeUInt32LE(16, 16);
  h.writeUInt16LE(1, 20);
  h.writeUInt16LE(2, 22);
  h.writeUInt32LE(rate, 24);
  h.writeUInt32LE(rate * 4, 28);
  h.writeUInt16LE(4, 32);
  h.writeUInt16LE(16, 34);
  h.write("data", 36, "latin1");
  h.writeUInt32LE(data.length, 40);
  writeFileSync(path, Buffer.concat([h, data]));
  return total / rate;
}

/** `make_audio_fixture`: an audio-only MP4 of a sine (`opus` everywhere, `aac` on Linux). */
export function makeAudioMp4(path, codec, { seconds = 3, hz = 880, dbfs = -20 } = {}) {
  const r = spawnSync("cargo", ["run", "-q", "--release", "-p", "fin-codecs", "--features", "testing", "--example", "make_audio_fixture", "--", path, "--codec", codec, "--seconds", String(seconds), "--hz", String(hz), "--dbfs", String(dbfs)], { cwd: ROOT, encoding: "utf8", maxBuffer: 64 << 20 });
  if (r.status !== 0) throw new Error(`make_audio_fixture ${codec} failed (exit ${r.status}): ${r.stderr}`);
  return path;
}

const num = (x) => (typeof x === "number" ? x : null);

/** The numbers of one `ListenStats` the golden holds. */
export function digest(stats) {
  return {
    integrated: num(stats.integrated),
    truePeak: num(stats.truePeak),
    silences: (stats.silences ?? []).map((s) => ({ from: String(s.from), to: String(s.to) })),
  };
}

/** The scenario's answer as the golden stores it: the mix and each track by id. */
export function goldenOf(listen) {
  return { mix: digest(listen.mix), tracks: Object.fromEntries(listen.tracks.map((t) => [t.track, digest(t.stats)])) };
}

/** The difference between two digests ("" when equal within the tolerances). */
export function compare(got, want, where) {
  const issues = [];
  for (const [key, tol] of [["integrated", TOL.lu], ["truePeak", TOL.db]]) {
    const [g, w] = [got[key], want[key]];
    if (g === null || w === null ? g !== w : Math.abs(g - w) > tol) issues.push(`${where}.${key}: ${g} vs golden ${w} (±${tol})`);
  }
  if (JSON.stringify(got.silences) !== JSON.stringify(want.silences)) issues.push(`${where}.silences: ${JSON.stringify(got.silences)} vs golden ${JSON.stringify(want.silences)} (exact)`);
  return issues;
}

/** Checks `listen` against the golden (or rewrites it when blessing). */
export function checkGolden(listen, log) {
  const got = goldenOf(listen);
  if (blessing()) {
    writeFileSync(GOLDEN_FILE, `${JSON.stringify(got, null, 2)}\n`);
    log(`blessed ${GOLDEN_FILE}`);
    return;
  }
  if (!existsSync(GOLDEN_FILE)) throw new Error(`no golden at ${GOLDEN_FILE}; bless one with E2E_LISTEN_BLESS=1`);
  const want = JSON.parse(readFileSync(GOLDEN_FILE, "utf8"));
  const issues = [...compare(got.mix, want.mix, "mix")];
  for (const id of new Set([...Object.keys(got.tracks), ...Object.keys(want.tracks)])) {
    if (!got.tracks[id] || !want.tracks[id]) issues.push(`track ${id} is ${got.tracks[id] ? "not in the golden" : "missing"}`);
    else issues.push(...compare(got.tracks[id], want.tracks[id], `tracks.${id}`));
  }
  if (issues.length > 0) throw new Error(`listen differs from the golden:\n  ${issues.join("\n  ")}`);
}

/** Absolute paths (POSIX or Windows) or file names inside `text`: what no diagnostics payload may hold. */
export const PATH_LIKE = /(?:^|[\s"'])(?:\/(?:Users|home|tmp|var|private|Volumes)\/|[A-Za-z]:[\\/])|\.(?:wav|mp4|opus|m4a)\b/i;

/** The pcm and wave entries `fin cache status` counts (its `entries`). */
export const cacheEntries = (status) => status.entries ?? 0;

export { join };
