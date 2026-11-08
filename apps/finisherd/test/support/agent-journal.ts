// Helpers for the agent journal tests (plan 7A Task 10): the two D34 stores of a temp server
// data (the real fin-node `Journal`s), their files read back raw, and sessions to record.
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import type { Session, Usage } from "@finisher/contracts";
import { Journal } from "@finisher/fin-node";
import { manualClock, systemIds } from "../../src/agent/clock.ts";
import { AgentJournal, type JournalStore } from "../../src/agent/journal.ts";

/** Project ids as the store makes them (lower-case ULIDs). */
export const P1 = "01jb0000000000000000000001";
export const P2 = "01jb0000000000000000000002";

export type Entry = { kind: string; ctx?: Record<string, any>; payload: Record<string, any>; consent: { purposes: string[] } };

export interface Stores {
  readonly diag: Journal;
  readonly traces: Journal;
  close(): Promise<void>;
}

/** Both stores of `serverData` under `privacy` (the `settings.json` section), P1 and P2 known. */
export function openStores(serverData: string, privacy: object | null, traceTest: { maxTotalBytes?: number; maxFileBytes?: number } = {}): Stores {
  const arg = privacy === null ? null : JSON.stringify(privacy);
  const diag = Journal.open(serverData, arg, "test-host");
  const traces = Journal.openTraces(serverData, arg, "test-host", traceTest);
  for (const p of [P1, P2]) diag.setProjectPolicy(p, "default");
  return {
    diag,
    traces,
    close: async () => {
      await traces.close();
      await diag.close();
    },
  };
}

/** Every entry of `<serverData>/<sub>` as it is on disk (flush first). */
export function entries(serverData: string, sub: "events" | "agent-traces"): Entry[] {
  const d = join(serverData, sub);
  if (!existsSync(d)) return [];
  return readdirSync(d)
    .filter((n) => n.startsWith("j-") && n.endsWith(".jsonl"))
    .sort()
    .flatMap((n) =>
      readFileSync(join(d, n), "utf8")
        .split("\n")
        .filter((l) => l.length > 0)
        .map((l) => JSON.parse(l) as Entry),
    );
}

export const agentEntries = (serverData: string, sub: "events" | "agent-traces") => entries(serverData, sub).filter((e) => e.kind.startsWith("agent."));

/** Flushes both stores (`status` reads after a flush). */
export async function flush(s: Stores): Promise<void> {
  await s.diag.status();
  await s.traces.status();
}

export function blobFiles(serverData: string, sub: "events" | "agent-traces"): string[] {
  const d = join(serverData, sub, "blobs");
  return existsSync(d) ? readdirSync(d).sort() : [];
}

/** A content field's value: inline, or its blob read back from the trace store. */
export function content(serverData: string, body: { inline?: unknown; blob?: { hash: string } }): unknown {
  if ("inline" in body) return body.inline;
  const hash = body.blob!.hash;
  const name = hash.includes(":") ? hash.slice(hash.indexOf(":") + 1) : hash;
  return JSON.parse(readFileSync(join(serverData, "agent-traces", "blobs", name), "utf8"));
}

export const CONTENT_FIELDS = ["messagesDelta", "output", "system", "tools", "args", "subject"];

/** A session as the store holds one (only the fields the journal reads matter). */
export function session(over: Partial<Session> = {}): Session {
  return {
    id: "01JS0000000000000000000001",
    projectId: P1,
    harness: "builtin",
    provider: "openrouter",
    model: "anthropic/claude-sonnet-5.5",
    weights: "closed",
    role: "editor",
    mode: "draft",
    rootBranch: "br1",
    base: { branch: "main", seq: 3 },
    status: "running",
    credentialId: over.id ?? "01JS0000000000000000000001",
    budget: {},
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 0 },
    createdBy: { principal: { kind: "local", id: "local" } },
    createdAt: 1_794_571_200_000,
    ...over,
  } as Session;
}

export const usage = (over: Partial<Usage> = {}): Usage => ({ input: 100, output: 20, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 50, costBasis: "reported", ...over });

/** A store with nothing in it (the direct-call tests). */
export const EMPTY_STORE: JournalStore = {
  get: () => undefined,
  txsOfCall: () => [],
  txsOfSession: () => [],
  approvals: () => [],
  nodes: () => [],
};

/** An `AgentJournal` over `s`, with a manual clock and real ULIDs; `logs` collects its refusals. */
export function agentJournal(s: Stores, store: JournalStore = EMPTY_STORE) {
  const clock = manualClock();
  const logs: string[] = [];
  const j = new AgentJournal({ diagnostics: s.diag, traces: s.traces, store, ids: systemIds(clock), clock, log: (m, f) => void logs.push(`${m} ${JSON.stringify(f ?? {})}`) });
  return { j, clock, logs };
}
