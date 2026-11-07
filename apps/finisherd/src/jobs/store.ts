// The job store (5C-1 plan Decisions 25–27, 30): the only module that touches the `jobs`
// chain's tables. Importing it registers the chain (`main.ts` imports it before it opens the
// control store). Every statement is prepared once; a transition is one `UPDATE`; bulk
// deletes go through `deleteInBatches`.
import type { StatementSync } from "node:sqlite";
import {
  type FinErrorData,
  FinError,
  type JobItemError,
  type JobOwner,
  type JobState,
  type JobView,
  type JobVia,
  type Priority,
  TERMINAL_JOB_STATES,
} from "@finisher/contracts";
import { Effect } from "effect";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { jobs0001 } from "../control/migrations/jobs/0001.ts";
import { ulid } from "../control/ulid.ts";
import type { ItemOutcome } from "./kinds.ts";

registerChain("jobs", [jobs0001], { formatVersion: 1, minReaderVersion: 1 });

const DAY_MS = 86_400_000;
export const RETENTION_MAX_AGE_MS = 7 * DAY_MS;
export const RETENTION_MAX_ROWS = 10_000;
const MAX_LIMIT = 500;

/** A new job (state `queued`). */
export interface NewJobRow {
  readonly id?: string;
  readonly kind: string;
  readonly kindVersion: number;
  readonly family: string;
  readonly params: unknown;
  readonly owner: JobOwner;
  readonly via?: JobVia;
  readonly rights?: unknown;
  readonly project?: string;
  readonly library?: string;
  readonly parent?: string;
  readonly priority: Priority;
  readonly warm?: boolean;
  readonly maxAttempts: number;
  readonly idemKey?: string;
  readonly dedupeKey?: string;
  readonly createdAt?: number;
}

/** What a transition may change besides the state. `null` clears a column. */
export interface TransitionPatch {
  readonly attempts?: number;
  readonly priority?: Priority;
  readonly warm?: boolean;
  readonly nextRunAt?: number | null;
  readonly error?: FinErrorData | null;
  readonly result?: unknown;
  readonly checkpoint?: unknown;
  readonly extra?: unknown;
  readonly isolation?: unknown;
  readonly pausedFrom?: JobState | null;
  readonly startedAt?: number;
}

/** A job's full row: the view plus the engine's own columns. */
export type JobRow = JobView & {
  readonly phase: "run" | "hooks";
  readonly checkpoint?: unknown;
  readonly extra?: unknown;
  readonly isolation?: unknown;
  readonly pausedFrom?: JobState;
};

export interface JobQuery {
  readonly owner?: JobOwner;
  readonly state?: ReadonlyArray<JobState>;
  readonly kind?: string;
  readonly project?: string;
  readonly library?: string;
  readonly cursor?: string;
  /** Default 100, at most 500. */
  readonly limit?: number;
}

/** Legal transitions (anything else is a bug). `running → running` starts another run in place. */
const LEGAL: Readonly<Record<JobState, ReadonlyArray<JobState>>> = {
  queued: ["running", "paused", "cancelled", "failed"],
  running: ["running", "succeeded", "failed", "cancelled", "interrupted", "queued", "paused"],
  paused: ["queued", "cancelled"],
  interrupted: ["queued"],
  succeeded: [],
  failed: [],
  cancelled: [],
};

interface Row {
  id: string;
  kind: string;
  kindVersion: number;
  family: string;
  params: string;
  ownerKind: string;
  ownerId: string;
  via: string | null;
  rights: string | null;
  project: string | null;
  library: string | null;
  parent: string | null;
  priority: Priority;
  state: JobState;
  phase: "run" | "hooks";
  pausedFrom: JobState | null;
  warm: number;
  attempts: number;
  maxAttempts: number;
  idemKey: string | null;
  dedupeKey: string | null;
  checkpoint: string | null;
  extra: string | null;
  result: string | null;
  error: string | null;
  isolation: string | null;
  createdAt: number;
  startedAt: number | null;
  finishedAt: number | null;
  nextRunAt: number | null;
}

const json = (v: unknown): string | null => (v === undefined ? null : JSON.stringify(v));
const parse = (s: string | null): unknown => (s === null ? undefined : JSON.parse(s));

function errorData(e: { readonly code: string; readonly message: string; readonly details: unknown; readonly fix?: string | undefined }): FinErrorData {
  return e.fix === undefined ? { code: e.code, message: e.message, details: e.details } : { code: e.code, message: e.message, details: e.details, fix: e.fix };
}

/** Builds the view from a row, leaving out absent columns (`exactOptionalPropertyTypes`). */
function toRow(r: Row, itemErrors: ReadonlyArray<JobItemError>): JobRow {
  const v: Record<string, unknown> = {
    id: r.id,
    kind: r.kind,
    kindVersion: r.kindVersion,
    family: r.family,
    params: JSON.parse(r.params),
    owner: { kind: r.ownerKind, id: r.ownerId },
    priority: r.priority,
    state: r.state,
    phase: r.phase,
    attempts: r.attempts,
    maxAttempts: r.maxAttempts,
    warm: r.warm !== 0,
    createdAt: r.createdAt,
  };
  const opt = (key: string, value: unknown): void => {
    if (value !== null && value !== undefined) v[key] = value;
  };
  opt("via", parse(r.via));
  opt("rights", parse(r.rights));
  opt("project", r.project);
  opt("library", r.library);
  opt("parent", r.parent);
  opt("idemKey", r.idemKey);
  opt("dedupeKey", r.dedupeKey);
  opt("result", parse(r.result));
  opt("error", parse(r.error));
  opt("checkpoint", parse(r.checkpoint));
  opt("extra", parse(r.extra));
  opt("isolation", parse(r.isolation));
  opt("pausedFrom", r.pausedFrom);
  opt("startedAt", r.startedAt);
  opt("finishedAt", r.finishedAt);
  opt("nextRunAt", r.nextRunAt);
  if (itemErrors.length > 0) v.itemErrors = itemErrors;
  return v as JobRow;
}

/** The public view of a row (the engine's own columns removed). */
export function viewOf(row: JobRow): JobView {
  const { phase: _p, checkpoint: _c, extra: _e, isolation: _i, pausedFrom: _f, ...view } = row;
  return view;
}

const COLUMNS =
  "id, kind, kindVersion, family, params, ownerKind, ownerId, via, rights, project, library, parent, priority, state, phase, " +
  "pausedFrom, warm, attempts, maxAttempts, idemKey, dedupeKey, checkpoint, extra, result, error, isolation, createdAt, " +
  "startedAt, finishedAt, nextRunAt";

export class JobStore {
  readonly #store: ControlStore;
  readonly #clock: () => number;
  readonly #s: Record<string, StatementSync>;
  readonly #lists = new Map<string, StatementSync>();

  constructor(store: ControlStore, clock: () => number = Date.now) {
    this.#store = store;
    this.#clock = clock;
    const db = store.db;
    const p = (sql: string): StatementSync => db.prepare(sql);
    this.#s = {
      insert: p(
        `INSERT INTO jobs (id, kind, kindVersion, family, params, ownerKind, ownerId, via, rights, project, library, parent, ` +
          `priority, state, warm, maxAttempts, idemKey, dedupeKey, createdAt) ` +
          `VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?)`,
      ),
      get: p(`SELECT ${COLUMNS} FROM jobs WHERE id = ?`),
      itemErrors: p("SELECT item, error FROM job_items WHERE jobId = ? AND error IS NOT NULL ORDER BY item"),
      itemErrorsOf: p(
        "SELECT jobId, item, error FROM job_items WHERE error IS NOT NULL AND jobId IN (SELECT value FROM json_each(?)) ORDER BY jobId, item",
      ),
      byIdem: p(`SELECT ${COLUMNS} FROM jobs WHERE ownerKind = ? AND ownerId = ? AND idemKey = ?`),
      activeByDedupe: p(
        `SELECT ${COLUMNS} FROM jobs WHERE kind = ? AND dedupeKey = ? AND state IN ('queued', 'running', 'paused') ORDER BY createdAt DESC LIMIT 1`,
      ),
      poisonedByDedupe: p(
        `SELECT ${COLUMNS} FROM jobs WHERE kind = ? AND dedupeKey = ? AND state = 'failed' AND json_extract(error, '$.code') = 'POISON_ITEM' ` +
          "ORDER BY finishedAt DESC LIMIT 1",
      ),
      update: p(
        "UPDATE jobs SET state = ?, phase = ?, pausedFrom = ?, warm = ?, attempts = ?, priority = ?, checkpoint = ?, extra = ?, " +
          "result = ?, error = ?, isolation = ?, startedAt = ?, finishedAt = ?, nextRunAt = ? WHERE id = ? AND state = ? AND phase = ?",
      ),
      hooks: p("UPDATE jobs SET phase = 'hooks', result = ? WHERE id = ? AND state = 'running'"),
      checkpoint: p("UPDATE jobs SET checkpoint = ? WHERE id = ?"),
      items: p("SELECT item, value, error FROM job_items WHERE jobId = ? ORDER BY item"),
      writeItem: p("INSERT INTO job_items (jobId, item, value, error) VALUES (?, ?, ?, ?) ON CONFLICT (jobId, item) DO UPDATE SET value = excluded.value, error = excluded.error"),
      unfinished: p("SELECT COUNT(*) AS n FROM jobs WHERE finishedAt IS NULL"),
      inState: p(`SELECT ${COLUMNS} FROM jobs WHERE finishedAt IS NULL AND state = ? ORDER BY createdAt, id`),
      inHooks: p(`SELECT ${COLUMNS} FROM jobs WHERE finishedAt IS NULL AND state = 'running' AND phase = 'hooks' ORDER BY createdAt, id`),
      nthFinished: p("SELECT finishedAt FROM jobs WHERE finishedAt IS NOT NULL ORDER BY finishedAt DESC LIMIT 1 OFFSET ?"),
    };
  }

  #s1(name: string): StatementSync {
    const s = this.#s[name];
    if (s === undefined) throw new Error(`job store: no statement ${name}`);
    return s;
  }

  #one(r: Row | undefined): JobRow | undefined {
    if (r === undefined) return undefined;
    const errors = (this.#s1("itemErrors").all(r.id) as unknown as Array<{ item: string; error: string }>).map((e) => ({
      item: e.item,
      error: JSON.parse(e.error) as FinErrorData,
    }));
    return toRow(r, errors);
  }

  #many(rows: Row[]): JobRow[] {
    if (rows.length === 0) return [];
    const byJob = new Map<string, JobItemError[]>();
    const errs = this.#s1("itemErrorsOf").all(JSON.stringify(rows.map((r) => r.id))) as unknown as Array<{
      jobId: string;
      item: string;
      error: string;
    }>;
    for (const e of errs) {
      const list = byJob.get(e.jobId) ?? [];
      list.push({ item: e.item, error: JSON.parse(e.error) as FinErrorData });
      byJob.set(e.jobId, list);
    }
    return rows.map((r) => toRow(r, byJob.get(r.id) ?? []));
  }

  insert(row: NewJobRow): JobView {
    const id = row.id ?? ulid(this.#clock());
    const createdAt = row.createdAt ?? this.#clock();
    this.#s1("insert").run(
      id,
      row.kind,
      row.kindVersion,
      row.family,
      JSON.stringify(row.params ?? null),
      row.owner.kind,
      row.owner.id,
      json(row.via),
      json(row.rights),
      row.project ?? null,
      row.library ?? null,
      row.parent ?? null,
      row.priority,
      row.warm === true ? 1 : 0,
      row.maxAttempts,
      row.idemKey ?? null,
      row.dedupeKey ?? null,
      createdAt,
    );
    return this.get(id) ?? refuse(`job ${id} vanished after its insert`);
  }

  get(id: string): JobView | undefined {
    const row = this.row(id);
    return row === undefined ? undefined : viewOf(row);
  }

  /** The full row (phase, checkpoint, extra, isolation), for the engine. */
  row(id: string): JobRow | undefined {
    return this.#one(this.#s1("get").get(id) as Row | undefined);
  }

  byIdem(owner: JobOwner, idemKey: string): JobView | undefined {
    return this.#view(this.#s1("byIdem").get(owner.kind, owner.id, idemKey) as Row | undefined);
  }

  activeByDedupe(kind: string, key: string): JobView | undefined {
    return this.#view(this.#s1("activeByDedupe").get(kind, key) as Row | undefined);
  }

  /** A finished job of this dedupe key that failed `POISON_ITEM` (`rememberPoison`, Decision 27). */
  poisonedByDedupe(kind: string, key: string): JobView | undefined {
    return this.#view(this.#s1("poisonedByDedupe").get(kind, key) as Row | undefined);
  }

  #view(r: Row | undefined): JobView | undefined {
    const row = this.#one(r);
    return row === undefined ? undefined : viewOf(row);
  }

  /**
   * Moves a job to `to` in one `UPDATE` (Decision 26). Terminal states set `finishedAt`; the
   * others clear it; `running` sets `startedAt` once; `queued` resets the phase to `run`. An
   * illegal transition, or a row changed under it, is a bug and throws.
   */
  transition(id: string, to: JobState, patch: TransitionPatch = {}): JobView {
    const cur = this.row(id) ?? refuse(`transition of unknown job ${id}`);
    if (!LEGAL[cur.state].includes(to)) refuse(`illegal job transition ${cur.state} → ${to} (${id})`);
    const now = this.#clock();
    const pick = <K extends keyof TransitionPatch>(k: K, fallback: unknown): unknown => (k in patch ? patch[k] : fallback);
    const phase = to === "queued" ? "run" : cur.phase;
    const finishedAt = TERMINAL_JOB_STATES.includes(to) ? now : null;
    const startedAt = to === "running" ? (cur.startedAt ?? patch.startedAt ?? now) : (cur.startedAt ?? null);
    const error = pick("error", cur.error) as FinErrorData | null | undefined;
    const changed = this.#s1("update").run(
      to,
      phase,
      (pick("pausedFrom", cur.pausedFrom) as JobState | null | undefined) ?? null,
      (pick("warm", cur.warm) as boolean) ? 1 : 0,
      pick("attempts", cur.attempts) as number,
      pick("priority", cur.priority) as Priority,
      json(pick("checkpoint", cur.checkpoint) ?? undefined),
      json(pick("extra", cur.extra) ?? undefined),
      json(pick("result", cur.result) ?? undefined),
      error === null || error === undefined ? null : JSON.stringify(errorData(error)),
      json(pick("isolation", cur.isolation) ?? undefined),
      startedAt,
      finishedAt,
      (pick("nextRunAt", cur.nextRunAt) as number | null | undefined) ?? null,
      id,
      cur.state,
      cur.phase,
    ).changes;
    if (Number(changed) !== 1) refuse(`job ${id} changed under its transition`);
    return this.get(id) ?? refuse(`job ${id} vanished`);
  }

  /** The worker succeeded: its result and `phase: hooks` in one `UPDATE`; the state stays `running` (Decision 40). */
  enterHooks(id: string, result: unknown): JobView {
    const changed = this.#s1("hooks").run(JSON.stringify(result ?? null), id).changes;
    if (Number(changed) !== 1) refuse(`enterHooks of job ${id}, which is not running`);
    return this.get(id) ?? refuse(`job ${id} vanished`);
  }

  /** Stores the newest checkpoint; the caller throttles (Decision 26). */
  checkpoint(id: string, data: unknown): void {
    this.#s1("checkpoint").run(JSON.stringify(data ?? null), id);
  }

  items(id: string): Map<string, ItemOutcome> {
    const rows = this.#s1("items").all(id) as unknown as Array<{ item: string; value: string | null; error: string | null }>;
    const out = new Map<string, ItemOutcome>();
    for (const r of rows) {
      if (r.error !== null) out.set(r.item, { error: new FinError(JSON.parse(r.error) as FinErrorData) });
      else out.set(r.item, r.value === null ? {} : { value: JSON.parse(r.value) });
    }
    return out;
  }

  /** Writes item outcomes in one transaction (the caller batches per second or 256 rows). */
  writeItems(id: string, items: ReadonlyArray<readonly [string, ItemOutcome]>): void {
    if (items.length === 0) return;
    const stmt = this.#s1("writeItem");
    this.#store.tx(() => {
      for (const [item, outcome] of items) {
        if ("error" in outcome) stmt.run(id, item, null, JSON.stringify(errorData(outcome.error)));
        else stmt.run(id, item, json(outcome.value), null);
      }
    });
  }

  /** Newest first; `next` continues the listing (an unreadable cursor reads as the end). */
  list(q: JobQuery = {}): { jobs: JobView[]; next?: string } {
    const limit = Math.max(1, Math.min(MAX_LIMIT, Math.floor(q.limit ?? 100)));
    const where: string[] = [];
    const params: Array<string | number> = [];
    if (q.cursor !== undefined) {
      const m = /^(\d{1,16})\.([0-9A-Za-z]{1,64})$/.exec(q.cursor);
      if (m === null) return { jobs: [] };
      where.push("(createdAt < ? OR (createdAt = ? AND id < ?))");
      params.push(Number(m[1]), Number(m[1]), m[2] as string);
    }
    if (q.owner !== undefined) {
      where.push("ownerKind = ? AND ownerId = ?");
      params.push(q.owner.kind, q.owner.id);
    }
    if (q.state !== undefined) {
      where.push("state IN (SELECT value FROM json_each(?))");
      params.push(JSON.stringify(q.state));
    }
    for (const key of ["kind", "project", "library"] as const) {
      const value = q[key];
      if (value !== undefined) {
        where.push(`${key} = ?`);
        params.push(value);
      }
    }
    const sql = `SELECT ${COLUMNS} FROM jobs${where.length ? ` WHERE ${where.join(" AND ")}` : ""} ORDER BY createdAt DESC, id DESC LIMIT ?`;
    let stmt = this.#lists.get(sql);
    if (stmt === undefined) {
      stmt = this.#store.db.prepare(sql);
      this.#lists.set(sql, stmt);
    }
    const rows = stmt.all(...params, limit + 1) as unknown as Row[];
    const jobs = this.#many(rows.slice(0, limit)).map(viewOf);
    const last = jobs[jobs.length - 1];
    return rows.length > limit && last !== undefined ? { jobs, next: `${last.createdAt}.${last.id}` } : { jobs };
  }

  unfinishedCount(): number {
    return Number((this.#s1("unfinished").get() as { n: number }).n);
  }

  running(): JobView[] {
    return this.#inState("running");
  }

  /** Running jobs whose worker succeeded and whose `onSucceeded` may not have finished. */
  inHooks(): JobView[] {
    return this.#many(this.#s1("inHooks").all() as unknown as Row[]).map(viewOf);
  }

  queued(): JobView[] {
    return this.#inState("queued");
  }

  paused(): JobView[] {
    return this.#inState("paused");
  }

  #inState(state: JobState): JobView[] {
    return this.#many(this.#s1("inState").all(state) as unknown as Row[]).map(viewOf);
  }

  /**
   * Retention (Decision 30): finished jobs older than 7 days or beyond the newest 10,000
   * finished go, their `job_items` first, ≤ 500 rows per statement, one statement per
   * event-loop turn. Succeeds with the number of jobs deleted.
   */
  retention(now: number, opts: { maxAgeMs?: number; maxRows?: number } = {}): Effect.Effect<number> {
    return Effect.suspend(() => {
      const maxRows = opts.maxRows ?? RETENTION_MAX_ROWS;
      let cutoff = now - (opts.maxAgeMs ?? RETENTION_MAX_AGE_MS);
      const nth = this.#s1("nthFinished").get(maxRows - 1) as { finishedAt: number } | undefined;
      if (nth !== undefined) cutoff = Math.max(cutoff, nth.finishedAt);
      return this.#store
        .deleteInBatches(
          "DELETE FROM job_items WHERE (jobId, item) IN (SELECT i.jobId, i.item FROM jobs j JOIN job_items i ON i.jobId = j.id " +
            "WHERE j.finishedAt IS NOT NULL AND j.finishedAt < ? LIMIT ?)",
          [cutoff],
        )
        .pipe(
          Effect.flatMap(() =>
            this.#store.deleteInBatches(
              "DELETE FROM jobs WHERE id IN (SELECT id FROM jobs WHERE finishedAt IS NOT NULL AND finishedAt < ? ORDER BY finishedAt LIMIT ?)",
              [cutoff],
            ),
          ),
        );
    });
  }
}

function refuse(what: string): never {
  throw new Error(`job store: ${what}`);
}
