// What a rollback keeps in the agent store (plan 7B Decision 23, agent migration 0008): the
// nodes of the runs it took back are marked `rolledBack`, never removed (display and audit), and
// the built-in loop leaves them out of its next request. Runs stay as they were: their rows,
// seqs and statuses; the next run takes the next seq after all of them.
import type { StatementSync } from "node:sqlite";
import type { ControlStore } from "../control/db.ts";

export class SessionRollbacks {
  readonly #s: Record<string, StatementSync>;

  constructor(cs: ControlStore) {
    const sql: Record<string, string> = {
      mark:
        "UPDATE nodes SET rolledBack = 1 WHERE sessionId = ? AND rolledBack = 0 " +
        "AND runId IN (SELECT id FROM runs WHERE sessionId = ? AND seq > ?)",
      runs: "SELECT DISTINCT runId FROM nodes WHERE sessionId = ? AND rolledBack = 1",
    };
    this.#s = Object.fromEntries(Object.entries(sql).map(([k, v]) => [k, cs.db.prepare(v)]));
  }

  /** Marks the nodes of the session's runs after seq `toRun`; how many it marked. */
  mark(sessionId: string, toRun: number): number {
    return Number(this.#s.mark!.run(sessionId, sessionId, toRun).changes);
  }

  /** The runs (ids) a rollback took back. */
  runs(sessionId: string): Set<string> {
    return new Set((this.#s.runs!.all(sessionId) as Array<{ runId: string }>).map((r) => r.runId));
  }
}
