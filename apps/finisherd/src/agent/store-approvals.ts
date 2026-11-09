// The open-approval reads `approvals.ts` needs beside `AgentStore`'s (sub-project 7B Task 4,
// Decision 9), over the same `approvals` table: the open card an identical ask reuses and the
// counts the quotas refuse past. Run inside the caller's `ControlStore.tx`, so a check and the
// insert after it see one state.
//
// "Open" here is `status = 'open'` and not yet due (`expiresAt > now`): an approval past its
// deadline that the hourly sweep has not reached yet can no longer be answered (an answer
// expires it instead), so it is neither reused nor counted. Only the kinds an agent's request
// opens (`rule`, `provider`) are counted: a `budget` card is the server's own, at most one per
// interrupted session, and must not use up the agents' quota.
import type { DatabaseSync, StatementSync } from "node:sqlite";

/** The approval kinds an agent's request opens, the ones the quotas count. */
export const ASKED_KINDS = ["rule", "provider"] as const;

const ASKED = `kind IN (${ASKED_KINDS.map((k) => `'${k}'`).join(", ")})`;

export class OpenApprovals {
  readonly #match: StatementSync;
  readonly #ofSession: StatementSync;
  readonly #all: StatementSync;

  constructor(db: DatabaseSync) {
    this.#match = db.prepare(
      "SELECT id FROM approvals WHERE sessionId = ? AND kind = ? AND argsHash = ? AND json_extract(subject, '$.command') = ? " +
        "AND status = 'open' AND expiresAt > ? ORDER BY requestedAt, id LIMIT 1",
    );
    this.#ofSession = db.prepare(`SELECT COUNT(*) AS n FROM approvals WHERE sessionId = ? AND ${ASKED} AND status = 'open' AND expiresAt > ?`);
    this.#all = db.prepare(`SELECT COUNT(*) AS n FROM approvals WHERE ${ASKED} AND status = 'open' AND expiresAt > ?`);
  }

  /** The open approval of the same `(sessionId, kind, command, argsHash)`, if any. */
  matching(sessionId: string, kind: string, command: string, argsHash: string, now: number): string | undefined {
    return (this.#match.get(sessionId, kind, argsHash, command, now) as { id: string } | undefined)?.id;
  }

  /** The session's open asked approvals. */
  ofSession(sessionId: string, now: number): number {
    return (this.#ofSession.get(sessionId, now) as { n: number }).n;
  }

  /** The server's open asked approvals. */
  all(now: number): number {
    return (this.#all.get(now) as { n: number }).n;
  }
}

/**
 * One-time grants (sub-project 7B Decision 10; spec §7.3): an `allow` answer of a `rule` card
 * not yet consumed and not past its `expiresAt` (a grant expires with its approval), keyed by
 * exactly `(sessionId, command, argsHash)`. Consuming one records the call's idempotency key in
 * the same statement, so a restore puts back exactly that call's grant, never another's.
 */
export class Grants {
  readonly #consume: StatementSync;
  readonly #restore: StatementSync;

  constructor(db: DatabaseSync) {
    // The oldest grant first; one statement, so two calls can never take the same row.
    this.#consume = db.prepare(
      "UPDATE approvals SET consumedAt = ?, grantIdem = ? WHERE id = (SELECT id FROM approvals WHERE sessionId = ? AND kind = 'rule' " +
        "AND status = 'answered' AND decision = 'allow' AND consumedAt IS NULL AND argsHash = ? AND json_extract(subject, '$.command') = ? " +
        "AND expiresAt > ? ORDER BY answeredAt, id LIMIT 1) RETURNING id",
    );
    // The latest one the call consumed (an earlier one under the same key, lost to a crash, stays lost).
    this.#restore = db.prepare(
      "UPDATE approvals SET consumedAt = NULL, grantIdem = NULL WHERE id = (SELECT id FROM approvals WHERE sessionId = ? AND kind = 'rule' " +
        "AND grantIdem = ? AND (? IS NULL OR id = ?) ORDER BY consumedAt DESC, id DESC LIMIT 1) RETURNING id",
    );
  }

  /** Consumes the oldest matching grant for the call `idem`; its approval id, or undefined. */
  consume(sessionId: string, command: string, argsHash: string, idem: string, now: number): string | undefined {
    return (this.#consume.get(now, idem, sessionId, argsHash, command, now) as { id: string } | undefined)?.id;
  }

  /** Puts back the grant the call `idem` consumed last (or exactly `approvalId`); whether one was. */
  restore(sessionId: string, idem: string, approvalId?: string): boolean {
    return this.#restore.get(sessionId, idem, approvalId ?? null, approvalId ?? null) !== undefined;
  }
}
