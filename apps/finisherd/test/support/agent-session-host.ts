// A `finisherd` for `fin`'s Rust agent-session tests (plan 7A Task 8b, B1/B3/B4): the real
// server (`serve`), plus a way to open, revoke and inspect agent sessions, which the RPCs give
// people only from Task 12 on. Run as
//
//   node apps/finisherd/test/support/agent-session-host.ts --server-data <dir> --requests <dir>
//     [--fake-scripts <file>]
//
// `--fake-scripts`: the `fake` harness's scripts (a JSON list, `agent/fake.ts`), for a
// test-hooks fin-node build run with `FIN_TEST_HOOKS=1` (7B Task 6: children run on it).
//
// The test writes `<n>.req.json` into the requests folder; the host answers with `<n>.res.json`
// (written temp-then-rename) and removes the request. Requests:
//   {op: "open", projectId, mode?, role?, base?, harness?} → {sessionId, credential, rootBranch?}
//   {op: "revoke", sessionId}                      → {}
//   {op: "txs", sessionId}                         → {txs: [{txId, callId, attribution}]}
//   {op: "state", sessionId}                       → {defaultBranch, tries}
//   {op: "ask", sessionId, command}                → {rule}: a local `ask` rule for the session, and a run
//   {op: "answer", id, decision}                   → {approval}: a person's answer (7B Task 5)
//   {op: "expireAll"}                              → {}: every open approval expired (a clock past them all)
//   {op: "listener"}                               → {endpoint}  (starts the agent listener, 7C Task 6)
// A failure answers {error: {code, message}}.
import { mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";
import type { Mode } from "@finisher/contracts";
import { Effect } from "effect";
import type { AgentCore } from "../../src/agent/core.ts";
import type { FakeScript } from "../../src/agent/fake.ts";
import type { AgentRuntimeParts } from "../../src/agent/wiring.ts";
import type { AgentListener } from "../../src/agent/listener.ts";
import { toFinError } from "../../src/errors.ts";
import { serve, StderrLogger } from "../../src/main.ts";
import { open } from "./agent-open.ts";
import { connectProved, type NdjsonClient, type ServerInfo } from "./ndjson-client.ts";

const { values } = parseArgs({ options: { "server-data": { type: "string" }, requests: { type: "string" }, "fake-scripts": { type: "string" } } });
const serverData = values["server-data"];
const requests = values.requests;
if (serverData === undefined || requests === undefined) throw new Error("--server-data and --requests are required");
const fakeScripts = values["fake-scripts"] === undefined ? undefined : (JSON.parse(readFileSync(values["fake-scripts"], "utf8")) as FakeScript[]);

let core: AgentCore | undefined;
let agents: AgentRuntimeParts | undefined;
let listener: AgentListener | undefined;
let info: ServerInfo | undefined;
let starter: NdjsonClient | undefined;

type Request =
  | { op: "open"; projectId: string; mode?: Mode; role?: string; base?: string; harness?: "builtin" | "fake" }
  | { op: "revoke"; sessionId: string }
  | { op: "txs"; sessionId: string }
  | { op: "state"; sessionId: string }
  | { op: "ask"; sessionId: string; command: string }
  | { op: "answer"; id: string; decision: "allow" | "deny" }
  | { op: "expireAll" }
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
        ...(r.harness === undefined ? {} : { harness: r.harness }),
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
    case "ask": {
      const s = c.store.get(r.sessionId)!;
      const rule = agents!.ruleStore.add({ id: c.ids.ulid(), effect: "ask", command: r.command, projectId: s.projectId, sessionId: s.id, source: "test", createdAt: c.clock.now() });
      if (c.store.activeRun(s.id) === null) c.store.startRun(s.id, { trigger: "prompt" });
      return { rule };
    }
    case "answer": {
      const owner = { kind: "console", id: "owner", scopes: [], projects: "*", branches: "*", human: true } as const;
      return Effect.runPromise(Effect.map(agents!.approvals.answer(r.id, r.decision, owner), (a) => ({ approval: a.approval })));
    }
    case "expireAll":
      c.store.expireOverdueApprovals(Number.MAX_SAFE_INTEGER);
      return {};
    case "listener":
      return { endpoint: await listener!.ensure() };
  }
}

let busy = false;
async function poll(): Promise<void> {
  if (busy || core === undefined || agents === undefined || info === undefined) return;
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
    ...(fakeScripts === undefined ? {} : { agents: { fakeScripts } }),
    onAgentRuntime: (a) => (agents = a),
    onAgentListener: (l) => (listener = l),
    onReady: (i) => (info = i as ServerInfo),
  }).pipe(Effect.provide(StderrLogger)),
);
process.exit(0);
