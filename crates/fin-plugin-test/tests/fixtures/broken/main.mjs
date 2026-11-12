// A deliberately broken plugin for the `command` profile's tests (crates/fin-plugin-test/tests/
// profiles.rs). It speaks the fin-jobs envelope by hand and ignores `cancel`:
// - `sneaky` reads the project without declaring `project:read`;
// - `slow` answers after 5 s, past its 1 s timeout;
// - `crash` exits in the middle of the command;
// - `leaky` logs the cookie the fake API set and a path, and writes the cookie to stderr;
// - `bad-tx` proposes an op fin-core refuses; `readonly-tx` returns a transaction from a
//   command that does not say `writes`;
// - `far-fetch` fetches an origin the manifest does not name;
// - `odd-calls` calls a later batch's method, an unknown one, and `log` with bad params;
// - `no-ready` leaves out `ready` after its job in a warm process.
import { createInterface } from "node:readline";

const warm = process.argv.at(-1) === "--warm";
let seq = 0;
const send = (t, fields, then) => process.stdout.write(`${JSON.stringify({ v: 1, t, seq: ++seq, ...fields })}\n`, then);

let nextId = 0;
const pending = new Map();
const call = (method, params) =>
  new Promise((resolve) => {
    const id = ++nextId;
    pending.set(id, resolve);
    send("call", { id, method, params });
  });

const data = (value) => ({ value: { kind: "data", value } });

function end(job, fields, ready = true) {
  if (fields.error !== undefined) send("error", { job: job.id, error: fields.error });
  else send("result", { job: job.id, value: fields.value });
  if (warm && ready) send("ready", {});
  else if (warm) return;
  else send("alive", {}, () => process.exit(0));
}

async function run(job) {
  const { command } = job.params;
  switch (command) {
    case "sneaky": {
      const r = await call("project.snapshot", {});
      return end(job, data({ refused: r.error?.code ?? null }));
    }
    case "slow":
      return void setTimeout(() => end(job, data({})), 5000);
    case "crash":
      return process.exit(3);
    case "leaky": {
      const head = await call("net.fetch", { request: { method: "GET", url: "https://api.example.org/" } });
      const cookie = head.value?.headers?.find((h) => h.name === "set-cookie")?.value ?? "";
      await call("log", { level: "info", msg: `the session is ${cookie}` });
      await call("log", { level: "warn", msg: "saved to /home/someone/notes/plugin.txt" });
      process.stderr.write(`debug: ${cookie}\n`);
      return end(job, data({}));
    }
    case "bad-tx":
      return end(job, { value: { kind: "tx", ops: [{ op: "item.delete", item: "no-such-item", mode: "lift" }] } });
    case "readonly-tx":
      return end(job, { value: { kind: "tx", ops: [{ op: "item.delete", item: "adj1", mode: "lift" }] } });
    case "far-fetch": {
      const r = await call("net.fetch", { request: { method: "GET", url: "https://elsewhere.example.com/" } });
      return end(job, data({ refused: r.error?.code ?? null }));
    }
    case "odd-calls": {
      await call("media.pcm", {});
      await call("no.such.method", {});
      await call("log", { level: "loud", msg: "x" });
      return end(job, data({}));
    }
    case "no-ready":
      return end(job, data({}), false);
    default:
      return end(job, { error: { code: "PLUGIN_PROTOCOL", message: `unknown command ${command}`, details: { reason: "unknownCommand", command } } });
  }
}

const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const m = JSON.parse(line);
  if (m.t === "reply") {
    const resolve = pending.get(m.id);
    pending.delete(m.id);
    resolve?.(m);
  } else if (m.t === "start") {
    void run(m.job);
  } else if (m.t === "shutdown") {
    process.exit(0);
  }
});
rl.on("close", () => process.exit(70));

send("hello", { pid: process.pid, minor: 0, role: "worker", family: "plugin", kinds: [{ kind: "plugin.command", version: 1 }], ...(warm ? { warm: true } : {}) });
