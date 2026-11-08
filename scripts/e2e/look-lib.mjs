// Helpers of the "look" e2e scenario (6A Task 15): a minimal PNG reader, the frame-id code read
// back from a composited picture, the nearest-frame rule re-derived for the VFR fixture, the
// goldens (plan hashes; T0 hashes keyed by the fixtures' digest), and process helpers for
// killing the worker and copying a job directory aside.
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";
import { FIN, WIN, log } from "./lib.mjs";

export const GOLDEN_DIR = fileURLToPath(new URL("./look-golden", import.meta.url));

/** Whether goldens are rewritten: `E2E_LOOK_BLESS=1`, refused under `CI` like the reel's. */
export function blessing(env = process.env) {
  if (env.E2E_LOOK_BLESS !== "1") return false;
  if (env.CI) throw new Error("E2E_LOOK_BLESS is refused under CI: bless locally and commit the goldens");
  return true;
}

/** SHA-256 of the files' bytes in order: what identifies the fixture clips a T0 golden belongs to. */
export function filesDigest(paths) {
  const h = createHash("sha256");
  for (const p of paths) h.update(readFileSync(p));
  return h.digest("hex");
}

/** `{os}-{arch}` as the encode golden names a machine class. */
export const machineClass = () => `${process.platform === "win32" ? "windows" : process.platform === "darwin" ? "macos" : process.platform}-${process.arch === "x64" ? "x86_64" : process.arch === "arm64" ? "aarch64" : process.arch}`;

/** Decodes an 8-bit RGBA, non-interlaced PNG (what the worker writes): `{width, height, rgba}`. */
export function readPng(path) {
  const b = readFileSync(path);
  if (b.readUInt32BE(0) !== 0x89504e47) throw new Error(`${path}: not a PNG`);
  let pos = 8;
  let width = 0;
  let height = 0;
  const idat = [];
  while (pos < b.length) {
    const len = b.readUInt32BE(pos);
    const type = b.toString("latin1", pos + 4, pos + 8);
    const data = b.subarray(pos + 8, pos + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      if (data[8] !== 8 || data[9] !== 6 || data[12] !== 0) throw new Error(`${path}: not 8-bit RGBA, non-interlaced (depth ${data[8]}, colour ${data[9]})`);
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    pos += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const rgba = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const left = x >= 4 ? rgba[y * stride + x - 4] : 0;
      const up = y > 0 ? rgba[(y - 1) * stride + x] : 0;
      const upLeft = x >= 4 && y > 0 ? rgba[(y - 1) * stride + x - 4] : 0;
      let v = line[x];
      if (filter === 1) v += left;
      else if (filter === 2) v += up;
      else if (filter === 3) v += (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const [pa, pb, pc] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - upLeft)];
        v += pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
      } else if (filter !== 0) throw new Error(`${path}: PNG filter ${filter}`);
      rgba[y * stride + x] = v & 0xff;
    }
  }
  return { width, height, rgba };
}

/** CRC-8 (polynomial 0x07, initial 0) as `fin_media::testing::crc8`. */
function crc8(bytes) {
  let crc = 0;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i++) crc = crc & 0x80 ? ((crc << 1) ^ 0x07) & 0xff : (crc << 1) & 0xff;
  }
  return crc;
}

/**
 * Reads the frame-id code (`fin_media::testing`: 4 rows of 8 blocks, 16 px a block at 540 lines,
 * the grid's first block one block in from the picture's corner) from a composited picture.
 * `place(cx, cy)` maps a point of the coded 960×540 picture to the output's pixels and `scale`
 * is how many output pixels one coded pixel became. Returns the id, or `undefined` when the CRC
 * does not match.
 */
export function readFrameId({ width, height, rgba }, place, scale) {
  const radius = Math.floor((16 * scale) / 4);
  let bits = 0n;
  for (let b = 0; b < 32; b++) {
    const [row, col] = [Math.floor(b / 8), b % 8];
    const [cx, cy] = place(16 * (col + 1.5), 16 * (row + 1.5));
    let sum = 0;
    let n = 0;
    for (let dy = -radius; dy <= radius; dy++) {
      for (let dx = -radius; dx <= radius; dx++) {
        const x = Math.floor(cx) + dx;
        const y = Math.floor(cy) + dy;
        if (x < 0 || y < 0 || x >= width || y >= height) return undefined;
        const i = (y * width + x) * 4;
        sum += Math.floor((2126 * rgba[i] + 7152 * rgba[i + 1] + 722 * rgba[i + 2] + 5000) / 10_000);
        n++;
      }
    }
    if (sum >= 128 * n) bits |= 1n << BigInt(31 - b);
  }
  const id = Number(bits >> 8n);
  return crc8([(id >> 16) & 0xff, (id >> 8) & 0xff, id & 0xff]) === Number(bits & 0xffn) ? id : undefined;
}

/** Where an unrotated 960×540 clip lands in the 1080×1920 frame: "contain" at scale 9/8, centred vertically. */
export const placeFlat = (cx, cy) => [(1080 / 960) * cx, (1920 - 540 * (1080 / 960)) / 2 + (1080 / 960) * cy];

/**
 * Where a 960×540 clip tagged for a 90° clockwise display rotation lands in the 1080×1920
 * frame: shown as 540×960 it fills the frame at scale 2, and a coded point (cx, cy) goes to
 * (540 − cy, cx) there.
 */
export const placeRotated = (cx, cy) => [2 * (540 - cy), 2 * cx];

/** The VFR fixture's frame times in 1/30 s ticks: durations cycle 1, 1, 2, 1, 3 (`fin_codecs::fixture`). */
export function vfrPts(frames) {
  const cycle = [1, 1, 2, 1, 3];
  const out = [];
  let t = 0;
  for (let i = 0; i < frames; i++) {
    out.push(t);
    t += cycle[i % cycle.length];
  }
  return out;
}

/**
 * The frame `frame_at` picks at `ticks`/30 s in a clip whose frames start at `pts` (the same
 * ticks): the nearest start time, a tie going to the earlier frame (spec §10.2). Re-derived
 * here so the e2e does not ask the code under test.
 */
export function nearestFrame(pts, ticksNum, ticksDen = 1) {
  // |pts[i] − t| compared as integers: scale everything by ticksDen.
  let best = 0;
  let bestDist = Infinity;
  for (let i = 0; i < pts.length; i++) {
    const d = Math.abs(pts[i] * ticksDen - ticksNum);
    if (d < bestDist) {
      best = i;
      bestDist = d;
    }
  }
  return best;
}

/** Process ids whose command line matches `pattern` (Windows through PowerShell, else `ps`). */
export function findProcs(pattern) {
  if (WIN) {
    const r = spawnSync("powershell", ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.CommandLine)\" }"], { encoding: "utf8" });
    return (r.stdout ?? "").split(/\r?\n/).filter((l) => pattern.test(l)).map((l) => Number(l.trim().split(" ")[0]));
  }
  const r = spawnSync("ps", ["-eo", "pid=,args="], { encoding: "utf8" });
  return (r.stdout ?? "").split("\n").filter((l) => pattern.test(l)).map((l) => Number(l.trim().split(/\s+/)[0]));
}

/** The warm `fin-workers media` worker processes (not a `--replay` or the watchdog). */
export const mediaWorkers = () => findProcs(/fin-workers(\.exe)?"?\s+media(\s|$)(?!.*--replay)/);

/** Runs `fin` without blocking the event loop: `{status, json, text}` when it exits. */
export function finAsync(serverData, cwd, args) {
  const env = { ...process.env, FIN_SERVER_DATA: serverData };
  delete env.FIN_SERVER;
  return new Promise((resolve, reject) => {
    const child = spawn(FIN, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    let err = "";
    child.stdout.setEncoding("utf8").on("data", (d) => (out += d));
    child.stderr.setEncoding("utf8").on("data", (d) => (err += d));
    child.once("error", reject);
    child.once("exit", (status) => {
      let json;
      try {
        json = JSON.parse(out.trim());
      } catch {
        json = undefined;
      }
      log(`fin ${args.join(" ")} (exit ${status}) → ${out.length > 160 ? `${out.slice(0, 160)}…` : out.trim()}`);
      resolve({ status, json, text: out.trim(), stderr: err });
    });
  });
}

/** Every `.png` below `dir` (the jobs root), however deep. */
export function pngsBelow(dir) {
  const out = [];
  const walk = (d) => {
    let names;
    try {
      names = readdirSync(d, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of names) {
      if (e.isDirectory()) walk(join(d, e.name));
      else if (e.name.endsWith(".png")) out.push(join(d, e.name));
    }
  };
  walk(dir);
  return out;
}

/**
 * Copies a running look's job directory aside as soon as its `in/` holds the snapshot, the
 * manifest and the params (the files `--replay` and `reference_render` read): `dest/in/…`.
 * Resolves with the job directory's path, or `undefined` when none showed up before `stop()`.
 */
export async function copyJobAside(jobsRoot, dest, stop) {
  while (!stop()) {
    for (const name of safeList(jobsRoot)) {
      const inDir = join(jobsRoot, name, "in");
      if (["snapshot.json", "manifest.json", "params.json"].every((f) => existsSync(join(inDir, f)))) {
        try {
          cpSync(inDir, join(dest, "in"), { recursive: true });
          if (["snapshot.json", "manifest.json", "params.json"].every((f) => existsSync(join(dest, "in", f)))) return join(jobsRoot, name);
        } catch {
          // The job finished and its directory went away mid-copy: the caller runs the look again.
        }
      }
    }
    await new Promise((r) => setTimeout(r, 5));
  }
  return undefined;
}

function safeList(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}
