// A Node child that runs the job engine layer over a server data, for `kill.test.ts` (5C-1 plan
// Task 12): `node engine-child.ts <serverData> [<kind>=<params JSON>]…` builds the engine with
// the test kinds (cgroups and the watchdog as in production), submits each job, and prints one
// JSON line per fact on stdout (logs go to stderr):
//   {"type":"ready","pid":…,"caps":{…}}
//   {"type":"job","id":…,"kind":…,"worker":<pid>,"pids"?:{worker,mid,grandchild}}  (test.spawn's file)
// then waits. On stdin, `rows` prints {"type":"rows","rows":[{id,kind,state,code?}],"jobsDir":[…]}
// and `close` closes the engine (Decision 29), prints the rows, then {"type":"closed"} and exits.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { Effect, Layer, Logger } from "effect";
import { ControlStoreTag, openControlStore } from "../../src/control/db.ts";
import { JobEventSinkTag, memorySink } from "../../src/jobs/events.ts";
import { buildJobEngine } from "../../src/jobs/layer.ts";
import { JobStore } from "../../src/jobs/store.ts";

const [dataArg, ...specs] = process.argv.slice(2);
if (dataArg === undefined) throw new Error("usage: engine-child.ts <serverData> [<kind>=<params JSON>]…");
const serverData: string = dataArg;

const say = (fact: Record<string, unknown>): void => void process.stdout.write(`${JSON.stringify(fact)}\n`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const OWNER = { kind: "local", id: "local" } as const;

const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
const store = new JobStore(control);
const logger = Logger.layer([Logger.withConsoleError(Logger.formatSimple)]);
const engine = await Effect.runPromise(
  buildJobEngine(serverData, { testKinds: true }).pipe(
    Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, memorySink()), logger)),
  ),
);
say({ type: "ready", pid: process.pid, caps: engine.workers().caps });

async function until<A>(f: () => A | undefined, ms = 20_000): Promise<A> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("engine-child: timed out");
    await sleep(10);
  }
}

for (const spec of specs) {
  const at = spec.indexOf("=");
  const kind = at < 0 ? spec : spec.slice(0, at);
  const params = at < 0 ? {} : JSON.parse(spec.slice(at + 1));
  const v = await Effect.runPromise(engine.submit(OWNER, { kind, params }));
  const worker = await until(() => engine.workers().processes.find((p) => p.job === v.id && p.role === "worker" && p.pid !== undefined)?.pid);
  let pids: unknown;
  if (kind === "test.spawn") {
    const file = join(serverData, "jobs", v.id, (params as { out: string }).out);
    pids = await until(() => {
      try {
        return JSON.parse(readFileSync(file, "utf8").replace(/\0+$/, "")) as unknown;
      } catch {
        return undefined;
      }
    });
  }
  say({ type: "job", id: v.id, kind, worker, ...(pids === undefined ? {} : { pids }) });
}

function rows(): void {
  const all = store.list({ limit: 1000 }).jobs.map((j) => ({ id: j.id, kind: j.kind, state: j.state, ...(j.error === undefined ? {} : { code: j.error.code }) }));
  say({ type: "rows", rows: all, jobsDir: readdirSync(join(serverData, "jobs")) });
}

const keepAlive = setInterval(() => {}, 60_000);
for await (const line of createInterface({ input: process.stdin })) {
  if (line === "rows") rows();
  else if (line === "close") {
    await Effect.runPromise(engine.close());
    rows();
    await control.close();
    say({ type: "closed" });
    clearInterval(keepAlive);
    process.exit(0);
  }
}
