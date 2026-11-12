// The reviews store (5E plan Decisions 8–10, 18): the only module that touches the `reviews`
// chain's tables. Importing it registers the chain (`main.ts` imports it before it opens the
// control store). Review text (titles, summaries, comments, reasons) is content: it is stored
// and returned, never put in an error or an audit row.
import {
  type BranchPolicy,
  type Comment,
  DEFAULT_BRANCH_POLICY,
  type Decision,
  type DecisionValue,
  REVIEW_COMMENT_AT_MAX,
  REVIEW_COMMENT_MAX,
  REVIEW_COMMENTS_MAX,
  REVIEW_LIMIT,
  REVIEW_OPEN_STATES,
  REVIEW_REASON_MAX,
  REVIEW_SUMMARY_MAX,
  REVIEW_TITLE_MAX,
  type RequestedBy,
  type ReviewEvent,
  type ReviewEventKind,
  type ReviewItem,
  type ReviewKind,
  type ReviewState,
  reviewTextProblem,
  type Snapshot,
} from "@finisher/contracts";
import type { StatementSync } from "node:sqlite";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { reviews0001 } from "../control/migrations/reviews/0001.ts";
import { ulid } from "../control/ulid.ts";
import { finError } from "../errors.ts";

registerChain("reviews", [reviews0001], { formatVersion: 1, minReaderVersion: 1 });

/** A new item. `snapshot` is a `branchReview`'s; `subject` a conflict list or `{pluginId}`. */
export interface NewItem {
  readonly projectId: string;
  readonly kind: ReviewKind;
  readonly source?: string;
  readonly target?: string;
  readonly snapshot?: Snapshot;
  readonly title: string;
  readonly summary?: string;
  readonly requestedBy: RequestedBy;
  readonly rederived?: boolean;
  readonly subject?: unknown;
}

export interface NewComment {
  readonly itemId: string;
  readonly principal: string;
  readonly actor?: unknown;
  readonly body: string;
  readonly at?: string;
  readonly txId?: string;
}

/** A decision to record; `pending` takes the mark back. */
export interface DecisionInput {
  readonly txId: string;
  readonly decision: DecisionValue;
  readonly reason?: string | undefined;
}

export interface ItemQuery {
  readonly state?: ReviewState | undefined;
  readonly kind?: ReviewKind | undefined;
}

/** Stored rows of the other tables, as the JSON the columns hold. */
interface Row {
  id: string;
  projectId: string;
  kind: ReviewKind;
  state: ReviewState;
  source: string | null;
  target: string | null;
  snapshot: string | null;
  title: string;
  summary: string | null;
  requestedBy: string;
  createdAt: number;
  updatedAt: number;
  closedAt: number | null;
  closedBy: string | null;
  mergeId: string | null;
  rederived: number;
  subject: string | null;
}

const COLUMNS =
  "id, projectId, kind, state, source, target, snapshot, title, summary, requestedBy, createdAt, updatedAt, closedAt, closedBy, mergeId, rederived, subject";
const OPEN_SQL = REVIEW_OPEN_STATES.map((s) => `'${s}'`).join(", ");

const invalid = (field: string, reason: string, extra: object = {}) =>
  finError("INVALID_REQUEST", `${field} is not acceptable (${reason})`, { field, reason, ...extra }, "Shorten it or remove control characters.");

/** The text rules of Decision 10, the RPC schemas' own (`reviewTextProblem`). */
function checkText(field: string, value: string, o: { readonly min?: number; readonly max: number; readonly bytes?: boolean }): void {
  const problem = reviewTextProblem(value, o);
  if (problem !== undefined) throw invalid(field, problem, problem === "tooLong" ? { max: o.max } : {});
}

const notFound = (id: string) => finError("REVIEW_NOT_FOUND", "no such review item", { itemId: id }, "Check the item's id with a list of the project's reviews.");
const closed = (id: string, state: ReviewState) =>
  finError("REVIEW_CLOSED", `the review item is ${state}`, { itemId: id, state }, "Open a new review for the branch to decide more of it.");

const json = (v: unknown): string => JSON.stringify(v);

function itemOf(r: Row): ReviewItem {
  return {
    id: r.id,
    projectId: r.projectId,
    kind: r.kind,
    state: r.state,
    ...(r.source === null ? {} : { source: r.source }),
    ...(r.target === null ? {} : { target: r.target }),
    ...(r.snapshot === null ? {} : { snapshot: JSON.parse(r.snapshot) as Snapshot }),
    title: r.title,
    ...(r.summary === null ? {} : { summary: r.summary }),
    requestedBy: JSON.parse(r.requestedBy) as RequestedBy,
    createdAt: r.createdAt,
    updatedAt: r.updatedAt,
    ...(r.closedAt === null ? {} : { closedAt: r.closedAt }),
    ...(r.closedBy === null ? {} : { closedBy: r.closedBy }),
    ...(r.mergeId === null ? {} : { mergeId: r.mergeId }),
    rederived: r.rederived === 1,
    ...(r.subject === null ? {} : { subject: JSON.parse(r.subject) as unknown }),
  };
}

export class ReviewStore {
  readonly #store: ControlStore;
  readonly #clock: () => number;
  readonly #onChange: ((item: ReviewItem) => void) | undefined;
  readonly #sql: Record<string, StatementSync> = {};

  /** `onChange` hears every item written (the review hub publishes it). */
  constructor(store: ControlStore, clock: () => number = Date.now, onChange?: (item: ReviewItem) => void) {
    this.#store = store;
    this.#clock = clock;
    this.#onChange = onChange;
  }

  #stmt(sql: string): StatementSync {
    return (this.#sql[sql] ??= this.#store.db.prepare(sql));
  }

  #changed(id: string): ReviewItem {
    const item = this.get(id) as ReviewItem;
    this.#onChange?.(item);
    return item;
  }

  #event(itemId: string, kind: ReviewEventKind, by: string, details: object = {}): void {
    this.#stmt("INSERT INTO review_events (itemId, at, kind, by, details) VALUES (?, ?, ?, ?, ?)").run(itemId, this.#clock(), kind, by, json(details));
  }

  get(id: string): ReviewItem | undefined {
    const r = this.#stmt(`SELECT ${COLUMNS} FROM reviews WHERE id = ?`).get(id) as Row | undefined;
    return r === undefined ? undefined : itemOf(r);
  }

  /** The open `branchReview` or `mergeConflict` of a pair, if any (an open one is unique). */
  findOpen(projectId: string, kind: ReviewKind, source: string, target: string): ReviewItem | undefined {
    const r = this.#stmt(
      `SELECT ${COLUMNS} FROM reviews WHERE projectId = ? AND kind = ? AND source = ? AND target = ? AND state IN (${OPEN_SQL})`,
    ).get(projectId, kind, source, target) as Row | undefined;
    return r === undefined ? undefined : itemOf(r);
  }

  /** Newest first. */
  list(projectId: string, q: ItemQuery = {}): ReadonlyArray<ReviewItem> {
    const where = ["projectId = ?"];
    const args: string[] = [projectId];
    if (q.state !== undefined) (where.push("state = ?"), args.push(q.state));
    if (q.kind !== undefined) (where.push("kind = ?"), args.push(q.kind));
    const rows = this.#store.db.prepare(`SELECT ${COLUMNS} FROM reviews WHERE ${where.join(" AND ")} ORDER BY createdAt DESC, id DESC`).all(...args) as unknown as Row[];
    return rows.map(itemOf);
  }

  /** Items still open (`open` or `changesRequested`) in a project: what `REVIEW_LIMIT` counts. */
  openCount(projectId: string): number {
    const r = this.#stmt(`SELECT COUNT(*) AS n FROM reviews WHERE projectId = ? AND state IN (${OPEN_SQL})`).get(projectId) as { n: number };
    return r.n;
  }

  /**
   * Opens an item. A second open `branchReview` or `mergeConflict` for the same pair answers the
   * open one unchanged (Decision 10; `findOpen` first when the caller must tell). At most
   * `REVIEW_LIMIT` open items per project (`REVIEW_LIMIT`, exit 4).
   */
  open(item: NewItem): ReviewItem {
    checkText("title", item.title, { min: 1, max: REVIEW_TITLE_MAX });
    if (item.summary !== undefined) checkText("summary", item.summary, { max: REVIEW_SUMMARY_MAX, bytes: true });
    return this.#store.tx(() => {
      if (item.source !== undefined && item.target !== undefined && item.kind !== "pluginSuggestion") {
        const existing = this.findOpen(item.projectId, item.kind, item.source, item.target);
        if (existing !== undefined) return existing;
      }
      if (this.openCount(item.projectId) >= REVIEW_LIMIT) {
        throw finError(
          "REVIEW_LIMIT",
          `the project already has ${REVIEW_LIMIT} open review items`,
          { projectId: item.projectId, max: REVIEW_LIMIT },
          "Close or merge some of the open reviews first.",
        );
      }
      const now = this.#clock();
      const id = ulid(now);
      this.#stmt(
        `INSERT INTO reviews (${COLUMNS}) VALUES (?, ?, ?, 'open', ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?)`,
      ).run(
        id,
        item.projectId,
        item.kind,
        item.source ?? null,
        item.target ?? null,
        item.snapshot === undefined ? null : json(item.snapshot),
        item.title,
        item.summary ?? null,
        json(item.requestedBy),
        now,
        now,
        item.rederived === true ? 1 : 0,
        item.subject === undefined ? null : json(item.subject),
      );
      this.#event(id, "opened", item.requestedBy.principal, item.rederived === true ? { rederived: true } : {});
      return this.#changed(id);
    });
  }

  /** The item, open for writing: unknown is `REVIEW_NOT_FOUND`, a closed one `REVIEW_CLOSED`. */
  #writable(id: string): ReviewItem {
    const item = this.get(id);
    if (item === undefined) throw notFound(id);
    if (!REVIEW_OPEN_STATES.includes(item.state)) throw closed(id, item.state);
    return item;
  }

  /** Decisions by origin txId. They outlive refreshes of the snapshot. */
  decisions(id: string): ReadonlyMap<string, Decision> {
    const rows = this.#stmt("SELECT txId, decision, decidedBy, reason, at FROM review_decisions WHERE itemId = ? ORDER BY txId").all(id) as unknown as Array<{
      txId: string;
      decision: "accept" | "reject";
      decidedBy: string;
      reason: string | null;
      at: number;
    }>;
    return new Map(rows.map((r) => [r.txId, { txId: r.txId, decision: r.decision, decidedBy: r.decidedBy, ...(r.reason === null ? {} : { reason: r.reason }), at: r.at }]));
  }

  /** Records decisions in one transaction; `pending` deletes the mark. Later entries win. */
  setDecisions(id: string, ds: ReadonlyArray<DecisionInput>, by: string): void {
    for (const d of ds) if (d.reason !== undefined) checkText("reason", d.reason, { max: REVIEW_REASON_MAX, bytes: true });
    this.#store.tx(() => {
      this.#writable(id);
      const now = this.#clock();
      let accepted = 0;
      let rejected = 0;
      for (const d of ds) {
        if (d.decision === "pending") {
          this.#stmt("DELETE FROM review_decisions WHERE itemId = ? AND txId = ?").run(id, d.txId);
          continue;
        }
        this.#stmt(
          "INSERT INTO review_decisions (itemId, txId, decision, decidedBy, reason, at) VALUES (?, ?, ?, ?, ?, ?) " +
            "ON CONFLICT (itemId, txId) DO UPDATE SET decision = excluded.decision, decidedBy = excluded.decidedBy, reason = excluded.reason, at = excluded.at",
        ).run(id, d.txId, d.decision, by, d.reason ?? null, now);
        if (d.decision === "accept") accepted++;
        else rejected++;
      }
      this.#stmt("UPDATE reviews SET updatedAt = ? WHERE id = ?").run(now, id);
      this.#event(id, "decided", by, { accepted, rejected, pending: ds.length - accepted - rejected });
    });
    this.#changed(id);
  }

  /**
   * Moves an item to `state` (an open one to any state; `open` ↔ `changesRequested`; a closed
   * one is `REVIEW_CLOSED`). `extra.snapshot` replaces the snapshot (a refresh); `extra.mergeId`
   * records a merge. A terminal state stamps `closedAt` and `closedBy`.
   */
  setState(id: string, state: ReviewState, by: string, extra: { readonly mergeId?: string; readonly snapshot?: Snapshot } = {}): ReviewItem {
    this.#store.tx(() => {
      const item = this.#writable(id);
      const now = this.#clock();
      const terminal = !REVIEW_OPEN_STATES.includes(state);
      this.#stmt(
        "UPDATE reviews SET state = ?, updatedAt = ?, closedAt = ?, closedBy = ?, mergeId = COALESCE(?, mergeId), snapshot = COALESCE(?, snapshot) WHERE id = ?",
      ).run(state, now, terminal ? now : null, terminal ? by : null, extra.mergeId ?? null, extra.snapshot === undefined ? null : json(extra.snapshot), id);
      const kind: ReviewEventKind =
        state === "merged" ? "merged" : state === "changesRequested" ? "changesRequested" : state === "superseded" ? "superseded" : terminal ? "closed" : item.state === "changesRequested" ? "reopened" : "refreshed";
      this.#event(id, kind, by, terminal ? { state } : {});
    });
    return this.#changed(id);
  }

  /** Moves the snapshot (a refresh) without changing the state; `details` go into the event (ids and words only). */
  setSnapshot(id: string, snapshot: Snapshot, by: string, details: object = {}): ReviewItem {
    this.#store.tx(() => {
      this.#writable(id);
      this.#stmt("UPDATE reviews SET snapshot = ?, updatedAt = ? WHERE id = ?").run(json(snapshot), this.#clock(), id);
      this.#event(id, "refreshed", by, details);
    });
    return this.#changed(id);
  }

  /** Replaces an open item's `subject` (a `mergeConflict`'s conflict list) and touches it. */
  setSubject(id: string, subject: unknown): ReviewItem {
    this.#store.tx(() => {
      this.#writable(id);
      this.#stmt("UPDATE reviews SET subject = ?, updatedAt = ? WHERE id = ?").run(json(subject), this.#clock(), id);
    });
    return this.#changed(id);
  }

  comment(c: NewComment): Comment {
    checkText("body", c.body, { min: 1, max: REVIEW_COMMENT_MAX, bytes: true });
    if (c.at !== undefined) checkText("at", c.at, { max: REVIEW_COMMENT_AT_MAX });
    const comment = this.#store.tx(() => {
      this.#writable(c.itemId);
      const n = (this.#stmt("SELECT COUNT(*) AS n FROM review_comments WHERE itemId = ?").get(c.itemId) as { n: number }).n;
      if (n >= REVIEW_COMMENTS_MAX) {
        throw finError("REVIEW_LIMIT", `the item already has ${REVIEW_COMMENTS_MAX} comments`, { itemId: c.itemId, max: REVIEW_COMMENTS_MAX, reason: "comments" });
      }
      const now = this.#clock();
      const made: Comment = {
        id: ulid(now),
        itemId: c.itemId,
        principal: c.principal,
        ...(c.actor === undefined ? {} : { actor: c.actor }),
        body: c.body,
        ...(c.at === undefined ? {} : { at: c.at }),
        ...(c.txId === undefined ? {} : { txId: c.txId }),
        createdAt: now,
      };
      this.#stmt("INSERT INTO review_comments (id, itemId, principal, actor, body, at, txId, createdAt) VALUES (?, ?, ?, ?, ?, ?, ?, ?)").run(
        made.id,
        made.itemId,
        made.principal,
        c.actor === undefined ? null : json(c.actor),
        made.body,
        c.at ?? null,
        c.txId ?? null,
        now,
      );
      this.#stmt("UPDATE reviews SET updatedAt = ? WHERE id = ?").run(now, c.itemId);
      return made;
    });
    this.#changed(c.itemId);
    return comment;
  }

  comments(id: string): ReadonlyArray<Comment> {
    const rows = this.#stmt("SELECT id, itemId, principal, actor, body, at, txId, createdAt FROM review_comments WHERE itemId = ? ORDER BY createdAt, id").all(id) as unknown as Array<{
      id: string;
      itemId: string;
      principal: string;
      actor: string | null;
      body: string;
      at: string | null;
      txId: string | null;
      createdAt: number;
    }>;
    return rows.map((r) => ({
      id: r.id,
      itemId: r.itemId,
      principal: r.principal,
      ...(r.actor === null ? {} : { actor: JSON.parse(r.actor) as unknown }),
      body: r.body,
      ...(r.at === null ? {} : { at: r.at }),
      ...(r.txId === null ? {} : { txId: r.txId }),
      createdAt: r.createdAt,
    }));
  }

  events(id: string): ReadonlyArray<ReviewEvent> {
    const rows = this.#stmt("SELECT id, itemId, at, kind, by, details FROM review_events WHERE itemId = ? ORDER BY id").all(id) as unknown as Array<{
      id: number;
      itemId: string;
      at: number;
      kind: ReviewEventKind;
      by: string;
      details: string;
    }>;
    return rows.map((r) => ({ ...r, details: JSON.parse(r.details) as Record<string, unknown> }));
  }

  /** The project's policy; the default (`main` protected, nothing else on) when none was set. */
  policy(projectId: string): BranchPolicy {
    const r = this.#stmt("SELECT protected, requireReview, autoMerge FROM branch_policy WHERE projectId = ?").get(projectId) as
      | { protected: string; requireReview: number; autoMerge: string }
      | undefined;
    if (r === undefined) return DEFAULT_BRANCH_POLICY;
    return { protected: JSON.parse(r.protected) as string[], requireReview: r.requireReview === 1, autoMerge: JSON.parse(r.autoMerge) as { enabled: boolean } };
  }

  /** Stores the policy and answers the one it replaced (the default when none was set). */
  setPolicy(projectId: string, p: BranchPolicy): { readonly old: BranchPolicy } {
    return this.#store.tx(() => {
      const old = this.policy(projectId);
      this.#stmt(
        "INSERT INTO branch_policy (projectId, protected, requireReview, autoMerge) VALUES (?, ?, ?, ?) " +
          "ON CONFLICT (projectId) DO UPDATE SET protected = excluded.protected, requireReview = excluded.requireReview, autoMerge = excluded.autoMerge",
      ).run(projectId, json([...new Set(p.protected)]), p.requireReview ? 1 : 0, json(p.autoMerge));
      return { old };
    });
  }
}
