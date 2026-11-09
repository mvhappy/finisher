// A test-only plugin process (sp8b Task 4's tests; the SDKs come in Tasks 12 and 13): it speaks
// 5C's `fin-jobs` envelope by hand (NDJSON on stdin/stdout, `seq` from 1) as a worker of the
// `plugin` family, and serves `test.echo` jobs whose `params.value.op` says what to report or
// try. `kinds.json` beside it, when present, replaces the kinds its `hello` names.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline";

const warm = process.argv.includes("--warm");
let seq = 0;
const send = (m) => process.stdout.write(`${JSON.stringify({ v: 1, ...m, seq: ++seq })}\n`);
let kinds = [{ kind: "test.echo", version: 1 }];
try {
  kinds = JSON.parse(readFileSync(new URL("./kinds.json", import.meta.url), "utf8"));
} catch {}
send({ t: "hello", minor: 0, role: "worker", family: "plugin", kinds, warm, pid: process.pid });
const alive = setInterval(() => send({ t: "alive" }), 500);
let served = 0;

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
  const v = job.params?.value ?? {};
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
    case "sleep":
      return new Promise((resolve) => setTimeout(() => resolve({ pid: process.pid, slept: v.ms }), v.ms));
    case "crash":
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
  if (m.t !== "start") return;
  const value = await run(m.job);
  send({ t: "result", job: m.job.id, value });
  if (warm) send({ t: "ready" });
  else stop();
});
rl.on("close", stop);
