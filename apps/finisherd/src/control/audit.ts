// The always-on local security audit log (plan 5A Decision 4, spec §3.3): rows in
// `control.db` `audit`. Consent does not gate it and nothing uploads it. It never holds
// secrets, tokens, MACs, nonces, links, paths or request payloads: each kind's `details` is a
// closed schema (`AUDIT_DETAILS` in contracts) of ids, words and counts; details that do not
// fit are dropped (the row is kept) and counted as a failure.
//
// - `append` never throws: a failed write is logged to stderr and counted (`failures()`,
//   reported by `server.status`); the request it records still gets its answer.
// - `auth.fail` rows aggregate per `(credentialKind, remote)` and `auth.denied` rows per
//   `(principal, code, rpc)`, per minute, into one row with a `count`, so a flood of
//   connections or refused requests cannot flood the log (review of Task 5, M5).
// - Retention: 180 days or 100,000 rows, swept at start and hourly (`layerAudit`), in batches
//   of ≤ 500 rows per `DELETE`, one batch per event-loop turn.
import type { StatementSync } from "node:sqlite";
import { type AuditEntry, type AuditKind, type AuditOutcome, auditDetailsValid } from "@finisher/contracts";
import { Context, Duration, Effect, Layer, Schedule } from "effect";
import { type ControlStore, ControlStoreTag } from "./db.ts";

export type { AuditEntry, AuditKind } from "@finisher/contracts";

export interface AuditInput {
  kind: AuditKind;
  principal: { kind: string; id: string };
  credentialKind?: string;
  outcome: AuditOutcome;
  code?: string;
  details?: Record<string, unknown>;
}

export interface AuditListQuery {
  /** Entries at or after this time (ms). */
  since?: number;
  kind?: AuditKind;
  /** The `next` of the previous page. */
  cursor?: string;
  /** Default 100, at most 1000. */
  limit?: number;
}

export interface AuditOptions {
  /** Default 180 days. */
  readonly maxAgeMs?: number;
  /** Default 100,000. */
  readonly maxRows?: number;
}

const DAY_MS = 86_400_000;
const MINUTE_MS = 60_000;
export const AUDIT_MAX_AGE_MS = 180 * DAY_MS;
export const AUDIT_MAX_ROWS = 100_000;
const MAX_LIMIT = 1000;

interface Row {
  id: number;
  at: number;
  kind: string;
  principalKind: string;
  principalId: string;
  credentialKind: string | null;
  outcome: AuditOutcome;
  code: string | null;
  remote: string | null;
  count: number;
  details: string;
}

function toEntry(r: Row): AuditEntry {
  let details: Record<string, unknown> = {};
  try {
    details = JSON.parse(r.details) as Record<string, unknown>;
  } catch {
    details = {};
  }
  return {
    id: r.id,
    at: r.at,
    kind: r.kind,
    principalKind: r.principalKind,
    principalId: r.principalId,
    ...(r.credentialKind === null ? {} : { credentialKind: r.credentialKind }),
    outcome: r.outcome,
    ...(r.code === null ? {} : { code: r.code }),
    ...(r.remote === null ? {} : { remote: r.remote }),
    count: r.count,
    details,
  };
}

const COLUMNS = "id, at, kind, principalKind, principalId, credentialKind, outcome, code, remote, count, details";

export class Audit {
  readonly #store: ControlStore;
  readonly #clock: () => number;
  readonly #maxAgeMs: number;
  readonly #maxRows: number;
  #failures = 0;
  #insert: StatementSync | undefined;
  #bump: StatementSync | undefined;
  readonly #lists = new Map<string, StatementSync>();
  /** The current minute's aggregated rows (`failKey`) → row id. */
  #failMinute = -1;
  readonly #failRows = new Map<string, number>();

  constructor(store: ControlStore, clock: () => number = Date.now, opts: AuditOptions = {}) {
    this.#store = store;
    this.#clock = clock;
    this.#maxAgeMs = opts.maxAgeMs ?? AUDIT_MAX_AGE_MS;
    this.#maxRows = opts.maxRows ?? AUDIT_MAX_ROWS;
  }

  /** Records `e`; never throws (failures are logged and counted). `remote` stays unset in 5A. */
  append(e: AuditInput): void {
    try {
      const at = this.#clock();
      if (aggregated(e) && this.#aggregate(e, at)) return;
      let details = e.details ?? {};
      if (!auditDetailsValid(e.kind, details)) {
        // A caller bug: the event is still recorded, without the details (never printed).
        this.#failures++;
        process.stderr.write(`finisherd: warning: audit details refused for ${e.kind}\n`);
        details = {};
      }
      this.#insert ??= this.#store.db.prepare(
        "INSERT INTO audit (at, kind, principalKind, principalId, credentialKind, outcome, code, remote, count, details) " +
          "VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1, ?)",
      );
      const r = this.#insert.run(
        at,
        e.kind,
        e.principal.kind,
        e.principal.id,
        e.credentialKind ?? null,
        e.outcome,
        e.code ?? null,
        JSON.stringify(details),
      );
      if (aggregated(e)) this.#failRows.set(failKey(e), Number(r.lastInsertRowid));
    } catch (err) {
      this.#failures++;
      process.stderr.write(`finisherd: warning: audit write failed (${e.kind}): ${String((err as Error)?.message ?? err)}\n`);
    }
  }

  /** Adds one to this minute's row for the same credential kind and remote, if there is one. */
  #aggregate(e: AuditInput, at: number): boolean {
    const minute = Math.floor(at / MINUTE_MS);
    if (minute !== this.#failMinute) {
      this.#failMinute = minute;
      this.#failRows.clear();
      return false;
    }
    const id = this.#failRows.get(failKey(e));
    if (id === undefined) return false;
    this.#bump ??= this.#store.db.prepare("UPDATE audit SET count = count + 1 WHERE id = ?");
    // Purged meanwhile: write a new row.
    return Number(this.#bump.run(id).changes) === 1;
  }

  /** Newest first; `next` continues the listing. */
  list(q: AuditListQuery = {}): { entries: AuditEntry[]; next?: string } {
    const limit = Math.max(1, Math.min(MAX_LIMIT, Math.floor(q.limit ?? 100)));
    const where: string[] = [];
    const params: Array<string | number> = [];
    if (q.cursor !== undefined) {
      const before = Number(q.cursor);
      if (!Number.isSafeInteger(before)) return { entries: [] };
      where.push("id < ?");
      params.push(before);
    }
    if (q.since !== undefined) {
      where.push("at >= ?");
      params.push(q.since);
    }
    if (q.kind !== undefined) {
      where.push("kind = ?");
      params.push(q.kind);
    }
    const sql = `SELECT ${COLUMNS} FROM audit${where.length ? ` WHERE ${where.join(" AND ")}` : ""} ORDER BY id DESC LIMIT ?`;
    let stmt = this.#lists.get(sql);
    if (stmt === undefined) {
      stmt = this.#store.db.prepare(sql);
      this.#lists.set(sql, stmt);
    }
    const rows = stmt.all(...params, limit + 1) as unknown as Row[];
    const entries = rows.slice(0, limit).map(toEntry);
    const last = entries[entries.length - 1];
    return rows.length > limit && last !== undefined ? { entries, next: String(last.id) } : { entries };
  }

  /** Deletes entries before `before` (ms), or all of them; succeeds with the count. */
  purge(q: { before?: number; all?: boolean }): Effect.Effect<number> {
    this.#failRows.clear();
    if (q.all === true) {
      return this.#store.deleteInBatches("DELETE FROM audit WHERE id IN (SELECT id FROM audit ORDER BY id LIMIT ?)", []);
    }
    if (q.before === undefined) return Effect.succeed(0);
    return this.#store.deleteInBatches(
      "DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE at < ? ORDER BY at LIMIT ?)",
      [q.before],
    );
  }

  /**
   * The owner's purge (`auth.audit.purge`; review of 5A Tasks 8–9, Task 8): deletes entries
   * before `before` (ms), or all of them, **but never an `audit.purge` row**, and records this
   * purge (`audit.purge {reason, before?, count, transport}`) in the same transaction, so the
   * log can never lose the fact that, when, by whom and how far back it was purged. One
   * `DELETE` (≤ 100,000 rows: a fraction of a second on the main thread, for a rare owner
   * command). Throws `CONTROL_DB_BUSY` and changes nothing when the store is busy.
   */
  ownerPurge(q: { before: number } | { all: true }, by: { readonly kind: string; readonly id: string }, transport: string): number {
    this.#failRows.clear();
    return this.#store.tx(() => {
      const removed = Number(
        ("all" in q
          ? this.#store.db.prepare("DELETE FROM audit WHERE kind != 'audit.purge'").run()
          : this.#store.db.prepare("DELETE FROM audit WHERE kind != 'audit.purge' AND at < ?").run(q.before)
        ).changes,
      );
      const details = { reason: "all" in q ? "all" : "before", ...("all" in q ? {} : { before: q.before }), count: removed, transport };
      if (!auditDetailsValid("audit.purge", details)) throw new Error("audit.purge details do not fit their schema");
      this.#store.db
        .prepare(
          "INSERT INTO audit (at, kind, principalKind, principalId, credentialKind, outcome, code, remote, count, details) " +
            "VALUES (?, 'audit.purge', ?, ?, NULL, 'ok', NULL, NULL, 1, ?)",
        )
        .run(this.#clock(), by.kind, by.id, JSON.stringify(details));
      return removed;
    });
  }

  /** Applies retention: older than the age limit, then the oldest rows past the row limit. */
  sweep(): Effect.Effect<void> {
    // Ids only grow, so everything at or below `max(id) - maxRows` is past the limit (with
    // gaps from purges this keeps fewer than `maxRows`, never more). An index seek, no OFFSET.
    const rowLimit = Effect.suspend(() => {
      const top = this.#store.db.prepare("SELECT MAX(id) AS id FROM audit").get() as { id: number | null };
      const cut = (top.id ?? 0) - this.#maxRows;
      return cut <= 0
        ? Effect.succeed(0)
        : this.#store.deleteInBatches("DELETE FROM audit WHERE id IN (SELECT id FROM audit WHERE id <= ? ORDER BY id LIMIT ?)", [cut]);
    });
    return Effect.suspend(() => this.purge({ before: this.#clock() - this.#maxAgeMs })).pipe(
      Effect.andThen(rowLimit),
      Effect.asVoid,
      Effect.catchDefect((e) =>
        Effect.sync(() => {
          this.#failures++;
          process.stderr.write(`finisherd: warning: audit sweep failed: ${String(e)}\n`);
        }),
      ),
    );
  }

  /** Writes that failed since start. */
  failures(): number {
    return this.#failures;
  }
}

/**
 * Rows counted per minute rather than written one by one: failed proofs per credential kind,
 * and denials per principal, code and RPC, so no caller can flush the log by being refused
 * (review of Task 5, M5).
 */
function aggregated(e: AuditInput): boolean {
  return e.kind === "auth.fail" || e.kind === "auth.denied";
}

function failKey(e: AuditInput): string {
  if (e.kind === "auth.denied") {
    const rpc = (e.details as { rpc?: unknown } | undefined)?.rpc;
    return `${e.kind}|${e.principal.kind}:${e.principal.id}|${e.code ?? ""}|${typeof rpc === "string" ? rpc : ""}`;
  }
  return `${e.kind}|${e.credentialKind ?? ""}|`;
}

export class AuditTag extends Context.Service<AuditTag, Audit>()("finisherd/Audit") {}

/** The audit log over the store, swept at start and hourly for the enclosing scope. */
export const layerAudit: Layer.Layer<AuditTag, never, ControlStoreTag> = Layer.effect(
  AuditTag,
  Effect.gen(function* () {
    const store = yield* ControlStoreTag;
    const audit = new Audit(store);
    yield* Effect.forkScoped(Effect.repeat(audit.sweep(), Schedule.spaced(Duration.hours(1))));
    return audit;
  }),
);
