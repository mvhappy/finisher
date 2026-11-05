// Cross-host storage harness controller (plan 4A Task 19). See README.md.
//
//   node tests/storage-xhost/run.mjs <scenario> [--rounds N] [--storage share|local]
//        [--nodes a,b[,c]] [--out DIR] [--secs S] [--stall-for-ms U] [--stall-mode M]
//   node tests/storage-xhost/run.mjs analyse <results dir>
//
// Scenarios: race, race-stale, stall, stall-short, stall-check, stall-sigstop, follow, skew.
// Host and share access come only from XHOST_* variables (README.md); nothing about hosts is
// written into this repository.
import { randomBytes } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, statSync, writeFileSync, createWriteStream } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { analyseRound, summarise } from "./analyse.mjs";

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
      return {
        create: { node: a, args: ["create", "--data", d(a, "creator"), "--dir", P(a), "--label", "create", ...(scenario === "race" ? ["--free"] : [])] },
        procs: [
          { node: a, role: "holder", args: ["holder", ...base(a, "a", "a", secs), "--commit-every-ms", 300], delayMs: 0 },
          { node: b, role: "holder", args: ["holder", ...base(b, "b", "b", secs), "--commit-every-ms", 300], delayMs: 0 },
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
  const running = p.procs.map(async (x) => {
    const label = x.args[x.args.indexOf("--label") + 1];
    const args = [...x.args, "--start-at-ms", t0 + x.delayMs];
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
    return tag(x.node, x.role, label, await r);
  });
  procs.push(...(await Promise.all(running)));
  const v = nodes[0];
  const vr = await run(v, ["verify", "--data", pjoin(v, v.data, `verify-${runId}-${scenario}-r${k}`), "--dir", pjoin(v, v.base, rel), "--label", "verify"], join(rdir, "verify.jsonl"));
  procs.push(tag(v, "verify", "verify", vr));
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
  console.log(`  verify failures ${c.verifyFailed}; refusals ${JSON.stringify(s.refused)}`);
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
  const outDir = o.out ?? join(REPO, "archive/spike/sp4a-xhost", `${runId}-${storage}`);
  const rounds = Number(o.rounds ?? (scenario === "race" ? 20 : 1));
  const results = [];
  for (let k = 1; k <= rounds; k++) {
    const procs = await round(scenario, nodes, o, outDir, runId, k);
    const r = analyseRound(procs, { tolMs });
    results.push(r);
    const c = r.counts;
    console.log(`${scenario} r${k}: dbl=${c.doubleHolders} early=${c.earlyTakeovers} staleApplied=${c.staleApplied} left=${c.staleLeftApplied} idReuse=${c.idReuse} lostAck=${c.lostAcked}(vanished ${c.lostAckedVanished}) rewinds=${c.rewinds} verifyFail=${c.verifyFailed} chain=${c.chain}${r.problems.length ? ` problems=${r.problems.length}` : ""}`);
  }
  report(outDir, scenario, results, tolMs);
  console.log(`results: ${outDir}`);
}

main().catch((e) => {
  console.error(e.stack ?? String(e));
  process.exitCode = 1;
});
