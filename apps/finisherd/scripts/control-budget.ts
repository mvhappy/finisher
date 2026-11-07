// The control store's event-loop budget (plan 5A Task 1, spec §3.1): with 100k audit rows,
// every prepared statement the server runs per request stays at p99 ≤ 1 ms on the Linux VM.
// Usage: node apps/finisherd/scripts/control-budget.ts [<dir>]   (prints JSON; default a temp dir)
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { Audit } from "../src/control/audit.ts";
import { openControlStore } from "../src/control/db.ts";

export interface BudgetResult {
  readonly rows: number;
  readonly p99Ms: Record<string, number>;
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
    return {
      rows,
      p99Ms: { append: p99(append), authFail: p99(fail), list: p99(page), listByKind: p99(byKind), listSince: p99(since), meta: p99(meta) },
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
