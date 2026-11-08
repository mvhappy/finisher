// Helpers of the "library" scenario (plan 5D Task 15): writing a library tree, the hash of a
// tree and its metadata, the overlay's open log, and the scenario's `fin` checks.
import { createHash } from "node:crypto";
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fin, finRaw } from "./lib.mjs";

/** Writes `rel` (a `/`-separated path) under `root` with `bytes`, making its folders. */
export function put(root, rel, bytes = `bytes of ${rel}`) {
  const path = join(root, ...rel.split("/"));
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, bytes);
}

/**
 * A hash of `root` and everything under it: each entry's raw name, type, mode, size, mtime, ctime,
 * inode and link count, and each file's bytes, in name order. The access time is left out:
 * macOS and Windows have no `O_NOATIME`, so a read may move it (spec §7: nothing is created,
 * written, renamed, locked or deleted under a root; reading is what a scan does).
 */
export function treeHash(root) {
  const h = createHash("sha256");
  const stat = (path) => {
    const st = lstatSync(path, { bigint: true });
    return { st, text: `${st.mode} ${st.size} ${st.mtimeNs} ${st.ctimeNs} ${st.ino} ${st.nlink}` };
  };
  // The root's own times too: an entry made and removed directly under it moves only those.
  h.update(`/\0d ${stat(root).text}\n`);
  const walk = (dir, rel) => {
    // The scenario's names are UTF-8 (NFC and NFD twins included), so strings name them exactly.
    for (const name of readdirSync(dir).sort()) {
      const path = join(dir, name);
      const { st, text } = stat(path);
      const kind = st.isDirectory() ? "d" : st.isSymbolicLink() ? "l" : st.isFile() ? "f" : "o";
      h.update(`${rel}/${name}\0${kind} ${text}\n`);
      if (kind === "f") h.update(readFileSync(path));
      if (kind === "d") walk(path, `${rel}/${name}`);
    }
  };
  walk(root, "");
  return h.digest("hex");
}

/** The overlay's open log (`{op, path, hydrate?, refused?}` lines), empty before the first open. */
export function openLog(path) {
  if (!existsSync(path)) return [];
  return readFileSync(path, "utf8")
    .split("\n")
    .filter((l) => l.trim() !== "")
    .map((l) => JSON.parse(l));
}

/** Every file row of `dir` (all pages; `missing` rows only with `state`). */
export function allFiles(sd, cwd, lib, dir, state) {
  const out = [];
  let cursor;
  for (;;) {
    const args = ["library", "files", lib, ...(dir === undefined ? [] : [dir]), "--limit", "200", ...(state === undefined ? [] : ["--state", state]), ...(cursor === undefined ? [] : ["--cursor", cursor])];
    const page = fin(sd, cwd, args);
    out.push(...page.files);
    if (page.next === undefined) return { ...page, files: out };
    cursor = page.next;
  }
}

/** `fin library show <lib> <path>`: the file row, or throws with the error. */
export const show = (sd, cwd, lib, path) => fin(sd, cwd, ["library", "show", lib, path]).file;

/** `fin library show` that must fail with `code`: answers the error. */
export function showFails(sd, cwd, lib, path, code) {
  const r = finRaw(sd, cwd, ["library", "show", lib, path]);
  if (r.status === 0 || r.json?.error?.code !== code) throw new Error(`show ${path}: expected ${code}, got exit ${r.status}: ${r.text}`);
  return r.json.error;
}

/** Runs a scan with `fin library scan` and waits for it with `fin wait`. */
export function scanAndWait(sd, cwd, lib, args = ["--full"]) {
  const job = fin(sd, cwd, ["library", "scan", lib, ...args]).job;
  const waited = fin(sd, cwd, ["wait", job.id, "--timeout", "120"]);
  if (waited.jobs[0].state !== "succeeded") throw new Error(`scan ${job.id} ended ${JSON.stringify(waited.jobs[0])}`);
  return job;
}
