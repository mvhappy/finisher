// The A0 version gate (plan 6C Task 11 review): an A0 hash never moves without an
// `audioVersion` bump. `crates/fin-audio/tests/a0/expected.json` holds the blessed hashes and
// the `audioVersion` they were blessed at (the a0 test ties it to `fin_audio::AUDIO_VERSION`).
// For every commit that touches the file (and the working tree against HEAD), against each
// parent that has it: a plan whose `blake3` or `peaks` changed, or a plan removed, needs a
// higher `audioVersion` in the same commit; a lower one is always a finding. Adding a plan
// needs no bump. The first commit holding the file is the baseline.
//
// It needs the whole history: a shallow clone or a directory outside git fails loudly
// (A0_HISTORY_UNAVAILABLE) instead of passing. CI checks out with `fetch-depth: 0`.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { report, rootArg } from "./walk.mjs";

export const FILE = "crates/fin-audio/tests/a0/expected.json";

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

function git(root, args) {
  const r = spawnSync("git", args, { cwd: root, encoding: "utf8", maxBuffer: 1 << 28 });
  return { ok: r.status === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? "").trim() };
}

/** The file at `rev`, parsed, or null when the revision does not hold it. */
function at(root, rev) {
  const r = git(root, ["show", `${rev}:${FILE}`]);
  return r.ok ? JSON.parse(r.out) : null;
}

export function check(root) {
  const inside = git(root, ["rev-parse", "--is-inside-work-tree"]);
  const top = git(root, ["rev-parse", "--show-toplevel"]);
  if (!inside.ok || inside.out !== "true" || !top.ok)
    return [`A0_HISTORY_UNAVAILABLE: ${root} is not a git checkout; the A0 version gate needs its history`];
  if (git(root, ["rev-parse", "--is-shallow-repository"]).out === "true")
    return [
      `A0_HISTORY_UNAVAILABLE: ${root} is a shallow clone; the A0 version gate needs the whole history ` +
        "(git fetch --unshallow, or fetch-depth: 0 in CI)",
    ];
  const log = git(root, ["log", "--full-history", "--format=%H %P", "--", FILE]);
  if (!log.ok) return [`A0_HISTORY_UNAVAILABLE: git log failed: ${log.err}`];
  const findings = [];
  for (const line of log.out.split("\n").filter(Boolean)) {
    const [commit, ...parents] = line.split(" ");
    const cur = at(root, commit);
    for (const p of parents) {
      const old = at(root, p);
      if (old === null) continue;
      const where = `commit ${commit.slice(0, 12)} (against ${p.slice(0, 12)})`;
      if (cur === null) findings.push(`A0_HASH_WITHOUT_BUMP: ${where}: ${FILE} removed`);
      else findings.push(...compare(old, cur, where));
    }
  }
  const head = at(root, "HEAD");
  if (head !== null) {
    const path = join(top.out, FILE);
    if (!existsSync(path)) findings.push(`A0_HASH_WITHOUT_BUMP: working tree: ${FILE} removed`);
    else findings.push(...compare(head, JSON.parse(readFileSync(path, "utf8")), "working tree (against HEAD)"));
  }
  return findings;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report(check(rootArg()));
