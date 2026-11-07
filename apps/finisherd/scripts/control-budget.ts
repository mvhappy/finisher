// The control store's event-loop budget (plan 5A Task 1, spec §3.1): with 100k audit rows,
// every prepared statement the server runs per request stays at p99 ≤ 1 ms on the Linux VM.
// Usage: node apps/finisherd/scripts/control-budget.ts [<dir>]   (prints JSON; default a temp dir)
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { Effect } from "effect";
import { Audit } from "../src/control/audit.ts";
import { openControlStore } from "../src/control/db.ts";

export interface BudgetResult {
  readonly rows: number;
  /** Per prepared statement (the spec's budget: p99 ≤ 1 ms). */
  readonly p99Ms: Record<string, number>;
  /** One whole retention sweep, for information. */
  readonly sweepMs: number;
}

function p99(samples: number[]): number {
  const sorted = [...samples].sort((a, b) => a - b);
  return Number((sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.99))] ?? 0).toFixed(3));
}

function time(n: number, f: (i: number) => void): number[] {
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = process.hrtime.bigint();
    f(i);
    out.push(Number(process.hrtime.bigint() - t) / 1e6);
  }
  return out;
}

/** Fills `dir`'s control store with `rows` audit rows and times the per-request statements. */
export async function measureControlBudget(dir: string, rows = 100_000): Promise<BudgetResult> {
  const store = openControlStore(dir, { serverId: "budget", checkpoint: false });
  try {
    const kinds = ["auth.denied", "auth.ticket", "credential.mint", "consent.change"];
    store.tx(() => {
      const ins = store.db.prepare(
        "INSERT INTO audit (at, kind, principalKind, principalId, outcome, count, details) VALUES (?, ?, 'local', 'local', 'denied', 1, '{}')",
      );
      const start = Date.now() - rows;
      for (let i = 0; i < rows; i++) ins.run(start + i, kinds[i % kinds.length]!);
    });
    let now = Date.now();
    const audit = new Audit(store, () => now);
    const denied = { kind: "auth.denied", principal: { kind: "agent", id: "a" }, outcome: "denied", code: "SCOPE_DENIED" } as const;
    // Warm the statement caches.
    audit.append(denied);
    audit.list({ limit: 50 });
    const append = time(2000, () => audit.append(denied));
    const fail = time(2000, (i) => {
      now += i % 100 === 0 ? 60_000 : 1; // a new minute now and then: insert, then aggregate
      audit.append({ kind: "auth.fail", principal: { kind: "anonymous", id: "-" }, credentialKind: "local", outcome: "failed", code: "UNAUTHENTICATED" });
    });
    const page = time(500, () => audit.list({ limit: 50 }));
    const byKind = time(500, () => audit.list({ kind: "credential.mint", limit: 50 }));
    const since = time(500, () => audit.list({ since: Date.now() - 1000, limit: 50 }));
    const envId = store.db.prepare("SELECT value FROM meta WHERE key = 'envId'");
    const meta = time(2000, () => envId.get());
    // The hourly sweep over 100k+ rows: its statements (the budget is per statement), and
    // one whole sweep (three statements and the Effect around them) for information.
    const sweeper = new Audit(store, () => now, { maxRows: rows });
    await Effect.runPromise(sweeper.sweep()); // trims the ~4k rows appended above, in batches
    const agePurge = store.db.prepare("DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE at < ? ORDER BY at LIMIT ?)");
    const maxId = store.db.prepare("SELECT MAX(id) AS id FROM audit");
    const rowPurge = store.db.prepare("DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE id <= ? ORDER BY id LIMIT ?)");
    const sweepAge = time(200, () => agePurge.run(now - 180 * 86_400_000, 500));
    const sweepMax = time(200, () => maxId.get());
    const sweepRows = time(200, () => {
      audit.append(denied);
      rowPurge.run(((maxId.get() as { id: number }).id ?? 0) - rows, 500);
    });
    const sweeps: number[] = [];
    for (let i = 0; i < 20; i++) {
      for (let j = 0; j < 10; j++) audit.append(denied);
      const t = process.hrtime.bigint();
      await Effect.runPromise(sweeper.sweep());
      sweeps.push(Number(process.hrtime.bigint() - t) / 1e6);
    }
    return {
      rows,
      p99Ms: { append: p99(append), authFail: p99(fail), list: p99(page), listByKind: p99(byKind), listSince: p99(since), meta: p99(meta), sweepAge: p99(sweepAge), sweepMax: p99(sweepMax), sweepRows: p99(sweepRows) },
      sweepMs: p99(sweeps),
    };
  } finally {
    await store.close();
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const given = process.argv[2];
  const dir = given ?? mkdtempSync(join(tmpdir(), "control-budget-"));
  measureControlBudget(dir)
    .then((r) => process.stdout.write(`${JSON.stringify(r)}\n`))
    .finally(() => {
      if (given === undefined) rmSync(dir, { recursive: true, force: true });
    });
}
