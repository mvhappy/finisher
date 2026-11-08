// Cross-host storage harness controller (plan 4A Task 19). See README.md.
//
//   node tests/storage-xhost/run.mjs <scenario> [--rounds N] [--storage share|local]
//        [--nodes a,b[,c]] [--out DIR] [--secs S] [--stall-for-ms U] [--stall-mode M]
//        [--skew-ms K] [--b-delay-ms D]
//   node tests/storage-xhost/run.mjs analyse <results dir>
//
// Scenarios: race, race-stale, stall, stall-short, stall-check, stall-sigstop, follow, skew,
// history, mixed; the 4C rows (plan 4C Task 29) upgrade4c, mixed4c, newer4c, edit4c, whose
// 4B nodes take a 4B `xhost` in XHOST_<N>_BIN (README.md, "4C rows").
// Host and share access come only from XHOST_* variables (README.md); nothing about hosts is
// written into this repository.
import { randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync, createWriteStream } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { analyseRound, summarise } from "./analyse.mjs";
import { roundLine4c, reportLines4c } from "./analyse-edit4c.mjs";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const env = process.env;

function opts(argv) {
  const o = { _: [] };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) o[a.slice(2)] = argv[i + 1]?.startsWith("--") || argv[i + 1] === undefined ? true : argv[++i];
    else o._.push(a);
  }
  return o;
}

/** A node: where and how `xhost` runs (README.md, "Nodes"). */
function node(name, storage) {
  const k = (s) => env[`XHOST_${name.toUpperCase()}_${s}`];
  const cmd = (k("CMD") ?? "").trim();
  const local = cmd === "";
  const os = k("OS") ?? "posix";
  const base = storage === "local" ? (k("LOCAL") ?? env.XHOST_LOCAL) : (k("SHARE") ?? env.XHOST_SHARE);
  if (!base) throw new Error(`no ${storage === "local" ? "XHOST_LOCAL" : "XHOST_SHARE"} for node ${name}`);
  return {
    name,
    cmd,
    local,
    os,
    host: k("HOST") ?? (local ? "local" : name),
    bin: k("BIN") ?? (local ? join(REPO, "target/release/xhost") : "xhost"),
    base,
    data: k("DATA") ?? env.XHOST_DATA ?? (local ? join(REPO, "archive/spike/sp4a-xhost/data", name) : `xhost-data/${name}`),
  };
}

/** Two nodes that must be one host (the same `XHOST_<N>_HOST`). */
function sameHost(x, y, what) {
  if (x.host !== y.host) throw new Error(`${what} must be one host (XHOST_${x.name.toUpperCase()}_HOST = XHOST_${y.name.toUpperCase()}_HOST); got ${x.host} and ${y.host}`);
}

const sep = (n) => (n.os === "windows" ? "\\" : "/");
const pjoin = (n, ...parts) => parts.join(sep(n));
// cmd.exe strips the first and last quote of a line with more than two, so on Windows only
// arguments that need it are quoted.
const winQuote = (s) => (/^[\w:\\.\-=,+/]+$/.test(s) ? s : `"${s.replaceAll('"', '\\"')}"`);
const quote = (n, s) => (n.os === "windows" ? winQuote(String(s)) : `'${String(s).replaceAll("'", "'\\''")}'`);

/** Runs `argv` on a node; resolves with the JSONL events it printed and its exit code. */
function exec(n, argv, outFile, { onEvent } = {}) {
  argv = argv.map(String);
  let child;
  if (n.local) child = spawn(argv[0], argv.slice(1), { stdio: ["ignore", "pipe", "pipe"] });
  else {
    const [c, ...pre] = n.cmd.split(/\s+/);
    child = spawn(c, [...pre, argv.map((a) => quote(n, a)).join(" ")], { stdio: ["ignore", "pipe", "pipe"] });
  }
  const out = outFile ? createWriteStream(outFile) : null;
  const err = outFile ? createWriteStream(`${outFile}.err`) : null;
  const events = [];
  let buf = "";
  child.stdout.on("data", (d) => {
    out?.write(d);
    buf += d;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      try {
        const e = JSON.parse(line);
        events.push(e);
        onEvent?.(e);
      } catch {
        /* not an event */
      }
    }
  });
  child.stderr.on("data", (d) => err?.write(d));
  return new Promise((ok) =>
    child.on("close", (code) => {
      out?.end();
      err?.end();
      ok({ events, exit: code });
    }),
  );
}

/** Runs `xhost args` on a node. */
const run = (n, args, outFile, o) => exec(n, [n.bin, ...args], outFile, o);

/** Sends a signal to a process of a POSIX node (by the pid its `start` event reported). */
async function signal(n, pid, sig) {
  if (n.local) process.kill(pid, sig);
  else await exec(n, ["kill", `-${sig.replace("SIG", "")}`, pid], null);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The processes of one round of `scenario`, as [{node, role, args, label, delayMs, sigstop}]. */
function plan(scenario, nodes, o, dir, round) {
  // The 4C rows keep 4B and 4C server data apart (4B nodes use `*4b` directories), except
  // upgrade4c, whose 4B server data is upgraded on purpose.
  const [a, b] = nodes;
  const c = nodes[2] ?? b;
  const d = (n, sub) => pjoin(n, n.data, sub);
  const P = (n) => pjoin(n, n.base, dir);
  const base = (n, data, label, secs) => ["--data", d(n, data), "--dir", P(n), "--label", label, "--secs", secs];
  const stallAt = o["stall-at-ms"] ?? 10_000;
  const mk = (stallFor, mode, secs) => ({
    create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
    procs: [
      { node: a, role: "holder", args: ["holder", ...base(a, "a", "a", secs), "--commit-every-ms", 300, ...(mode === "sigstop" ? [] : ["--stall-at-ms", stallAt, "--stall-for-ms", stallFor, "--stall-mode", mode])], delayMs: 0, sigstop: mode === "sigstop" ? { atMs: stallAt, forMs: stallFor } : null },
      { node: b, role: "contender", args: ["contender", ...base(b, "b", "b", secs - 3), "--commit-every-ms", 300], delayMs: 3000 },
      { node: c, role: "follower", args: ["follower", ...base(c, "f", "f", secs - 4)], delayMs: 4000 },
    ],
  });
  const stallFor = Number(o["stall-for-ms"] ?? 90_000);
  switch (scenario) {
    case "race":
    case "race-stale": {
      const secs = Number(o.secs ?? (scenario === "race" ? 12 : 110));
      // `--skew-ms K` (opt-in here): racer `a` sees the share's clock K ms off, as a client
      // whose mtimes are its own skewed clock would. `--b-delay-ms D`: racer `b` starts D ms
      // later (to let `a` claim first).
      const skew = o["skew-ms"] === undefined ? [] : ["--skew-ms", Number(o["skew-ms"])];
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", ...(scenario === "race" ? ["--free"] : [])] },
        procs: [
          { node: a, role: "holder", args: ["holder", ...base(a, "a", "a", secs), "--commit-every-ms", 300, ...skew], delayMs: 0 },
          { node: b, role: "holder", args: ["holder", ...base(b, "b", "b", secs), "--commit-every-ms", 300], delayMs: Number(o["b-delay-ms"] ?? 0) },
        ],
      };
    }
    case "stall":
      return mk(stallFor, o["stall-mode"] ?? "freeze", Number(o.secs ?? Math.round(stallFor / 1000) + 60));
    case "stall-short":
      return mk(Number(o["stall-for-ms"] ?? 30_000), o["stall-mode"] ?? "freeze", Number(o.secs ?? 75));
    case "stall-check":
      return mk(stallFor, "check", Number(o.secs ?? Math.round(stallFor / 1000) + 60));
    case "stall-sigstop":
      if (a.os === "windows") throw new Error("stall-sigstop needs a POSIX holder node");
      return mk(stallFor, "sigstop", Number(o.secs ?? Math.round(stallFor / 1000) + 60));
    case "follow": {
      const secs = Number(o.secs ?? 70);
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "holder", args: ["holder", ...base(a, "a", "a", secs), "--commit-every-ms", o["commit-every-ms"] ?? 1000], delayMs: 0 },
          { node: b, role: "follower", args: ["follower", ...base(b, "f", "f", secs - 2)], delayMs: 2000 },
        ],
      };
    }
    case "skew": {
      // Emulated server-time skew (`--skew-ms`) on the holder, which stalls: its records look
      // older (negative) or younger (positive) to the contender by the skew.
      const skew = Number(o["skew-ms"] ?? -60_000);
      const p = mk(stallFor, "freeze", Number(o.secs ?? Math.round(stallFor / 1000) + 60));
      p.procs[0].args.push("--skew-ms", skew);
      return p;
    }
    case "history": {
      // Brancher `a` (node a) makes branch x and edits both lines, then stops without closing;
      // merger `b` (node b) starts when it has ended, waits for the takeover and the heads `a`
      // reached, merges x into main skipping the conflict, and exports; follower `f` (node c,
      // else a: the host the merger is not on) stops one follow cycle after applying the merge.
      const f = nodes[2] ?? a;
      const heads = (evs) => Object.entries(evs.find((e) => e.ev === "final")?.hashes?.heads ?? {}).map(([br, [seq]]) => `${br}=${seq}`).join(",");
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "brancher", args: ["brancher", ...base(a, "a", "a", Number(o.secs ?? 60)), "--commit-every-ms", o["commit-every-ms"] ?? 500, "--edits", o.edits ?? 12], delayMs: 0 },
          { node: f, role: "follower", args: ["follower", ...base(f, "f", "f", 300), "--until-kind", "merge", "--settle-ms", 3000], delayMs: 2000 },
          { node: b, role: "merger", args: ["merger", ...base(b, "b", "b", 180), "--open-retry-ms", o["open-retry-ms"] ?? 90_000], delayMs: 0, after: "a", extra: (evs) => ["--expect", heads(evs) || "main=0"] },
        ],
        verifyOn: [a, b],
      };
    }
    case "mixed": {
      // Mixed versions (plan 4B Decision 4): a 4B brancher on node a edits main only for
      // `--plain-ms` (default 20 s), then makes a branch and edits both; node b runs a 4A
      // follower (`XHOST_B_BIN`: a 4A `xhost`), which must follow the edit-only part and then
      // turn read-only NEWER_FORMAT, applying nothing past the branch record.
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "brancher", args: ["brancher", ...base(a, "a", "a", Number(o.secs ?? 60)), "--commit-every-ms", o["commit-every-ms"] ?? 1000, "--plain-ms", o["plain-ms"] ?? 20_000, "--edits", o.edits ?? 8], delayMs: 0 },
          { node: b, role: "follower-4a", args: ["follower", ...base(b, "o", "o", Number(o.secs ?? 60) + 10)], delayMs: 2000 },
        ],
      };
    }
    case "upgrade4c": {
      // A 4B brancher (node a, XHOST_A_BIN: a 4B xhost) makes branch x and edits both lines,
      // then stops; a 4C `upgrade` on node b opens the SAME server data (so its database is
      // migrated in place), verifies, undoes a 4B record and exports; a fresh 4C verify on
      // node b rebuilds from the folder alone. Nodes a and b must be one host.
      sameHost(a, b, "upgrade4c: nodes a and b");
      // Server data of its own each round: the upgrade migrates it, so 4B never reuses it.
      const up = `upgrade-${dir.replaceAll("/", "-")}`;
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "brancher", args: ["brancher", ...base(a, up, "a", Number(o.secs ?? 60)), "--commit-every-ms", o["commit-every-ms"] ?? 500, "--edits", o.edits ?? 8], delayMs: 0 },
          // Node a's server data, run by node b's (4C) binary.
          { node: b, role: "upgrade", args: ["upgrade", "--data", d(a, up), "--dir", P(b), "--label", "u", "--secs", 120], after: "a" },
        ],
        verifyOn: [b],
      };
    }
    case "mixed4c": {
      // Per NAS pair, hosts X and Y (nodes a,b,c,d): a 4B holder `a` on X (XHOST_A_BIN: 4B)
      // writes for `--secs` (30) and closes; when it has ended a 4C holder `b` on Y takes over
      // (the 45 s staleness, then the ordering rule) and writes for `--hold-secs` (20); a 4B
      // follower `c` on Y (XHOST_C_BIN: 4B) follows throughout (`--follow-secs`, 120) and must
      // turn read-only NEWER_FORMAT at b's first record; a 4C follower `d` on X follows until
      // b's done marker. Fresh 4C verifiers on Y and X.
      const [, , c4b, d4c] = nodes;
      if (!c4b || !d4c) throw new Error("mixed4c needs four nodes: --nodes a,b,c,d");
      sameHost(b, c4b, "mixed4c: nodes b and c");
      sameHost(a, d4c, "mixed4c: nodes a and d");
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "holder-4b", args: ["holder", ...base(a, "a4b", "a", Number(o.secs ?? 30)), "--commit-every-ms", o["commit-every-ms"] ?? 1000], delayMs: 0 },
          { node: c4b, role: "follower-4b", args: ["follower", ...base(c4b, "o4b", "o", Number(o["follow-secs"] ?? 120)), "--open-retry-ms", o["open-retry-ms"] ?? 90_000], delayMs: 2000 },
          { node: d4c, role: "follower-4c", args: ["follower", ...base(d4c, "f", "f", 400), "--until-done", "b", "--settle-ms", 3000, "--open-retry-ms", o["open-retry-ms"] ?? 90_000], delayMs: 2000 },
          { node: b, role: "holder-4c", args: ["holder", ...base(b, "b", "b", 300), "--hold-secs", o["hold-secs"] ?? 20, "--commit-every-ms", o["commit-every-ms"] ?? 1000, "--done-marker", "--open-retry-ms", o["open-retry-ms"] ?? 90_000], after: "a" },
        ],
        verifyOn: [b, d4c],
      };
    }
    case "newer4c": {
      // A 4C holder on node a creates and writes for `--hold-secs` (10) and closes; then a 4B
      // follower on node b (XHOST_B_BIN: 4B) follows for `--follow-secs` (15), and a 4B verify
      // runs there: both must be read-only NEWER_FORMAT (or refuse the project) and write
      // nothing. A fresh 4C verify on node a is the reference.
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "holder-4c", args: ["holder", ...base(a, "a", "a", 120), "--hold-secs", o["hold-secs"] ?? 10, "--commit-every-ms", o["commit-every-ms"] ?? 1000], delayMs: 0 },
          { node: b, role: "follower-4b", args: ["follower", ...base(b, "o4b", "o", Number(o["follow-secs"] ?? 15)), "--open-retry-ms", o["open-retry-ms"] ?? 90_000], after: "a" },
        ],
        verifyOn: [a, { node: b, role: "verify-4b" }],
      };
    }
    case "edit4c": {
      // The 4C edit row, per NAS pair: editor `a` (node a) pins fonts and sets captions
      // (`xhost editor`) and writes its done marker; follower `f` (node b) follows until it,
      // then fetches the pinned fonts itself (`--fonts fetch`); follower `o` (node c, else b)
      // fetches nothing (`--fonts offline`). Fresh verifiers on nodes a and b.
      const off = nodes[2] ?? b;
      const follower = (n, data, mode) => ["follower", ...base(n, data, data, 400), "--until-done", "a", "--fonts", mode, "--settle-ms", 3000, "--open-retry-ms", o["open-retry-ms"] ?? 90_000];
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", "--free"] },
        procs: [
          { node: a, role: "editor", args: ["editor", ...base(a, "a", "a", 300), "--commit-every-ms", o["commit-every-ms"] ?? 1000, "--done-marker"], delayMs: 0 },
          { node: b, role: "follower-fetch", args: follower(b, "f", "fetch"), delayMs: 1000 },
          { node: off, role: "follower-offline", args: follower(off, "o", "offline"), delayMs: 1000 },
        ],
        verifyOn: [a, b],
      };
    }
    default:
      throw new Error(`unknown scenario ${scenario}`);
  }
}

async function round(scenario, nodes, o, outDir, runId, k) {
  const rel = `${runId}/${scenario}-r${k}`;
  const rdir = join(outDir, scenario, `r${k}`);
  mkdirSync(rdir, { recursive: true });
  const p = plan(scenario, nodes, o, rel, k);
  const procs = [];
  const tag = (n, role, label, r) => ({ id: `${label}:${role}`, node: n.name, host: n.host, role, label, events: r.events, exit: r.exit, server: r.events.find((e) => e.server)?.server ?? null });
  const cr = await run(p.create.node, p.create.args, join(rdir, "create.jsonl"));
  procs.push(tag(p.create.node, "create", "create", cr));
  if (cr.exit !== 0) throw new Error(`create failed on ${p.create.node.name}: see ${rdir}/create.jsonl.err`);
  const t0 = Date.now() + 1500;
  const labelOf = (x) => x.args[x.args.indexOf("--label") + 1];
  // Each process's result, for `after` (it starts when that process has ended, with
  // `extra(its events)` appended).
  const ended = {};
  for (const x of p.procs) {
    let ok;
    ended[labelOf(x)] = { promise: new Promise((r) => (ok = r)), ok: (v) => ok(v) };
  }
  const running = p.procs.map(async (x) => {
    const label = labelOf(x);
    let args;
    if (x.after) {
      const prev = await ended[x.after].promise;
      args = [...x.args, ...(x.extra ? x.extra(prev.events) : [])];
    }
    else args = [...x.args, "--start-at-ms", t0 + x.delayMs];
    let pid = null;
    const r = run(x.node, args, join(rdir, `${label}.jsonl`), { onEvent: (e) => (e.ev === "start" ? (pid = e.pid) : null) });
    if (x.sigstop) {
      await sleep(t0 + x.delayMs - Date.now() + x.sigstop.atMs);
      if (pid) {
        await signal(x.node, pid, "SIGSTOP");
        await sleep(x.sigstop.forMs);
        await signal(x.node, pid, "SIGCONT");
      }
    }
    const res = await r;
    ended[label].ok(res);
    return tag(x.node, x.role, label, res);
  });
  procs.push(...(await Promise.all(running)));
  // A fresh reader on each `verifyOn` node (default: the first): `verify`, `verify-<node>`.
  // An entry is a node, or `{node, role}` to tag that verifier with another role.
  for (const [i, entry] of (p.verifyOn ?? [nodes[0]]).entries()) {
    const v = entry.node ?? entry;
    const label = i === 0 ? "verify" : `verify-${v.name}`;
    const vr = await run(v, ["verify", "--data", pjoin(v, v.data, `${label}-${runId}-${scenario}-r${k}`), "--dir", pjoin(v, v.base, rel), "--label", label], join(rdir, `${label}.jsonl`));
    procs.push(tag(v, entry.role ?? (i === 0 ? "verify" : "verify-other"), label, vr));
  }
  writeFileSync(join(rdir, "meta.json"), JSON.stringify({ scenario, round: k, rel, nodes: procs.map(({ events, ...m }) => m) }, null, 1));
  return procs;
}

function loadRound(rdir) {
  const meta = JSON.parse(readFileSync(join(rdir, "meta.json"), "utf8"));
  return meta.nodes.map((m) => {
    const f = join(rdir, `${m.label}.jsonl`);
    const events = readFileSync(f, "utf8").split("\n").filter(Boolean).flatMap((l) => {
      try {
        return [JSON.parse(l)];
      } catch {
        return [];
      }
    });
    return { ...m, events };
  });
}

function report(outDir, scenario, rounds, tolMs) {
  const s = summarise(rounds);
  const out = { scenario, tolMs, ...s, perRound: rounds.map((r, i) => ({ round: i + 1, critical: r.critical, counts: r.counts, problems: r.problems, notes: r.notes })) };
  writeFileSync(join(outDir, scenario, "summary.json"), JSON.stringify(out, null, 1));
  writeFileSync(join(outDir, scenario, "detail.json"), JSON.stringify(rounds.map((r) => ({ ...r.detail, latencyMs: r.latencyMs })), null, 1));
  const c = s.counts;
  console.log(`\n== ${scenario}: ${s.rounds} rounds, ${s.criticalRounds} critical (tolerance ${tolMs} ms cross-host)`);
  console.log(`  double holders ${c.doubleHolders} (suspects ${c.doubleHolderSuspects}), early takeovers ${c.earlyTakeovers}/${c.takeovers} (unsafe, < 25 s: ${c.unsafeTakeovers})`);
  console.log(`  stale: accepted-after-newer-epoch ${c.staleAcceptedAfterNewerEpoch}, exported ${c.staleExported}, applied ${c.staleApplied}, left applied ${c.staleLeftApplied}; rewinds ${c.rewinds} (${c.rewoundRecords} records)`);
  console.log(`  id reuse ${c.idReuse} (seq collisions among accepted ${c.seqCollisions}); lost acknowledged ${c.lostAcked} (set aside ${c.lostAckedSetAside}, vanished ${c.lostAckedVanished}) of ${c.accepted} accepted`);
  console.log(`  verify failures ${c.verifyFailed}; refusals ${JSON.stringify(s.refused)}; opens retried ${c.openRetries} (PROJECT_NOT_FOUND)`);
  if (c.historyViews !== undefined) console.log(`  history: merged ${c.historyMerged}, skipped ${c.historySkipped} (missing ${c.historySkippedMissing}), wrong merge count ${c.historyMergedWrong}, hash mismatches ${c.historyHashMismatches} over ${c.historyViews} views`);
  if (c.mixedEditOnly !== undefined) console.log(`  mixed: 4A follower followed ${c.mixedFollowed}/${c.mixedEditOnly} edit-only records (${c.mixedEditsInBranchSegment} more exported in the branch record's v2 segment), applied past the branch record ${c.mixedAppliedPastBranch}, NEWER_FORMAT in ${c.mixedNewerFormat} rounds`);
  for (const line of reportLines4c(c)) console.log(`  ${line}`);
  console.log(`  follow latency ms: n=${s.latencyMs.n} p50=${s.latencyMs.p50} p95=${s.latencyMs.p95} max=${s.latencyMs.max}`);
  for (const p of s.problems.slice(0, 20)) console.log(`  problem: ${p}`);
  return out;
}

async function main() {
  const o = opts(process.argv.slice(2));
  const [scenario, arg] = o._;
  const tolMs = Number(env.XHOST_TOL_MS ?? 1000);
  if (scenario === "analyse") {
    for (const sc of readdirSync(arg).filter((x) => statSync(join(arg, x)).isDirectory() && x !== "data")) {
      const rds = readdirSync(join(arg, sc)).filter((x) => /^r\d+$/.test(x)).sort((x, y) => x.slice(1) - y.slice(1));
      report(arg, sc, rds.map((r) => analyseRound(loadRound(join(arg, sc, r)), { tolMs })), tolMs);
    }
    return;
  }
  if (!scenario) throw new Error("usage: run.mjs <scenario> [options] | analyse <dir>");
  const storage = o.storage ?? "share";
  const names = (o.nodes ?? env.XHOST_NODES ?? "a,b").split(",");
  const nodes = names.map((n) => node(n, storage));
  // Time-sortable, plus a random suffix so controllers started in the same millisecond differ.
  const runId = `${new Date().toISOString().replace(/[:.]/g, "-")}-${randomBytes(3).toString("hex")}`;
  const outDir = o.out ?? join(REPO, env.XHOST_RESULTS ?? "archive/spike/sp4a-xhost", `${runId}-${storage}`);
  const rounds = Number(o.rounds ?? (scenario === "race" ? 20 : ["history", "edit4c"].includes(scenario) ? 5 : 1));
  const results = [];
  for (let k = 1; k <= rounds; k++) {
    const procs = await round(scenario, nodes, o, outDir, runId, k);
    const r = analyseRound(procs, { tolMs });
    results.push(r);
    const c = r.counts;
    console.log(`${scenario} r${k}: dbl=${c.doubleHolders} early=${c.earlyTakeovers} staleApplied=${c.staleApplied} left=${c.staleLeftApplied} idReuse=${c.idReuse} lostAck=${c.lostAcked}(vanished ${c.lostAckedVanished}) rewinds=${c.rewinds} verifyFail=${c.verifyFailed} chain=${c.chain}${c.historyViews !== undefined ? ` merged=${c.historyMerged} skipped=${c.historySkipped} hashMismatch=${c.historyHashMismatches}` : ""}${c.mixedEditOnly !== undefined ? ` 4aFollowed=${c.mixedFollowed}/${c.mixedEditOnly} pastBranch=${c.mixedAppliedPastBranch} newer=${c.mixedNewerFormat}` : ""}${roundLine4c(c)}${r.problems.length ? ` problems=${r.problems.length}` : ""}`);
  }
  report(outDir, scenario, results, tolMs);
  console.log(`results: ${outDir}`);
}

main().catch((e) => {
  console.error(e.stack ?? String(e));
  process.exitCode = 1;
});
