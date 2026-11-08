// Reads the agent RPCs need beside `AgentStore`'s (plan 7A Task 12), over the same `control.db`
// tables: `agent.watch`'s `fromNode` (a node id) as a seq and `WATCH_LAGGING`'s `lastSeq` as a
// node id, and `agent.approval.list` across sessions (newest first, the caller's projects only).
import type { DatabaseSync, StatementSync } from "node:sqlite";
import type { ApprovalStatus } from "@finisher/contracts";

export interface ApprovalQuery {
  readonly sessionId?: string;
  readonly projectId?: string;
  /** The caller's reach; undefined: every project, empty: none. */
  readonly projects?: ReadonlyArray<string>;
  readonly status?: ApprovalStatus;
  readonly limit?: number;
}

export class AgentLookup {
  readonly #db: DatabaseSync;
  readonly #seq: StatementSync;
  readonly #id: StatementSync;
  readonly #lists = new Map<string, StatementSync>();

  constructor(db: DatabaseSync) {
    this.#db = db;
    this.#seq = db.prepare("SELECT seq FROM nodes WHERE sessionId = ? AND id = ?");
    this.#id = db.prepare("SELECT id FROM nodes WHERE sessionId = ? AND seq = ?");
  }

  /** The seq of node `nodeId` of the session, or undefined. */
  nodeSeq(sessionId: string, nodeId: string): number | undefined {
    const r = this.#seq.get(sessionId, nodeId) as { seq: number } | undefined;
    return r?.seq;
  }

  /** The id of the session's node at `seq`, or undefined. */
  nodeIdAt(sessionId: string, seq: number): string | undefined {
    const r = this.#id.get(sessionId, seq) as { id: string } | undefined;
    return r?.id;
  }

  /** Approval ids, newest first, at most `limit` (200). */
  approvalIds(q: ApprovalQuery): string[] {
    if (q.projects !== undefined && q.projects.length === 0) return [];
    const where: string[] = [];
    const args: (string | number)[] = [];
    if (q.sessionId !== undefined) (where.push("a.sessionId = ?"), args.push(q.sessionId));
    if (q.projectId !== undefined) (where.push("s.projectId = ?"), args.push(q.projectId));
    if (q.status !== undefined) (where.push("a.status = ?"), args.push(q.status));
    if (q.projects !== undefined) {
      where.push(`s.projectId IN (${q.projects.map(() => "?").join(", ")})`);
      args.push(...q.projects);
    }
    const sql = `SELECT a.id FROM approvals a JOIN sessions s ON s.id = a.sessionId${where.length ? ` WHERE ${where.join(" AND ")}` : ""} ORDER BY a.requestedAt DESC, a.id DESC LIMIT ?`;
    let stmt = this.#lists.get(sql);
    if (stmt === undefined) this.#lists.set(sql, (stmt = this.#db.prepare(sql)));
    return (stmt.all(...args, Math.min(Math.max(q.limit ?? 200, 1), 200)) as { id: string }[]).map((r) => r.id);
  }
}
