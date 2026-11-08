// The agent store's statement budget (plan 7A Task 3b, 5A's per-statement budget): with 10k
// sessions and 1M nodes, every statement a request or a model turn runs stays at p99 ≤ 1 ms
// on the Linux VM. Usage: node apps/finisherd/scripts/agent-store-budget.ts [<dir>] [<sessions>] [<nodes>]
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { AgentStore } from "../src/agent/store.ts";
import { openControlStore } from "../src/control/db.ts";

export interface AgentBudgetResult {
  readonly sessions: number;
  readonly nodes: number;
  readonly p99Ms: Record<string, number>;
  readonly p50Ms: Record<string, number>;
}

function pct(samples: number[], p: number): number {
  const sorted = [...samples].sort((a, b) => a - b);
  return Number((sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * p))] ?? 0).toFixed(3));
}

async function time(n: number, f: (i: number) => unknown): Promise<number[]> {
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = process.hrtime.bigint();
    await f(i);
    out.push(Number(process.hrtime.bigint() - t) / 1e6);
  }
  return out;
}

const id = (prefix: string, i: number): string => `${prefix}${String(i).padStart(24, "0")}`;

/** Seeds `sessions` sessions holding `nodes` nodes in all, then times the per-request calls. */
export async function measureAgentStoreBudget(dir: string, sessions = 10_000, nodes = 1_000_000): Promise<AgentBudgetResult> {
  const control = openControlStore(dir, { serverId: "budget", checkpoint: false });
  try {
    const clock = manualClock();
    const store = new AgentStore(control, dir, clock, systemIds(clock));
    const perSession = Math.max(1, Math.floor(nodes / sessions));
    // Raw inserts: the seed is not what is measured.
    control.tx(() => {
      const db = control.db;
      const s = db.prepare(
        "INSERT INTO sessions (id, projectId, harness, provider, model, weights, role, mode, rootBranch, baseBranch, baseSeq, status, " +
          "credentialId, budget, usage, createdBy, createdAt, endedAt) VALUES (?, ?, 'builtin', 'openrouter', 'm', 'open', 'editor', 'draft', " +
          "?, 'main', 0, ?, ?, '{}', ?, '{}', ?, ?)",
      );
      const st = db.prepare("INSERT INTO session_state (sessionId, defaultBranch) VALUES (?, ?)");
      const r = db.prepare("INSERT INTO runs (id, sessionId, seq, trigger, status, startedAt, usage) VALUES (?, ?, 1, 'prompt', ?, 0, ?)");
      const n = db.prepare("INSERT INTO nodes (id, runId, sessionId, seq, kind, callId, payload, createdAt) VALUES (?, ?, ?, ?, ?, ?, '{}', 0)");
      const t = db.prepare("INSERT INTO session_txs (txId, sessionId, callId, attribution, at) VALUES (?, ?, ?, 'exact', 0)");
      const usage = JSON.stringify({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 0 });
      for (let i = 0; i < sessions; i++) {
        const sid = id("S", i);
        const ended = i % 2 === 0;
        s.run(sid, `P${i % 50}`, `agent/editor-${i}`, ended ? "ended" : "idle", sid, usage, i, ended ? i : null);
        st.run(sid, `agent/editor-${i}`);
        r.run(id("R", i), sid, "completed", usage);
        for (let k = 0; k < perSession; k++) {
          const kind = k % 4 === 0 ? "toolCall" : k % 4 === 1 ? "toolResult" : "assistantMessage";
          const call = kind === "assistantMessage" ? null : id("C", i * perSession + k - (k % 4));
          n.run(id("N", i * perSession + k), id("R", i), sid, k + 1, kind, call);
          if (kind === "toolResult") t.run(id("T", i * perSession + k), sid, call);
        }
      }
    });
    const live = id("S", 1);
    const runId = store.startRun(live).run.id;
    // Warm the statement caches.
    store.get(live);
    store.nodes(live, 1, 50);
    const p99Ms: Record<string, number> = {};
    const p50Ms: Record<string, number> = {};
    const record = (name: string, samples: number[]) => {
      p99Ms[name] = pct(samples, 0.99);
      p50Ms[name] = pct(samples, 0.5);
    };
    record("get", await time(2000, (i) => store.get(id("S", i % sessions))));
    record("nodesPage", await time(1000, (i) => store.nodes(id("S", i % sessions), 1, 50)));
    record("appendNode", await time(2000, (i) => store.appendNode({ sessionId: live, runId, kind: "assistantMessage", payload: { i } })));
    record("appendCall", await time(500, (i) => store.appendNode({ sessionId: live, runId, kind: "toolCall", callId: id("X", i), payload: {} })));
    record("addUsage", await time(1000, () => store.addUsage(runId, { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 1 })));
    record("recordTx", await time(2000, (i) => store.recordTx({ txId: id("Y", i), sessionId: live, callId: null, attribution: "none", at: i })));
    record("sessionOfTx", await time(1000, (i) => store.sessionOfTx([id("T", (i * 4 + 1) % nodes), id("Y", i), "unknown"])));
    record("txsOfCall", await time(1000, (i) => store.txsOfCall(id("C", (i * 4) % nodes))));
    record("state", await time(2000, (i) => store.state(id("S", i % sessions))));
    record("ledger", await time(2000, (i) => store.ledger().add(live, "tokens", { used: i })));
    record("listPage", await time(500, () => store.list({ projectId: "P1", limit: 50 })));
    record("transition", await time(500, (i) => store.apply(live, i % 2 === 0 ? { kind: "runEnded", runId, stop: "end" } : { kind: "resumed" })));
    return { sessions, nodes, p99Ms, p50Ms };
  } finally {
    await control.close();
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const given = process.argv[2];
  const dir = given ?? mkdtempSync(join(tmpdir(), "agent-budget-"));
  const sessions = Number(process.argv[3] ?? 10_000);
  const nodes = Number(process.argv[4] ?? 1_000_000);
  try {
    console.log(JSON.stringify(await measureAgentStoreBudget(dir, sessions, nodes), null, 2));
  } finally {
    if (given === undefined) rmSync(dir, { recursive: true, force: true });
  }
}
