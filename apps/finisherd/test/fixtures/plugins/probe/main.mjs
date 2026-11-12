// A test-only plugin process (sp8b Task 4's tests; the SDKs come in Tasks 12 and 13): it speaks
// 5C's `fin-jobs` envelope by hand (NDJSON on stdin/stdout, `seq` from 1) as a worker of the
// `plugin` family, and serves `test.echo` jobs whose `params.value.op` says what to report or
// try (`command`: a `plugin.command` run's answer). `kinds.json` beside it, when present, replaces the kinds its `hello` names.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";

const warm = process.argv.includes("--warm");
let seq = 0;
const send = (m) => process.stdout.write(`${JSON.stringify({ v: 1, ...m, seq: ++seq })}\n`);
let kinds = [
  { kind: "test.echo", version: 1 },
  { kind: "plugin.command", version: 1 },
];
try {
  kinds = JSON.parse(readFileSync(new URL("./kinds.json", import.meta.url), "utf8"));
} catch {}
send({ t: "hello", minor: 0, role: "worker", family: "plugin", kinds, warm, pid: process.pid });
const alive = setInterval(() => send({ t: "alive" }), 500);
let served = 0;
/** Open host calls by id (`call` → `reply`). */
const replies = new Map();
let callId = 0;
const call = (method, params) =>
  new Promise((resolve) => {
    const id = ++callId;
    replies.set(id, resolve);
    send({ t: "call", id, method, params });
  });

/** What one attempt at `f` did: `ok`, or the error's code. */
async function attempt(f) {
  try {
    await f();
    return "ok";
  } catch (e) {
    return e?.code ?? String(e);
  }
}

async function run(job) {
  // `test.echo` carries the op in `value`, a `plugin.command` in its `args`.
  const v = job.params?.value ?? job.params?.args ?? {};
  switch (v.op) {
    case "env":
      return { pid: process.pid, env: process.env, argv: process.argv, execArgv: process.execArgv, cwd: process.cwd(), served: ++served };
    case "probe": {
      const fs = await import("node:fs");
      const out = {};
      for (const p of v.read ?? []) out[`read ${p}`] = await attempt(() => fs.readFileSync(p));
      for (const p of v.write ?? []) out[`write ${p}`] = await attempt(() => fs.writeFileSync(p, "x"));
      out.spawn = await attempt(async () => (await import("node:child_process")).spawnSync(process.execPath, ["-e", "0"]));
      out.worker = await attempt(async () => {
        const { Worker } = await import("node:worker_threads");
        await new Worker("0", { eval: true }).terminate();
      });
      return out;
    }
    case "calls": {
      // Each `{method, params}` in turn (after `pauseMs` from the one before); the replies.
      const out = [];
      for (const [i, c] of (v.calls ?? []).entries()) {
        if (i > 0 && v.pauseMs) await new Promise((r) => setTimeout(r, v.pauseMs));
        const r = await call(c.method, c.params);
        // `digest`: a page's bytes as their length and SHA-256 (a result is one 1 MiB line too).
        if (v.digest && typeof r.value?.bytesB64 === "string") {
          const bytes = Buffer.from(r.value.bytesB64, "base64");
          const { createHash } = await import("node:crypto");
          const { bytesB64: _, ...rest } = r.value;
          out.push({ value: { ...rest, length: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") } });
          continue;
        }
        out.push(r.error === undefined ? { value: r.value } : { error: r.error });
      }
      return { pid: process.pid, replies: out };
    }
    case "stderr":
      for (const l of v.lines ?? []) process.stderr.write(`${l}\n`);
      return { pid: process.pid };
    case "command": {
      // A command run (Task 9): waits for `waitFor` in its temp directory, reads the snapshot
      // when `read`, sleeps `sleepMs`, then answers `out` (a `CommandOut` as JSON) with `$REV`
      // replaced by the rev it read.
      if (v.waitFor) {
        const { existsSync } = await import("node:fs");
        const { join } = await import("node:path");
        while (!existsSync(join(process.env.TMPDIR, v.waitFor))) await new Promise((r) => setTimeout(r, 20));
      }
      let rev = -1;
      if (v.read) rev = (await call("project.snapshot", {})).value?.rev ?? -2;
      if (v.sleepMs) await new Promise((r) => setTimeout(r, v.sleepMs));
      return JSON.parse((v.out ?? '{"kind":"data","value":null}').replaceAll("$REV", String(rev)));
    }
    case "echo":
      // A `plugin.command` run's arguments as its data (`fin`'s argument mapping, Task 10).
      return { kind: "data", value: v };
    case "sleep":
      return new Promise((resolve) => setTimeout(() => resolve({ pid: process.pid, slept: v.ms }), v.ms));
    case "crash":
      // `say`: stderr lines first (a crash's `stderrTail`).
      for (const l of v.say ?? []) process.stderr.write(`${l}\n`);
      if (v.say) await new Promise((r) => process.stderr.write("", r));
      process.exit(3);
      break;
    case "hang":
      clearInterval(alive);
      return new Promise(() => {});
    default:
      return { pid: process.pid };
  }
}

const rl = createInterface({ input: process.stdin });
// Ends once stdout is flushed: no handle is left.
const stop = () => {
  clearInterval(alive);
  rl.close();
};
rl.on("line", async (line) => {
  const m = JSON.parse(line);
  if (m.t === "shutdown") return stop();
  if (m.t === "reply") {
    replies.get(m.id)?.(m);
    replies.delete(m.id);
    return;
  }
  if (m.t !== "start") return;
  const value = await run(m.job);
  send({ t: "result", job: m.job.id, value });
  if (warm) send({ t: "ready" });
  else stop();
});
rl.on("close", stop);
