// The pinned Codex binary (sub-project 7E, Task 2; plan Decision 1).
//
//   node scripts/codex-pin.mjs [--check] [--target <t>] [--all]
//
// Reads `vendor/codex/pin.json` (version, per-target sha512 integrity, kept files). For each
// target it fetches the platform package `@openai/codex@<version>-<target>` (`npm pack`, so
// the registry is whatever npm is configured to use; no registry URL is written down here)
// into the git-ignored `.cache/codex/downloads/`, **verifies the sha512 integrity before it
// extracts anything**, and extracts only the kept files (`bin/codex`, `codex-path/rg`, with
// `.exe` on Windows) into `.cache/codex/<version>/<target>/`. Everything else in the package
// is never written: above all `codex-resources/bwrap` (LGPL-2.0-or-later, spec §17).
//
// For the host's target it then runs, under a throwaway `HOME`/`CODEX_HOME` and a minimal
// environment (never the user's `~/.codex`, never a sign-in), `app-server
// generate-json-schema` (stable and `--experimental`) and `features list`, and writes the
// committed snapshot `vendor/codex/<version>/{schema/, schema-experimental/, features.txt}`.
// `schema-experimental/` keeps only the two bundle files (Decision 2: the experimental
// bundle is read for `dynamicTools` and `item/tool/call`).
// `--check` writes nothing: it compares a fresh generation with the committed snapshot, and
// fails when the host's target is not among those selected (nothing would be compared).
// `LICENSE` and `NOTICE` come from the `rust-v<version>` tag of github.com/openai/codex (the
// npm packages carry neither).
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  closeSync, copyFileSync, createReadStream, existsSync, mkdirSync, mkdtempSync, openSync, readdirSync,
  readFileSync, renameSync, rmSync, statSync, writeFileSync, writeSync,
} from "node:fs";
import { basename, dirname, join, relative, sep } from "node:path";
import { createGunzip } from "node:zlib";
import { fileURLToPath } from "node:url";

export const TARGETS = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64", "win32-arm64"];
export const EXPERIMENTAL_FILES = ["codex_app_server_protocol.schemas.json", "codex_app_server_protocol.v2.schemas.json"];

export function readPin(root) {
  return JSON.parse(readFileSync(join(root, "vendor/codex/pin.json"), "utf8"));
}

export function hostTarget(platform = process.platform, arch = process.arch) {
  const t = `${platform}-${arch}`;
  return TARGETS.includes(t) ? t : undefined;
}

/** `sha512-<base64>` of a file, streamed. */
export function integrityOf(file) {
  return new Promise((resolve, reject) => {
    const h = createHash("sha512");
    createReadStream(file).on("data", (c) => h.update(c)).on("error", reject).on("end", () => resolve(`sha512-${h.digest("base64")}`));
  });
}

export async function verifyTarball(file, expected) {
  const got = await integrityOf(file);
  if (got !== expected) throw new Error(`integrity mismatch for ${basename(file)}: expected ${expected}, got ${got}`);
}

const octal = (b) => {
  if (b[0] & 0x80) return Number(b.readBigUInt64BE(b.length - 8) & 0x7fffffffffffffffn);
  return parseInt(b.toString("latin1").replace(/\0.*$/, "").trim() || "0", 8);
};
const cstr = (b) => b.toString("utf8").replace(/\0.*$/s, "");

function paxPath(body) {
  let rest = body;
  while (rest.length) {
    const sp = rest.indexOf(0x20);
    const len = Number(rest.subarray(0, sp).toString());
    if (!(len > 0)) return undefined;
    const rec = rest.subarray(sp + 1, len - 1).toString("utf8");
    if (rec.startsWith("path=")) return rec.slice(5);
    rest = rest.subarray(len);
  }
  return undefined;
}

/**
 * Verifies `file` against `integrity`, then streams the tarball and writes only the entries
 * `package/vendor/<triple>/<kept>` into `destDir`, flat by base name (`bin/codex` becomes
 * `codex`). Returns the names written. It fails closed, removing every file it opened: a kept
 * file that is missing, a kept destination reached twice (another entry, or another triple's
 * copy, of the same name), an archive cut short (inside a body, a header or a pax record) or
 * without its end-of-archive block, or bytes other than zeros after that block.
 */
export async function extractKept(file, destDir, kept, { integrity }) {
  await verifyTarball(file, integrity);
  const want = new Map();
  for (const k of kept) {
    if (want.has(k) || [...want.values()].includes(basename(k))) throw new Error(`kept files collide: ${k}`);
    want.set(k, basename(k));
  }
  mkdirSync(destDir, { recursive: true });
  const written = [];
  /** Every destination opened (written or not), removed on any failure. */
  const opened = new Set();
  let ended = false; // the end-of-archive zero block was read
  let state = "header";
  let hdr = Buffer.alloc(0);
  let remaining = 0;
  let pad = 0;
  let cur; // { fd, name } of a kept file being written
  let special; // { type, chunks } of a pax/longname record
  let override; // path from a preceding pax/longname record
  const endBody = () => {
    if (cur) {
      closeSync(cur.fd);
      written.push(cur.name);
      cur = undefined;
    }
    if (special) {
      const body = Buffer.concat(special.chunks);
      override = special.type === "x" ? paxPath(body) : cstr(body);
      special = undefined;
    }
    state = pad ? "pad" : "header";
  };
  try {
    for await (const chunk of createReadStream(file).pipe(createGunzip())) {
      let buf = chunk;
      while (buf.length) {
        if (ended) {
          if (!buf.every((b) => b === 0)) throw new Error(`${basename(file)} has data after the end of the archive`);
          break;
        }
        if (state === "header") {
          const take = buf.subarray(0, 512 - hdr.length);
          hdr = Buffer.concat([hdr, take]);
          buf = buf.subarray(take.length);
          if (hdr.length < 512) continue;
          const h = hdr;
          hdr = Buffer.alloc(0);
          if (h.every((b) => b === 0)) {
            ended = true;
            continue;
          }
          const type = String.fromCharCode(h[156] || 0x30);
          const size = octal(h.subarray(124, 136));
          remaining = size;
          pad = (512 - (size % 512)) % 512;
          if (type === "x" || type === "L") {
            special = { type, chunks: [] };
          } else if (type === "0") {
            let name = cstr(h.subarray(0, 100));
            if (cstr(h.subarray(257, 262)) === "ustar" && h[345]) name = `${cstr(h.subarray(345, 500))}/${name}`;
            if (override !== undefined) name = override;
            const m = /^package\/vendor\/[^/]+\/(.+)$/.exec(name);
            if (m && want.has(m[1])) {
              const dest = want.get(m[1]);
              if (opened.has(dest)) throw new Error(`${basename(file)} holds ${dest} more than once (${name})`);
              opened.add(dest);
              cur = { fd: openSync(join(destDir, dest), "w", 0o755), name: dest };
            }
            override = undefined;
          } else if (type !== "g") override = undefined;
          if (remaining === 0) endBody();
          else state = "body";
        } else if (state === "body") {
          const take = buf.subarray(0, remaining);
          if (cur) for (let off = 0; off < take.length; ) off += writeSync(cur.fd, take, off);
          if (special) special.chunks.push(Buffer.from(take));
          remaining -= take.length;
          buf = buf.subarray(take.length);
          if (remaining === 0) endBody();
        } else {
          const skip = Math.min(pad, buf.length);
          pad -= skip;
          buf = buf.subarray(skip);
          if (pad === 0) state = "header";
        }
      }
    }
    if (!ended || state !== "header" || hdr.length !== 0 || special || override !== undefined)
      throw new Error(`${basename(file)} is truncated: it ends before the end of the archive`);
    const missing = [...want.entries()].filter(([, n]) => !written.includes(n)).map(([k]) => k);
    if (missing.length) throw new Error(`${basename(file)} does not contain ${missing.join(", ")}`);
  } catch (e) {
    if (cur) closeSync(cur.fd);
    cur = undefined;
    for (const n of opened) rmSync(join(destDir, n), { force: true });
    throw e;
  }
  return written;
}

/** The environment of every Codex process the script starts: a throwaway home, no user data. */
export function throwawayEnv(dir, platform = process.platform) {
  const home = join(dir, "home");
  const env = { HOME: home, USERPROFILE: home, CODEX_HOME: join(dir, "codex-home"), TMPDIR: join(dir, "tmp") };
  if (platform === "win32") {
    env.TEMP = env.TMP = env.TMPDIR;
    if (process.env.SystemRoot) env.SystemRoot = process.env.SystemRoot;
  } else env.PATH = "/usr/bin:/bin";
  return env;
}

const defaultRun = (cmd, args, opts) => spawnSync(cmd, args, { encoding: "utf8", maxBuffer: 64 << 20, ...opts });

function codex(run, bin, args, env, cwd) {
  const r = run(bin, args, { env, cwd });
  if (r.status !== 0) throw new Error(`codex ${args.join(" ")} failed (${r.status}): ${String(r.stderr).slice(0, 500)}`);
  return String(r.stdout);
}

/**
 * Runs `generate-json-schema` (stable, then `--experimental`) and `features list` under a
 * throwaway home inside `workDir`, writing `<outDir>/{schema/, schema-experimental/,
 * features.txt}` (the experimental bundle keeps `EXPERIMENTAL_FILES` only).
 */
export function generateArtifacts({ bin, outDir, workDir, run = defaultRun }) {
  const env = throwawayEnv(workDir);
  for (const d of [env.HOME, env.CODEX_HOME, env.TMPDIR]) mkdirSync(d, { recursive: true });
  for (const n of ["schema", "schema-experimental", "features.txt"]) rmSync(join(outDir, n), { recursive: true, force: true });
  mkdirSync(outDir, { recursive: true });
  const stable = join(outDir, "schema");
  codex(run, bin, ["app-server", "generate-json-schema", "--out", stable], env, env.HOME);
  const exp = join(workDir, "experimental");
  codex(run, bin, ["app-server", "generate-json-schema", "--experimental", "--out", exp], env, env.HOME);
  const keep = join(outDir, "schema-experimental");
  mkdirSync(keep, { recursive: true });
  for (const f of EXPERIMENTAL_FILES) {
    if (!existsSync(join(exp, f))) throw new Error(`the experimental schema lacks ${f}`);
    copyFileSync(join(exp, f), join(keep, f));
  }
  writeFileSync(join(outDir, "features.txt"), codex(run, bin, ["features", "list"], env, env.HOME));
}

function filesUnder(dir) {
  const out = [];
  const go = (d) => {
    if (!existsSync(d)) return;
    for (const e of readdirSync(d, { withFileTypes: true })) {
      if (e.isDirectory()) go(join(d, e.name));
      else out.push(relative(dir, join(d, e.name)).split(sep).join("/"));
    }
  };
  go(dir);
  return out.sort();
}
const text = (p) => readFileSync(p, "utf8").replace(/\r\n/g, "\n");

/** Differences between a fresh generation and the committed snapshot (empty = identical). */
export function checkSnapshot({ bin, snapshotDir, workDir, run = defaultRun }) {
  const fresh = join(workDir, "fresh");
  generateArtifacts({ bin, outDir: fresh, workDir, run });
  const diffs = [];
  for (const dir of ["schema", "schema-experimental"]) {
    const a = new Set(filesUnder(join(snapshotDir, dir)));
    const b = new Set(filesUnder(join(fresh, dir)));
    for (const f of a) if (!b.has(f)) diffs.push(`${dir}/${f}: committed, not generated`);
    for (const f of b) if (!a.has(f)) diffs.push(`${dir}/${f}: generated, not committed`);
    for (const f of a) if (b.has(f) && text(join(snapshotDir, dir, f)) !== text(join(fresh, dir, f))) diffs.push(`${dir}/${f}: differs`);
  }
  const f = join(snapshotDir, "features.txt");
  if (!existsSync(f) || text(f) !== text(join(fresh, "features.txt"))) diffs.push("features.txt: differs");
  return diffs;
}

/** The platform package's tarball in `cacheDir`, fetched with `npm pack` unless already there with the right integrity. */
export async function ensureTarball({ version, target, integrity, cacheDir, run = defaultRun }) {
  const file = join(cacheDir, `openai-codex-${version}-${target}.tgz`);
  if (existsSync(file) && (await integrityOf(file)) === integrity) return file;
  rmSync(file, { force: true });
  mkdirSync(cacheDir, { recursive: true });
  const win = process.platform === "win32";
  const r = run(win ? "npm.cmd" : "npm", ["pack", `@openai/codex@${version}-${target}`, "--pack-destination", cacheDir, "--silent"], { shell: win });
  if (r.status !== 0 || !existsSync(file)) throw new Error(`npm pack @openai/codex@${version}-${target} failed: ${String(r.stderr).slice(0, 300)}`);
  await verifyTarball(file, integrity);
  return file;
}

export async function main(argv, root = join(dirname(fileURLToPath(import.meta.url)), "..")) {
  const check = argv.includes("--check");
  const all = argv.includes("--all");
  const ti = argv.indexOf("--target");
  const host = hostTarget();
  const pin = readPin(root);
  const targets = all ? TARGETS : [ti === -1 ? host : argv[ti + 1]];
  // A check that compares nothing is no check: it needs the host's own target, which it can run.
  if (check && (host === undefined || !targets.includes(host)))
    throw new Error(`--check generates the schema with the host's own binary: select ${host ?? "a supported host"} (no target given, or --all)`);
  const cache = join(root, ".cache/codex");
  let failed = false;
  for (const target of targets) {
    const entry = pin.targets[target];
    if (!entry) throw new Error(`pin.json has no target ${target}`);
    const tgz = await ensureTarball({ version: pin.version, target, integrity: entry.integrity, cacheDir: join(cache, "downloads") });
    const dest = join(cache, pin.version, target);
    rmSync(dest, { recursive: true, force: true });
    const names = await extractKept(tgz, dest, entry.kept, { integrity: entry.integrity });
    console.log(`codex-pin: ${target}: integrity verified, kept ${names.join(", ")}`);
    if (target !== host) continue;
    const bin = join(dest, target.startsWith("win32") ? "codex.exe" : "codex");
    mkdirSync(join(cache, "work"), { recursive: true });
    const workDir = mkdtempSync(join(cache, "work", "run-"));
    try {
      const snapshotDir = join(root, "vendor/codex", pin.version);
      if (check) {
        const diffs = checkSnapshot({ bin, snapshotDir, workDir });
        for (const d of diffs) console.log(`CODEX_PIN_DRIFT ${d}`);
        if (diffs.length) failed = true;
        else console.log("codex-pin: the fresh schema and features equal the committed snapshot");
      } else {
        generateArtifacts({ bin, outDir: snapshotDir, workDir });
        console.log(`codex-pin: wrote ${relative(root, snapshotDir)}/{schema, schema-experimental, features.txt}`);
      }
    } finally {
      rmSync(workDir, { recursive: true, force: true });
    }
  }
  if (!targets.includes(host)) console.log(`codex-pin: ${host} was not selected: no schema generated or checked`);
  return failed ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).then((c) => { process.exitCode = c; }, (e) => { console.error(`codex-pin: ${e.message}`); process.exitCode = 1; });
}
