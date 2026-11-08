#!/usr/bin/env node
// The TUF attack matrix, run against the real client (plan 8A Task 13, review focus 2): the
// `tuf_conformance_client` example of fin-registry (feature `test-http`) refreshes and downloads
// from a hostile HTTP server that serves repositories `fin-plugin` built, mutated the way the
// attack says. Each case names the attack, the answer the client must give and what it gave.
//
//   rollback, freeze, mix-and-match, key rotation (and a stolen old key), threshold, expired
//   metadata, endless data (metadata and target), a tampered target.
//
// The upstream suite (run.mjs) covers the protocol; this covers what it cannot serve, an endless
// body above all, and prints the bytes the client read before it gave up.
//
// Usage: node scripts/tuf-conformance/attacks.mjs [--json] [--out <file>] (the table also goes to
// <file>, clear of the progress lines on stdout; after `cargo build --release -p
// fin-plugin-cli` and `cargo build --release -p fin-registry --features test-http --example
// tuf_conformance_client`; `CARGO_TARGET_DIR` is honoured). Exit 1 when any case differs.
import { spawn } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { TARGET_DIR, WIN } from "../e2e/lib.mjs";
import { makeRegistry, mustPlugin } from "../e2e/plugins-lib.mjs";

const CLIENT = join(TARGET_DIR, "release", "examples", WIN ? "tuf_conformance_client.exe" : "tuf_conformance_client");
const DAY_MS = 86_400_000;
/**
 * What the server may write into a connection before the client hangs up on an endless body: the
 * caps (16 KiB timestamp, 1 MiB snapshot, the signed length of a target) plus the kernel's socket
 * buffers, which fill before the server sees the close. A client that read on would reach the
 * server's 64 MiB stop.
 */
const SLACK = 4 * 1024 * 1024;
const TARGET = "acme/alpha/1.1.0.finpkg";

/** Serves `view(path)` over loopback: bytes, `"endless"` (a body that never ends), or undefined (404). */
function serve(view) {
  const stats = { sent: 0, endlessSent: 0 };
  const server = createServer((req, res) => {
    const path = decodeURIComponent((req.url ?? "/").slice(1));
    const out = view(path);
    if (out === undefined) return void res.writeHead(404).end();
    if (out !== "endless") return void res.writeHead(200, { "content-length": out.length }).end(out);
    res.writeHead(200); // no length: chunked, until the client hangs up
    const chunk = Buffer.alloc(8192, 0x20);
    const pump = () => {
      while (!res.destroyed && stats.endlessSent < 64 * 1024 * 1024) {
        stats.endlessSent += chunk.length;
        if (!res.write(chunk)) return void res.once("drain", pump);
      }
      res.end();
    };
    res.on("close", () => (stats.closed = true));
    pump();
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve({ url: `http://127.0.0.1:${server.address().port}/`, stats, close: () => server.closeAllConnections() || server.close() })));
}

/** A view of a repository folder with some paths replaced by `over` (bytes, or `"endless"`). */
const viewOf = (dir, over = {}) => (path) => (path in over ? over[path] : (() => { try { return readFileSync(join(dir, path)); } catch { return undefined; } })());

/** Runs the client (asynchronously: this process serves the repository while it runs). */
function client(md, args, env = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(CLIENT, ["--metadata-dir", md, ...args], { env: { ...process.env, ...env }, stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    child.stderr.setEncoding("utf8").on("data", (d) => (stderr += d));
    const timer = setTimeout(() => child.kill("SIGKILL"), 60_000);
    child.once("error", reject);
    child.once("close", (status) => {
      clearTimeout(timer);
      const text = stderr.trim();
      resolve({ status, code: /^([A-Z][A-Z_]+)(?: |:)/.exec(text)?.[1] ?? (status === 0 ? "ok" : `exit ${status}`), stderr: text });
    });
  });
}
const init = async (md, root) => {
  mkdirSync(md, { recursive: true });
  const r = await client(md, ["init", root]);
  if (r.status !== 0) throw new Error(`init: ${r.stderr}`);
};
const refresh = (md, url, env) => client(md, ["--metadata-url", `${url}metadata/`, "refresh"], env);
const download = (md, url, out, env) => client(md, ["--metadata-url", `${url}metadata/`, "--target-base-url", `${url}targets/`, "--target-name", TARGET, "--target-dir", out, "download"], env);
const at = (ms) => ({ FIN_TUF_FAKE_NOW_MS: String(Date.now() + ms) });

const versionsOf = (dir, role) => readdirSync(join(dir, "metadata")).filter((f) => f.endsWith(`.${role}.json`)).map((f) => Number.parseInt(f, 10)).sort((a, b) => a - b);
const latest = (dir, role) => versionsOf(dir, role).at(-1);
const meta = (dir, file) => readFileSync(join(dir, "metadata", file));
const targetPath = (dir) => {
  const base = join(dir, "targets", "acme", "alpha");
  return `targets/acme/alpha/${readdirSync(base).find((f) => f.endsWith(".1.1.0.finpkg"))}`;
};

const pkg = (version) => ({ id: "@acme/alpha", version, kind: "content", styles: ["s"] });

export async function run(work) {
  const rows = [];
  const expect = async (attack, want, pending, extra = "") => {
    const got = await pending;
    rows.push({ attack, want: [].concat(want), got: got.code, ok: [].concat(want).includes(got.code), extra });
    return got;
  };
  const reg = (name, opts) => makeRegistry(work, name, join(work, `reg-${name}`), ["acme"], opts);
  const root = (r) => join(r.dir, "metadata", "1.root.json");
  const fresh = async (name, r) => {
    const md = join(work, `md-${name}`);
    await init(md, root(r));
    return md;
  };
  const withServer = async (view, fn) => {
    const s = await serve(view);
    try {
      return await fn(s);
    } finally {
      s.close();
    }
  };

  // The baseline: the harness serves a healthy repository, and the client reads and downloads it.
  const base = reg("base");
  base.publish(pkg("1.0.0"));
  base.publish(pkg("1.1.0"));
  await withServer(viewOf(base.dir), async (s) => {
    const md = await fresh("base", base);
    await expect("baseline: refresh a healthy repository", "ok", refresh(md, s.url));
    await expect("baseline: download a signed target", "ok", download(md, s.url, join(work, "dl-base")));
  });

  // Rollback: the server serves what it served before the last publish.
  const rb = reg("rollback");
  rb.publish(pkg("1.0.0"));
  const old = join(work, "rollback-old");
  cpSync(rb.dir, old, { recursive: true });
  rb.publish(pkg("1.1.0"));
  {
    const md = await fresh("rollback", rb);
    await withServer(viewOf(rb.dir), async (s) => expect("rollback setup: the client trusts the newer repository", "ok", refresh(md, s.url)));
    await withServer(viewOf(old), async (s) => expect("rollback: an older repository (timestamp, snapshot, targets) after the newer", "TUF_ROLLBACK", refresh(md, s.url)));
    // The newer timestamp with the older snapshot behind it.
    const [sNew, sOld] = [latest(rb.dir, "snapshot"), latest(old, "snapshot")];
    await withServer(viewOf(rb.dir, { [`metadata/${sNew}.snapshot.json`]: meta(old, `${sOld}.snapshot.json`) }), async (s) => expect("rollback: the older snapshot under the newer timestamp (fresh client)", ["TUF_TARGET_MISMATCH", "TUF_ROLLBACK"], refresh(await fresh("rollback2", rb), s.url)));
  }

  // Freeze: the repository was signed with a one-day timestamp; the server keeps serving it.
  const fr = reg("freeze");
  fr.publish(pkg("1.0.0"));
  fr.publish(pkg("1.1.0"));
  mustPlugin(["registry", "resign-timestamp", fr.dir, "--keys", fr.keys, "--expires", "1"]);
  await withServer(viewOf(fr.dir), async (s) => {
    await expect("freeze: the one-day timestamp, half a day on", "ok", refresh(await fresh("freeze-a", fr), s.url, at(DAY_MS / 2)));
    await expect("freeze: the same timestamp served three days on", "TUF_EXPIRED", refresh(await fresh("freeze-b", fr), s.url, at(3 * DAY_MS)));
    const kept = await fresh("freeze-c", fr);
    refresh(kept, s.url, at(DAY_MS / 2));
    await expect("freeze: a client that trusted it before, three days on", "TUF_EXPIRED", refresh(kept, s.url, at(3 * DAY_MS)));
  });

  // Expired metadata: the thirty-day timestamp, then the one-year root, snapshot and targets.
  await withServer(viewOf(base.dir), async (s) => {
    await expect("expired: the timestamp, forty days on", "TUF_EXPIRED", refresh(await fresh("expired-a", base), s.url, at(40 * DAY_MS)));
    await expect("expired: everything, two years on", "TUF_EXPIRED", refresh(await fresh("expired-b", base), s.url, at(2 * 365 * DAY_MS)));
  });

  // Mix-and-match: parts of two states of one repository.
  const mm = makeRegistry(work, "mix", join(work, "reg-mix"), ["acme", "beta"]);
  mm.publish(pkg("1.0.0"));
  const mmOld = join(work, "mix-old");
  cpSync(mm.dir, mmOld, { recursive: true });
  mm.publish(pkg("1.1.0"));
  mm.publish({ id: "@beta/gamma", version: "1.0.0", kind: "content", styles: ["s"] }); // delegates beta in `targets`: a new version of it
  const [aNew, aOld] = [latest(mm.dir, "acme"), latest(mmOld, "acme")];
  await withServer(viewOf(mm.dir, { [`metadata/${aNew}.acme.json`]: meta(mmOld, `${aOld}.acme.json`) }), async (s) => expect("mix-and-match: an older publisher role under the newer snapshot", ["TUF_TARGET_MISMATCH", "TUF_ROLLBACK"], refresh(await fresh("mix-a", mm), s.url)));
  const [tNew, tOld] = [latest(mm.dir, "targets"), latest(mmOld, "targets")];
  if (tNew === tOld) throw new Error("the mix-and-match case needs a new targets version");
  await withServer(viewOf(mm.dir, { [`metadata/${tNew}.targets.json`]: meta(mmOld, `${tOld}.targets.json`) }), async (s) => expect("mix-and-match: an older targets under the newer snapshot", ["TUF_TARGET_MISMATCH", "TUF_ROLLBACK"], refresh(await fresh("mix-b", mm), s.url)));

  // Key rotation: the timestamp key is replaced; a client that trusted root 1 follows the chain,
  // and the old key signs nothing any more.
  const kr = reg("rotate");
  kr.publish(pkg("1.0.0"));
  const krOld = join(work, "rotate-old");
  cpSync(kr.dir, krOld, { recursive: true });
  mustPlugin(["keygen", kr.key("ts2")]);
  mustPlugin(["registry", "rotate-key", kr.dir, "--keys", kr.keys, "--role", "timestamp", "--new", kr.key("ts2")]);
  mustPlugin(["registry", "resign-timestamp", krOld, "--keys", kr.keys]); // the old key, on the old state, a newer timestamp
  {
    const md = await fresh("rotate", kr);
    await withServer(viewOf(kr.dir), async (s) => expect("key rotation: a client on root 1 follows the chain to root 2", "ok", refresh(md, s.url)));
    const stolen = meta(krOld, "timestamp.json");
    await withServer(viewOf(kr.dir, { "metadata/timestamp.json": stolen }), async (s) => expect("key rotation: a timestamp signed with the replaced key", ["TUF_SIGNATURE", "TUF_THRESHOLD"], refresh(md, s.url)));
    await withServer(viewOf(kr.dir, { "metadata/timestamp.json": stolen }), async (s) => expect("key rotation: the same, to a client that has not seen root 2", ["TUF_SIGNATURE", "TUF_THRESHOLD"], refresh(await fresh("rotate-b", kr), s.url)));
  }

  // Threshold: a root of two keys that must both sign; a rotation with one signature removed.
  const th = reg("threshold", { rootKeys: 2 });
  th.publish(pkg("1.0.0"));
  mustPlugin(["keygen", th.key("ts2")]);
  mustPlugin(["registry", "rotate-key", th.dir, "--keys", th.keys, "--role", "timestamp", "--new", th.key("ts2")]);
  {
    await withServer(viewOf(th.dir), async (s) => expect("threshold: a root 2 signed by both root keys", "ok", refresh(await fresh("threshold-a", th), s.url)));
    const second = JSON.parse(meta(th.dir, "2.root.json").toString("utf8"));
    second.signatures = second.signatures.slice(0, 1);
    await withServer(viewOf(th.dir, { "metadata/2.root.json": Buffer.from(JSON.stringify(second)) }), async (s) => expect("threshold: a root 2 with one of two signatures", ["TUF_THRESHOLD", "TUF_SIGNATURE"], refresh(await fresh("threshold-b", th), s.url)));
    const forged = JSON.parse(meta(th.dir, "2.root.json").toString("utf8"));
    forged.signatures[0].sig = forged.signatures[0].sig.replace(/^./, (c) => (c === "0" ? "1" : "0"));
    await withServer(viewOf(th.dir, { "metadata/2.root.json": Buffer.from(JSON.stringify(forged)) }), async (s) => expect("threshold: a root 2 with a damaged signature", ["TUF_THRESHOLD", "TUF_SIGNATURE"], refresh(await fresh("threshold-c", th), s.url)));
  }

  // Endless data: bodies that never end, and a target of the wrong bytes.
  {
    const md = await fresh("endless", base);
    await withServer(viewOf(base.dir, { "metadata/timestamp.json": "endless" }), async (s) => {
      const got = await refresh(md, s.url);
      await expect("endless data: an endless timestamp", ["REGISTRY_INVALID", "TUF_SIZE", "REGISTRY_UNREACHABLE"], got, `the server wrote ${Math.ceil(s.stats.endlessSent / 1024)} KiB before the hang-up`);
      rows.at(-1).ok &&= s.stats.endlessSent <= SLACK && got.status !== 0;
    });
    const snap = latest(base.dir, "snapshot");
    await withServer(viewOf(base.dir, { [`metadata/${snap}.snapshot.json`]: "endless" }), async (s) => {
      const got = await refresh(await fresh("endless2", base), s.url);
      await expect("endless data: an endless snapshot", ["REGISTRY_INVALID", "TUF_SIZE", "TUF_TARGET_MISMATCH", "REGISTRY_UNREACHABLE"], got, `the server wrote ${Math.ceil(s.stats.endlessSent / 1024)} KiB before the hang-up`);
      rows.at(-1).ok &&= s.stats.endlessSent <= 1024 * 1024 + SLACK && got.status !== 0;
    });
    const ok = await fresh("endless3", base);
    await withServer(viewOf(base.dir), (s) => refresh(ok, s.url));
    const path = targetPath(base.dir);
    await withServer(viewOf(base.dir, { [path]: "endless" }), async (s) => {
      const got = await download(ok, s.url, join(work, "dl-endless"));
      await expect("endless data: an endless target", ["TUF_TARGET_MISMATCH", "REGISTRY_INVALID", "TUF_SIZE", "REGISTRY_UNREACHABLE"], got, `the server wrote ${Math.ceil(s.stats.endlessSent / 1024)} KiB before the hang-up`);
      rows.at(-1).ok &&= s.stats.endlessSent <= SLACK && got.status !== 0;
    });
    const bytes = readFileSync(join(base.dir, path.replace(/^targets\//, "targets/")));
    const tampered = Buffer.from(bytes);
    tampered[tampered.length - 1] ^= 1;
    await withServer(viewOf(base.dir, { [path]: tampered }), async (s) => expect("tampered target: the right length, other bytes", "TUF_TARGET_MISMATCH", download(ok, s.url, join(work, "dl-tampered"))));
    await withServer(viewOf(base.dir, { [path]: Buffer.concat([bytes, Buffer.from("x")]) }), async (s) => expect("tampered target: longer than its signed length", ["TUF_TARGET_MISMATCH", "REGISTRY_INVALID"], download(ok, s.url, join(work, "dl-longer"))));
  }
  return rows;
}

const table = (rows) =>
  [
    "| Attack | Client must | Client did | Result |",
    "|---|---|---|---|",
    ...rows.map((r) => `| ${r.attack}${r.extra ? ` (${r.extra})` : ""} | ${r.want.join(" or ")} | ${r.got} | ${r.ok ? "refused as required" : "**DIFFERS**"} |`.replace("refused as required", r.want[0] === "ok" ? "accepted" : "refused")),
  ].join("\n");

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  const work = mkdtempSync(join(tmpdir(), "tuf-attacks-"));
  try {
    const rows = await run(work);
    const text = process.argv.includes("--json") ? `${JSON.stringify(rows, null, 2)}\n` : `${table(rows)}\n`;
    process.stdout.write(text);
    const out = process.argv.indexOf("--out");
    if (out > 0) writeFileSync(process.argv[out + 1], text);
    process.exitCode = rows.every((r) => r.ok) ? 0 : 1;
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}
