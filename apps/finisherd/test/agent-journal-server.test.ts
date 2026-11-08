// The agent journal on a real server (plan 7A Task 10; spec §12.1, §12.3; Review Focus 4, 5):
// every `log.commit` of a call carries its `FIN_CALL`, its `agent.toolResult` lists exactly
// those commits; the session comes from the principal; purge and export never touch the
// conversations in `control.db`.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Exit, Scope } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentJournal } from "../src/agent/journal.ts";
import { AgentStore } from "../src/agent/store.ts";
import { openControlStore } from "../src/control/db.ts";
import { makeDiagnostics } from "../src/diagnostics.ts";
import { AgentServer, open } from "./support/agent-server.ts";
import { agentEntries, entries, type Entry, session as sessionOf } from "./support/agent-journal.ts";
import { addAsset, commit, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "agent-journal-server-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01T${String(++n).padStart(23, "0")}`;

async function until<A>(f: () => A | undefined, what: string, ms = 8000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 25));
  }
}

const generation = async (c: NdjsonClient, projectId: string, branch: string) =>
  ok<{ generation: unknown }>(await c.call("snapshot.get", { projectId, branch })).generation;

describe("on a server", () => {
  let server: AgentServer;
  beforeEach(() => {
    server = new AgentServer(serverData);
  });
  afterEach(async () => {
    await server.stop();
  });

  async function setUp() {
    await server.start();
    const local = await server.local();
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
    const one = await open(server.core, local, projectId);
    const two = await open(server.core, local, projectId);
    const a = await server.agent(one.credentialPath);
    return { projectId, s1: one.session, s2: two.session, a };
  }

  const write = async (a: NdjsonClient, projectId: string, branch: string, headers: ReadonlyArray<[string, string]>) => {
    const txId = id();
    ok(await a.call("tx.commit", { projectId, tx: { ...tx(txId, 1, [insert(0)]), branch }, generation: await generation(a, projectId, branch) }, headers));
    return txId;
  };

  test("call_join: each log.commit of a call carries its FIN_CALL; its agent.toolResult lists exactly those commits", async () => {
    const { projectId, s1, a } = await setUp();
    const root = s1.rootBranch!;
    const runId = server.core.ids.ulid();
    const callId = server.core.ids.ulid();
    const s = server.core.store.get(s1.id)!;
    server.core.journal.toolCall({ session: s, runId, callId, providerId: "call_1", tool: "fin", input: { argv: ["item", "trim"] } });
    const inCall = [await write(a, projectId, root, [["fin-actor-call", callId]]), await write(a, projectId, root, [["fin-actor-call", callId]])];
    const outside = await write(a, projectId, root, []);
    server.core.journal.toolResult({ sessionId: s1.id, runId, callId, ok: true, output: { txIds: inCall }, synthetic: false });
    // `session_txs` has every agent commit, whatever its attribution.
    expect(server.core.store.txsOfSession(s1.id).map((t) => [t.txId, t.callId, t.attribution])).toEqual([
      [inCall[0], callId, "exact"],
      [inCall[1], callId, "exact"],
      [outside, null, "none"],
    ]);
    const events = () => entries(serverData, "events");
    const result = await until(() => events().find((e) => e.kind === "agent.toolResult"), "agent.toolResult");
    const commits = await until(() => {
      const cs = events().filter((e) => e.kind === "log.commit" && e.ctx?.actor?.session === s1.id);
      return cs.length === 3 ? cs : undefined;
    }, "three log.commit");
    const ofCall = commits.filter((c: Entry) => c.ctx?.actor?.call === callId);
    expect(ofCall.map((c) => c.ctx!.txId)).toEqual(inCall);
    expect(commits.find((c) => c.ctx?.txId === outside)!.ctx!.actor.call).toBeUndefined();
    expect(result.payload).toMatchObject({ callId, ok: true, attribution: "exact", txIds: inCall });
    expect(result.ctx).toMatchObject({ project: projectId, branch: root, actor: { kind: "agent", session: s1.id, call: callId } });
    const call = events().find((e) => e.kind === "agent.toolCall")!;
    expect(call.payload.callId).toBe(callId);
  });

  test("agent_header_cannot_claim_another_session: the session is the principal's; only the call comes from the header", async () => {
    const { projectId, s1, s2, a } = await setUp();
    const callId = server.core.ids.ulid();
    const txId = await write(a, projectId, s1.rootBranch!, [
      ["fin-actor-session", s2.id],
      ["fin-actor-kind", "user"],
      ["fin-actor-call", callId],
    ]);
    const c = await until(() => entries(serverData, "events").find((e) => e.kind === "log.commit" && e.ctx?.txId === txId), "the log.commit");
    expect(c.ctx!.actor).toEqual({ kind: "agent", session: s1.id, call: callId });
    expect(server.core.store.txsOfSession(s2.id)).toEqual([]);
    expect(server.core.store.txsOfCall(callId).map((t) => t.sessionId)).toEqual([s1.id]);
  });
});

// Purge and export --------------------------------------------------------------------------

/** Every row of the conversation tables, and every file under the session folders, hashed. */
function conversations(dir: string): string {
  const db = openControlStore(dir, { serverId: "sid", checkpoint: false });
  try {
    const rows = ["sessions", "runs", "nodes", "approvals", "session_txs"].map((t) => [
      t,
      db.db
        .prepare(`SELECT * FROM ${t}`)
        .all()
        .map((r) => JSON.stringify(r))
        .sort(),
    ]);
    const files: string[] = [];
    const walk = (d: string) => {
      if (!existsSync(d)) return;
      for (const e of readdirSync(d)) {
        const p = join(d, e);
        if (statSync(p).isDirectory()) walk(p);
        else files.push(`${p}:${createHash("sha256").update(readFileSync(p)).digest("hex")}`);
      }
    };
    walk(join(dir, "agents"));
    return JSON.stringify({ rows, files });
  } finally {
    void db.close();
  }
}

test("purge_and_export_do_not_touch_conversations: diagnostics purge --all and content exports leave control.db and session blobs as they were", async () => {
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ privacy: { diagnostics: "on", training: "on" } }));
  const scope = await Effect.runPromise(Scope.make());
  try {
    const diagnostics = await Effect.runPromise(Scope.provide(scope)(makeDiagnostics(serverData, { settingsPollMs: 60_000 })));
    const clock = manualClock(Date.now());
    const control = openControlStore(serverData, { serverId: "sid", checkpoint: false });
    const store = new AgentStore(control, serverData, clock, systemIds(clock));
    const journal = new AgentJournal({ diagnostics: diagnostics.journal, traces: diagnostics.traces, store, ids: systemIds(clock), clock });
    const P = "01jb0000000000000000000001";
    diagnostics.journal.setProjectPolicy(P, "default");
    const s = await store.createSession({ ...sessionOf({ id: systemIds(clock).ulid(), projectId: P }), rootBranch: "br1" });
    store.apply(s.id, { kind: "ready" });
    const { run } = store.startRun(s.id);
    // A conversation with a node large enough to be a blob beside the session.
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "userMessage", payload: { text: "trim the intro" } });
    await store.appendNode({ sessionId: s.id, runId: run.id, kind: "assistantMessage", payload: { text: "x".repeat(200_000) } });
    journal.capture({ session: s, runId: run.id, request: JSON.stringify({ model: "m", messages: [{ role: "user", content: "trim the intro" }] }), response: [] });
    journal.turn({ session: s, runId: run.id, usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 0 } });
    await diagnostics.journal.status();
    await diagnostics.traces.status();
    expect(agentEntries(serverData, "agent-traces")).toHaveLength(1);
    await control.close();
    const before = conversations(serverData);
    const seen = JSON.parse(before) as { rows: [string, string[]][]; files: string[] };
    expect(Object.fromEntries(seen.rows).nodes).toHaveLength(2);
    expect(seen.files.some((f) => f.includes(s.id))).toBe(true); // the large node's blob
    // A content export of each store, then a purge of everything.
    for (const store of ["diagnostics", "agentTraces"] as const) {
      const page = await Effect.runPromise(diagnostics.export({ store, limit: 1000 }, false));
      expect(page.lines.length).toBeGreaterThan(0);
    }
    const report = await Effect.runPromise(diagnostics.purge("all"));
    expect(report.removed).toBeGreaterThan(0);
    expect(agentEntries(serverData, "events")).toEqual([]);
    expect(agentEntries(serverData, "agent-traces")).toEqual([]);
    expect(conversations(serverData)).toBe(before);
  } finally {
    await Effect.runPromise(Scope.close(scope, Exit.void));
  }
});
