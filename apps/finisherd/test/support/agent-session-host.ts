// A `finisherd` for `fin`'s Rust agent-session tests (plan 7A Task 8b, B1/B3/B4): the real
// server (`serve`), plus a way to open, revoke and inspect agent sessions, which the RPCs give
// people only from Task 12 on. Run as
//
//   node apps/finisherd/test/support/agent-session-host.ts --server-data <dir> --requests <dir>
//
// The test writes `<n>.req.json` into the requests folder; the host answers with `<n>.res.json`
// (written temp-then-rename) and removes the request. Requests:
//   {op: "open", projectId, mode?, role?, base?}   → {sessionId, credential, rootBranch?}
//   {op: "revoke", sessionId}                      → {}
//   {op: "txs", sessionId}                         → {txs: [{txId, callId, attribution}]}
//   {op: "state", sessionId}                       → {defaultBranch, tries}
//   {op: "listener"}                               → {endpoint}  (starts the agent listener, 7C Task 6)
// A failure answers {error: {code, message}}.
import { mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import type { Mode } from "@finisher/contracts";
import { Effect } from "effect";
import type { AgentCore } from "../../src/agent/core.ts";
import type { AgentListener } from "../../src/agent/listener.ts";
import { toFinError } from "../../src/errors.ts";
import { serve, StderrLogger } from "../../src/main.ts";
import { open } from "./agent-open.ts";
import { connectProved, type NdjsonClient, type ServerInfo } from "./ndjson-client.ts";

const { values } = parseArgs({ options: { "server-data": { type: "string" }, requests: { type: "string" } } });
const serverData = values["server-data"];
const requests = values.requests;
if (serverData === undefined || requests === undefined) throw new Error("--server-data and --requests are required");

let core: AgentCore | undefined;
let listener: AgentListener | undefined;
let info: ServerInfo | undefined;
let starter: NdjsonClient | undefined;

type Request =
  | { op: "open"; projectId: string; mode?: Mode; role?: string; base?: string }
  | { op: "revoke"; sessionId: string }
  | { op: "txs"; sessionId: string }
  | { op: "state"; sessionId: string }
  | { op: "listener" };

async function handle(r: Request): Promise<unknown> {
  const c = core!;
  switch (r.op) {
    case "open": {
      starter ??= await connectProved({ info: info!, serverData: serverData! });
      const { session, credentialPath } = await open(c, starter, r.projectId, {
        ...(r.mode === undefined ? {} : { mode: r.mode }),
        ...(r.role === undefined ? {} : { role: r.role }),
        ...(r.base === undefined ? {} : { base: r.base }),
      });
      return { sessionId: session.id, credential: credentialPath, ...(session.rootBranch === undefined ? {} : { rootBranch: session.rootBranch }) };
    }
    case "revoke":
      await Effect.runPromise(c.credentials.revokeForSession(r.sessionId));
      return {};
    case "txs":
      return { txs: c.store.txsOfSession(r.sessionId).map((t) => ({ txId: t.txId, callId: t.callId, attribution: t.attribution })) };
    case "state":
      return c.store.state(r.sessionId);
    case "listener":
      return { endpoint: await listener!.ensure() };
  }
}

let busy = false;
async function poll(): Promise<void> {
  if (busy || core === undefined || info === undefined) return;
  busy = true;
  try {
    for (const name of readdirSync(requests!).filter((n) => n.endsWith(".req.json")).sort()) {
      const path = join(requests!, name);
      let answer: unknown;
      try {
        answer = await handle(JSON.parse(readFileSync(path, "utf8")) as Request);
      } catch (e) {
        const err = toFinError(e);
        answer = { error: { code: err.code, message: err.message } };
      }
      const out = join(requests!, name.replace(/\.req\.json$/, ".res.json"));
      writeFileSync(`${out}.tmp`, JSON.stringify(answer));
      renameSync(`${out}.tmp`, out);
      rmSync(path, { force: true });
    }
  } finally {
    busy = false;
  }
}

mkdirSync(requests, { recursive: true });
setInterval(() => void poll(), 20);

await Effect.runPromise(
  serve({
    serverData,
    onAgentCore: (c) => (core = c),
    onAgentListener: (l) => (listener = l),
    onReady: (i) => (info = i as ServerInfo),
  }).pipe(Effect.provide(StderrLogger)),
);
process.exit(0);
