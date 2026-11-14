// A plugin with hook handlers for the `hook` profile's tests (crates/fin-plugin-test/tests/
// profiles.rs). It speaks the fin-jobs envelope by hand:
// - `guard` (PreCommand) blocks a command and reads the project on the way (`project.view`);
// - `audit` (PreCommand and JobDone, one handler on two events) answers context naming the event;
// - `note` (SessionStart) allows;
// - `rewrite` (PreMerge) answers a `modify` reply, which v1 does not have;
// - `slow` (PreMerge) answers after 3 s, past its 1 s timeout.
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

function end(job, fields) {
  if (fields.error !== undefined) send("error", { job: job.id, error: fields.error });
  else send("result", { job: job.id, value: fields.value });
  if (warm) send("ready", {});
  else send("alive", {}, () => process.exit(0));
}

async function hook(job) {
  const { handler, request } = job.params;
  switch (handler) {
    case "guard": {
      await call("project.view", {});
      return end(job, { value: { reply: "block", reason: `no ${request.payload.command} here` } });
    }
    case "audit":
      return end(job, { value: { reply: "context", text: `${request.event} seen` } });
    case "note":
      return end(job, { value: { reply: "allow" } });
    case "rewrite":
      return end(job, { value: { reply: "modify", payload: { into: "elsewhere" } } });
    case "slow":
      return void setTimeout(() => end(job, { value: { reply: "allow" } }), 3000);
    default:
      return end(job, { error: { code: "PLUGIN_PROTOCOL", message: `unknown hook ${handler}`, details: { reason: "unknownHook", handler } } });
  }
}

function run(job) {
  if (job.kind === "plugin.hook") return void hook(job);
  return end(job, { value: { kind: "data", value: job.params.args } });
}

const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const m = JSON.parse(line);
  if (m.t === "reply") {
    const resolve = pending.get(m.id);
    pending.delete(m.id);
    resolve?.(m);
  } else if (m.t === "start") {
    run(m.job);
  } else if (m.t === "shutdown") {
    process.exit(0);
  }
});
rl.on("close", () => process.exit(70));

const kinds = [
  { kind: "plugin.command", version: 1 },
  { kind: "plugin.hook", version: 1 },
];
send("hello", { pid: process.pid, minor: 0, role: "worker", family: "plugin", kinds, ...(warm ? { warm: true } : {}) });
