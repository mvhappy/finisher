// The A0 version gate (plan 6C Task 11 review): an A0 hash never moves without an
// `audioVersion` bump. `crates/fin-audio/tests/a0/expected.json` holds the blessed hashes and
// the `audioVersion` they were blessed at (the a0 test ties it to `fin_audio::AUDIO_VERSION`).
// For every commit that touches the file (and the working tree against HEAD), against each
// parent that has it: a plan whose `blake3` or `peaks` changed, or a plan removed, needs a
// higher `audioVersion` in the same commit; a lower one is always a finding. Adding a plan
// needs no bump. The first commit holding the file is the baseline. At every revision it
// inspects (each commit touching expected.json or fin-audio's lib.rs, and the working tree),
// the JSON's `audioVersion` must equal that revision's `pub const AUDIO_VERSION` in
// `crates/fin-audio/src/lib.rs` (A0_VERSION_MISMATCH). All revisions are read in one
// `git cat-file --batch`.
//
// It needs the whole history: a shallow clone or a directory outside git fails loudly
// (A0_HISTORY_UNAVAILABLE) instead of passing. CI checks out with `fetch-depth: 0`.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg } from "./walk.mjs";

export const FILE = "crates/fin-audio/tests/a0/expected.json";
export const LIB = "crates/fin-audio/src/lib.rs";

/** The findings for `cur` against its parent's `old` (both parsed expected.json). */
export function compare(old, cur, where) {
  const [ov, cv] = [old.audioVersion, cur.audioVersion];
  if (!Number.isInteger(cv) || !Number.isInteger(ov))
    return [`A0_HISTORY: ${where}: audioVersion is not an integer (${ov} → ${cv})`];
  if (cv > ov) return [];
  const out = [];
  if (cv < ov) out.push(`A0_VERSION_LOWERED: ${where}: audioVersion went from ${ov} to ${cv}`);
  const now = cur.plans ?? {};
  for (const [name, was] of Object.entries(old.plans ?? {})) {
    const moved =
      now[name] === undefined ? ["removed"] : ["blake3", "peaks"].filter((k) => was[k] !== now[name][k]);
    if (moved.length)
      out.push(
        `A0_HASH_WITHOUT_BUMP: ${where}: plan ${name}: ${moved.join(", ")} ${moved[0] === "removed" ? "" : "changed "}` +
          `under audioVersion ${cv}; bump fin_audio::AUDIO_VERSION in the same commit and re-bless`,
      );
  }
  return out;
}

function git(root, args, input) {
  const r = spawnSync("git", args, { cwd: root, input, maxBuffer: 1 << 30 });
  return { ok: r.status === 0, out: r.stdout ?? Buffer.alloc(0), err: (r.stderr ?? "").toString().trim() };
}

/** The contents of each `rev:path` (null when the revision does not hold it), in one
 * `git cat-file --batch`. */
function blobs(root, specs) {
  if (specs.length === 0) return [];
  const r = git(root, ["cat-file", "--batch"], specs.join("\n") + "\n");
  if (!r.ok) throw new Error(`git cat-file failed: ${r.err}`);
  const buf = r.out;
  const out = [];
  let pos = 0;
  for (let i = 0; i < specs.length; i++) {
    const nl = buf.indexOf(10, pos);
    const header = buf.subarray(pos, nl).toString();
    pos = nl + 1;
    const [, type, size] = header.split(" ");
    if (type !== "blob") {
      out.push(null);
      continue;
    }
    out.push(buf.subarray(pos, pos + Number(size)).toString("utf8"));
    pos += Number(size) + 1;
  }
  return out;
}

/** `pub const AUDIO_VERSION: u32 = N;` of a lib.rs text, or null. */
export function rustVersion(text) {
  const m = /^pub const AUDIO_VERSION: u32 = (\d+);/m.exec(text);
  return m ? Number(m[1]) : null;
}

/** The finding when `json` (parsed expected.json) and `lib` (lib.rs text) disagree. */
function versionFinding(json, lib, where) {
  if (json === null) return [];
  const rust = lib === null ? null : rustVersion(lib);
  if (rust === null)
    return [`A0_VERSION_MISMATCH: ${where}: ${LIB} has no \`pub const AUDIO_VERSION: u32 = N;\` beside ${FILE}`];
  if (rust !== json.audioVersion)
    return [
      `A0_VERSION_MISMATCH: ${where}: expected.json has audioVersion ${json.audioVersion} but ${LIB} has AUDIO_VERSION ${rust}; ` +
        "they change together, in the commit that re-blesses",
    ];
  return [];
}

export function check(root) {
  const inside = git(root, ["rev-parse", "--is-inside-work-tree"]);
  const top = git(root, ["rev-parse", "--show-toplevel"]);
  if (!inside.ok || inside.out.toString().trim() !== "true" || !top.ok)
    return [`A0_HISTORY_UNAVAILABLE: ${root} is not a git checkout; the A0 version gate needs its history`];
  if (git(root, ["rev-parse", "--is-shallow-repository"]).out.toString().trim() === "true")
    return [
      `A0_HISTORY_UNAVAILABLE: ${root} is a shallow clone; the A0 version gate needs the whole history ` +
        "(git fetch --unshallow, or fetch-depth: 0 in CI)",
    ];
  const log = git(root, ["log", "--full-history", "--format=%H %P", "--", FILE, LIB]);
  if (!log.ok) return [`A0_HISTORY_UNAVAILABLE: git log failed: ${log.err}`];
  const commits = log.out
    .toString()
    .split("\n")
    .filter(Boolean)
    .map((line) => line.split(" ").filter(Boolean));
  const revs = [...new Set(["HEAD", ...commits.flat()])];
  const texts = blobs(root, revs.flatMap((r) => [`${r}:${FILE}`, `${r}:${LIB}`]));
  const json = new Map(revs.map((r, i) => [r, texts[2 * i] === null ? null : JSON.parse(texts[2 * i])]));
  const lib = new Map(revs.map((r, i) => [r, texts[2 * i + 1]]));
  const findings = [];
  for (const [commit, ...parents] of commits) {
    const cur = json.get(commit);
    findings.push(...versionFinding(cur, lib.get(commit), `commit ${commit.slice(0, 12)}`));
    for (const p of parents) {
      const old = json.get(p);
      if (old === null) continue;
      const where = `commit ${commit.slice(0, 12)} (against ${p.slice(0, 12)})`;
      if (cur === null) findings.push(`A0_HASH_WITHOUT_BUMP: ${where}: ${FILE} removed`);
      else findings.push(...compare(old, cur, where));
    }
  }
  const dir = top.out.toString().trim();
  const read = (path) => (existsSync(join(dir, path)) ? readFileSync(join(dir, path), "utf8") : null);
  const [wtJson, wtLib] = [read(FILE), read(LIB)];
  const wt = wtJson === null ? null : JSON.parse(wtJson);
  findings.push(...versionFinding(wt, wtLib, "working tree"));
  const head = json.get("HEAD");
  if (head !== null) {
    if (wt === null) findings.push(`A0_HASH_WITHOUT_BUMP: working tree: ${FILE} removed`);
    else findings.push(...compare(head, wt, "working tree (against HEAD)"));
  }
  return [...new Set(findings)];
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report(check(rootArg()));
