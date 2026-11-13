// The agent session store (7A-2 Task 3b; plan 7A Decisions 1–4, spec §3.2–3.4): the only
// module that touches the `agent` chain's tables. Importing it registers the chain (`main.ts`
// imports it before it opens the control store).
//
// - A session's status changes only through `session.ts`'s `next`, its effects inside
//   `control.db` applied in the same `ControlStore.tx` (runs opened and closed, synthetic tool
//   results, approvals expired) and the others queued in `session_outbox` in that transaction.
// - Nodes are appended in call order per session (one queue per session): a payload over
//   4 KiB is written to its blob first (async file I/O), then the row, so a crash between the
//   two leaves an orphan blob and never a row naming a missing file.
// - Crash recovery (Decision 3) is idempotent: a call gets at most one result (`nodes_call`),
//   a session is interrupted once, and what the outbox still holds is redone at each start.
import type { StatementSync } from "node:sqlite";
import type { Approval, ApprovalDecision, Node, Run, Session, Usage } from "@finisher/contracts";
import { Effect } from "effect";
import type { ControlStore } from "../control/db.ts";
import { registerChain } from "../control/migrations.ts";
import { AGENT_MIGRATIONS } from "../control/migrations/agent/index.ts";
import { finError } from "../errors.ts";
import { dirOlderThan, ensureAgentsRoot, listSessionDirs, makeSessionDirs, readBlob, removeBlob, removeSessionDir, sweepBlobs, writeBlob } from "./blobs.ts";
import type { Clock, Ids } from "./clock.ts";
import type { LedgerStore } from "./ledger.ts";
import { initialState, next, type SessionEffect, type SessionEvent, type SessionState } from "./session.ts";
import type { Appended, ApprovalRow, NewApproval, NewNode, NewSession, NodeRow, OutboxEffect, OutboxItem, Recovery, RunRow, RunStart, SessionQuery, SessionRow } from "./store-rows.ts";
import { type Applied, addUsage, approvalExpired, INLINE_MAX, invalid, outboxOf, SYNTHETIC_RESULT, STORE_SQL, sessionNotFound, stateOf } from "./store-rows.ts";
import { sessionInsertArgs, sessionListQuery, toApproval, toNode, toRun, toSession, ZERO_USAGE } from "./store-rows.ts";
import { SessionTxOutcomes } from "./store-outcomes.ts";
import { SessionRollbacks } from "./store-rollback.ts";
import { DbLedger, type SessionBranchState, SessionReviews, type SessionTx, SessionStates, SessionTree, SessionTxs, type TxOutcome } from "./store-tables.ts";

export type { Appended, Applied, NewApproval, NewNode, NewSession, OutboxEffect, OutboxItem, Recovery, RunStart, SessionQuery } from "./store-rows.ts";
export type { SessionBranchState, SessionTx, SessionTryRow, TxOutcome } from "./store-tables.ts";

registerChain("agent", [...AGENT_MIGRATIONS], { formatVersion: 12, minReaderVersion: 8 });

const DAY_MS = 86_400_000;
/** `agents.retentionDays` default (Decision 4). */
export const RETENTION_DAYS = 90;
/** An orphan blob or session directory younger than this is left alone by the hourly sweep. */
export const ORPHAN_GRACE_MS = 60 * 60_000;
const NODE_PAGE_MAX = 1000;
const SWEEP_PAGE = 100;
/** The reason a session interrupted by a restart carries (spec §3.4). */
export const SERVER_RESTART = "serverRestart";

export class AgentStore {
  readonly #cs: ControlStore;
  readonly #clock: Clock;
  readonly #ids: Ids;
  readonly #s: Record<string, StatementSync>;
  readonly #lists = new Map<string, StatementSync>();
  readonly #queues = new Map<string, Promise<unknown>>();
  readonly #txs: SessionTxs;
  readonly #states: SessionStates;
  readonly #ledger: DbLedger;
  readonly outcomes: SessionTxOutcomes; // every outcome of every agent transaction (Task 11)
  readonly tree: SessionTree; // parents, children, live counts (7B Task 6)
  readonly rollbacks: SessionRollbacks; // rolled-back runs' nodes (7B Task 8)
  readonly reviews: SessionReviews; // the review items each session asked for (7B Task 11)
  #closed = 0; // synthetic results written since the store opened
  readonly root: string; // `<server-data>/agents`

  constructor(control: ControlStore, serverData: string, clock: Clock, ids: Ids) {
    this.#cs = control;
    this.#clock = clock;
    this.#ids = ids;
    this.root = ensureAgentsRoot(serverData);
    this.#txs = new SessionTxs(control);
    this.#states = new SessionStates(control);
    this.#ledger = new DbLedger(control);
    this.outcomes = new SessionTxOutcomes(control);
    this.tree = new SessionTree(control);
    this.rollbacks = new SessionRollbacks(control);
    this.reviews = new SessionReviews(control);
    this.#s = Object.fromEntries(Object.entries(STORE_SQL).map(([k, sql]) => [k, control.db.prepare(sql)]));
  }

  #row(sessionId: string): SessionRow {
    return (this.#s.session!.get(sessionId) as unknown as SessionRow | undefined) ?? sessionNotFound(sessionId);
  }

  #runRow(runId: string): RunRow | undefined {
    return this.#s.run!.get(runId) as unknown as RunRow | undefined;
  }

  // Sessions ---------------------------------------------------------------------------------

  /**
   * Records a new session in `starting` with its `session_state` (default branch: the root, or
   * in `plan` mode the base branch, which it only reads). Its directories are made first.
   */
  async createSession(s: NewSession): Promise<Session> {
    if (s.mode === "plan" && s.rootBranch !== undefined) invalid("a plan session has no branch of its own", { mode: s.mode });
    if (s.mode !== "plan" && s.rootBranch === undefined) invalid("a session that edits needs its root branch", { mode: s.mode });
    await makeSessionDirs(this.root, s.id);
    return this.#cs.tx(() => {
      if (this.#s.exists!.get(s.id) !== undefined) throw finError("INVALID_REQUEST", "a session with this id exists", { sessionId: s.id });
      this.#s.insertSession!.run(...sessionInsertArgs(s, initialState.status, this.#clock.now()));
      if (s.carve !== undefined && s.parent !== undefined) this.#ledger.carve(s.parent, s.id, s.carve);
      this.#states.create(s.id, s.rootBranch ?? s.base.branch);
      return toSession(this.#row(s.id));
    });
  }

  get(sessionId: string): Session | undefined {
    const r = this.#s.session!.get(sessionId) as unknown as SessionRow | undefined;
    return r === undefined ? undefined : toSession(r);
  }

  /** Newest first. `next` is set when another page may follow. */
  list(q: SessionQuery = {}): { sessions: Session[]; next?: string } {
    if (q.projects !== undefined && q.projects.length === 0) return { sessions: [] };
    const { sql, args, limit } = sessionListQuery(q);
    let stmt = this.#lists.get(sql);
    if (stmt === undefined) this.#lists.set(sql, (stmt = this.#cs.db.prepare(sql)));
    const rows = stmt.all(...args, limit + 1) as unknown as SessionRow[];
    const sessions = rows.slice(0, limit).map(toSession);
    return rows.length > limit ? { sessions, next: sessions[sessions.length - 1]!.id } : { sessions };
  }

  /** The active run's id (`running`, `waitingApproval`), else null. */
  activeRun(sessionId: string): string | null {
    return this.#row(sessionId).runId;
  }

  /** The session's highest node seq (0 before its first node). */
  lastSeq(sessionId: string): number {
    return (this.#s.lastNodeSeq!.get(sessionId) as { seq: number } | undefined)?.seq ?? 0;
  }

  providerRef(sessionId: string): string | undefined {
    return this.#row(sessionId).providerRef ?? undefined;
  }

  /** The role's tool allow-set the session started with (agent 0010), null when none was recorded. */
  tools(sessionId: string): string[] | null {
    const t = this.#row(sessionId).tools;
    return t === null ? null : (JSON.parse(t) as string[]);
  }

  setProviderRef(sessionId: string, ref: string | null): void {
    this.#row(sessionId);
    this.#s.providerRef!.run(ref, sessionId);
  }

  /**
   * Feeds `e` to the session's state machine in one transaction: the new state, the effects
   * inside `control.db`, and the outbox entries for the others. Throws the machine's
   * `SESSION_BUSY`, `SESSION_ENDED` or `SESSION_INTERRUPTED`; `SESSION_NOT_FOUND`.
   */
  apply(sessionId: string, e: SessionEvent, run: RunStart = {}): Applied {
    return this.#cs.tx(() => this.#applyIn(sessionId, e, run));
  }

  /** Opens a new run (`runStarted` with a fresh id). */
  startRun(sessionId: string, r: RunStart = {}): { session: Session; run: Run } {
    return this.#cs.tx(() => {
      const runId = this.#ids.ulid();
      const { session } = this.#applyIn(sessionId, { kind: "runStarted", runId }, r);
      return { session, run: toRun(this.#runRow(runId)!) };
    });
  }

  #applyIn(sessionId: string, e: SessionEvent, run: RunStart): Applied {
    const row = this.#row(sessionId);
    const t = next(stateOf(row), e);
    if ("error" in t) throw t.error;
    const now = this.#clock.now();
    for (const effect of t.effects) this.#effect(row, t.state, effect, run, now);
    for (const o of outboxOf(t.effects)) this.#queue(sessionId, o, now);
    const s = t.state;
    if (e.kind === "ready" && s.status === "idle") this.#s.markReady!.run(now, sessionId);
    if (s.status === "ended") this.#ledger.settleCarve(sessionId);
    this.#s.setState!.run(s.status, s.reason, s.runId, JSON.stringify(s.approvals), s.status === "ended" ? now : null, sessionId);
    return { session: toSession(this.#row(sessionId)), effects: t.effects };
  }

  #effect(row: SessionRow, s: SessionState, e: SessionEffect, run: RunStart, now: number): void {
    switch (e.kind) {
      case "openRun": {
        const last = this.#s.lastRunSeq!.get(row.id) as { seq: number } | undefined;
        const seq = (last?.seq ?? 0) + 1;
        this.#s.insertRun!.run(e.runId, row.id, seq, run.trigger ?? "prompt", now, run.headAtStart ?? null, JSON.stringify(ZERO_USAGE));
        return;
      }
      case "closeRun":
        this.#s.closeRun!.run(e.status, now, e.stop, e.runId);
        return;
      case "closeOpenToolCalls":
        this.#closeCalls(row.id, e.runId, s.reason, now);
        return;
      case "expireApprovals":
        for (const id of e.ids) this.#s.expireApproval!.run(id);
        return;
      default:
        // stopProvider is the caller's now; the rest go to the outbox (`outboxOf`).
        return;
    }
  }

  /** Gives each announced call of `runId` without a result a synthetic one. Answers how many. */
  #closeCalls(sessionId: string, runId: string, reason: string | null, now: number): number {
    const open = this.#s.openCalls!.all(runId) as unknown as NodeRow[];
    for (const call of open) {
      const id = this.#ids.ulid();
      this.#insertNode(id, sessionId, runId, "toolResult", call.providerId, call.callId, JSON.stringify(SYNTHETIC_RESULT(reason)), null, now);
      this.#queue(sessionId, { kind: "toolResult", nodeId: id, callId: call.callId!, runId }, now);
    }
    this.#closed += open.length;
    return open.length;
  }

  #queue(sessionId: string, effect: OutboxEffect, now: number): void {
    this.#s.outboxAdd!.run(sessionId, JSON.stringify(effect), now);
  }

  // Runs ------------------------------------------------------------------------------------

  run(runId: string): Run | undefined {
    const r = this.#runRow(runId);
    return r === undefined ? undefined : toRun(r);
  }

  runs(sessionId: string): Run[] {
    return (this.#s.runs!.all(sessionId) as unknown as RunRow[]).map(toRun);
  }

  /** Adds a model call's usage to its run and to the session, in one transaction. */
  addUsage(runId: string, delta: Usage): { run: Run; session: Session } {
    return this.#cs.tx(() => {
      const r = this.#runRow(runId) ?? invalid("no such run", { runId });
      this.#s.runUsage!.run(JSON.stringify(addUsage(JSON.parse(r.usage) as Usage, delta)), runId);
      const s = this.#row(r.sessionId);
      this.#s.sessionUsage!.run(JSON.stringify(addUsage(JSON.parse(s.usage) as Usage, delta)), s.id);
      return { run: toRun(this.#runRow(runId)!), session: toSession(this.#row(s.id)) };
    });
  }

  // Nodes -----------------------------------------------------------------------------------

  /**
   * Appends a node after every earlier `appendNode` of the session. A payload over 4 KiB of
   * JSON goes to its blob first. A `toolCall` needs the run to be open; a `toolResult` needs
   * its call; a second node of the same kind for one call is not written (a late result after
   * the synthetic one): the first is answered with `inserted: false`.
   */
  appendNode(n: NewNode): Promise<Appended> {
    const prev = this.#queues.get(n.sessionId) ?? Promise.resolve();
    const done = prev.then(
      () => this.#append(n),
      () => this.#append(n),
    );
    const tail = done.catch(() => {});
    this.#queues.set(n.sessionId, tail);
    void tail.then(() => {
      if (this.#queues.get(n.sessionId) === tail) this.#queues.delete(n.sessionId);
    });
    return done;
  }

  async #append(n: NewNode): Promise<Appended> {
    const isCall = n.kind === "toolCall" || n.kind === "toolResult";
    if (isCall && n.callId === undefined) invalid(`a ${n.kind} node needs its call id`, { kind: n.kind });
    if (!isCall && n.callId !== undefined) invalid("only tool nodes carry a call id", { kind: n.kind });
    const json = JSON.stringify(n.payload ?? null);
    const bytes = Buffer.from(json, "utf8");
    const blobRef = bytes.byteLength > INLINE_MAX ? await writeBlob(this.root, n.sessionId, bytes) : null;
    try {
      return this.#cs.tx(() => {
        this.#row(n.sessionId);
        const run = this.#runRow(n.runId);
        if (run === undefined || run.sessionId !== n.sessionId) invalid("the run is not this session's", { runId: n.runId });
        if (isCall) {
          const known = this.#s.callNode!.get(n.callId!, n.kind) as unknown as NodeRow | undefined;
          if (known !== undefined) return { node: toNode(known), inserted: false };
          if (n.kind === "toolCall" && run.status !== "running") {
            throw finError("SESSION_INTERRUPTED", "the run is over: no tool call can start in it", { runId: n.runId, status: run.status });
          }
          if (n.kind === "toolResult" && this.#s.callNode!.get(n.callId!, "toolCall") === undefined) {
            invalid("a tool result needs its call", { callId: n.callId });
          }
        }
        const id = this.#ids.ulid();
        this.#insertNode(id, n.sessionId, n.runId, n.kind, n.providerId ?? null, n.callId ?? null, blobRef === null ? json : null, blobRef, this.#clock.now());
        return { node: toNode(this.#s.nodeById!.get(id) as unknown as NodeRow), inserted: true };
      });
    } catch (e) {
      // The blob no row names is removed now (appends of one session are serialised).
      if (blobRef !== null && this.#s.blobUsed!.get(n.sessionId, blobRef) === undefined) {
        await removeBlob(this.root, n.sessionId, blobRef).catch(() => {});
      }
      throw e;
    }
  }

  #insertNode(id: string, sessionId: string, runId: string, kind: string, providerId: string | null, callId: string | null, payload: string | null, blobRef: string | null, now: number): void {
    const last = this.#s.lastNodeSeq!.get(sessionId) as { seq: number } | undefined;
    this.#s.insertNode!.run(id, runId, sessionId, (last?.seq ?? 0) + 1, kind, providerId, callId, payload, blobRef, now);
  }

  /** The session's nodes from `fromSeq` (inclusive, default 1) on, at most `limit` (≤ 1000). */
  nodes(sessionId: string, fromSeq = 1, limit = 500): Node[] {
    const n = Math.min(Math.max(Math.trunc(limit), 1), NODE_PAGE_MAX);
    return (this.#s.nodes!.all(sessionId, fromSeq, n) as unknown as NodeRow[]).map(toNode);
  }

  /** A node's payload, inline or read from its blob. */
  async payload(node: Node): Promise<unknown> {
    if (node.blobRef === undefined) return node.payload;
    return JSON.parse((await readBlob(this.root, node.sessionId, node.blobRef)).toString("utf8")) as unknown;
  }

  #blobRefs(sessionId: string): string[] {
    return (this.#s.blobRefs!.all(sessionId) as Array<{ blobRef: string }>).map((r) => r.blobRef);
  }

  // Approvals -------------------------------------------------------------------------------

  /** Opens an approval and feeds `approvalOpened`; one the session cannot wait on is expired at once. */
  openApproval(a: NewApproval): { approval: Approval; session: Session } {
    return this.#cs.tx(() => {
      this.#row(a.sessionId);
      const id = this.#ids.ulid();
      this.#s.insertApproval!.run(id, a.sessionId, a.kind, JSON.stringify(a.subject ?? null), a.argsHash, this.#clock.now(), a.expiresAt);
      const { session } = this.#applyIn(a.sessionId, { kind: "approvalOpened", id }, {});
      return { approval: this.approval(id)!, session };
    });
  }

  /**
   * Answers an open approval (`by`: `kind:id` of the principal, or `policy`); else
   * `INVALID_REQUEST`. One past its `expiresAt` is expired (and leaves the session's open list)
   * instead, and the answer is `APPROVAL_EXPIRED`.
   */
  answerApproval(id: string, decision: ApprovalDecision, by: string): { approval: Approval; session: Session } {
    const answered = this.#cs.tx(() => {
      const a = this.approval(id) ?? invalid("no such approval", { id });
      const now = this.#clock.now();
      if (a.status === "open" && a.expiresAt <= now) {
        this.#expireIn(a.id, a.sessionId);
        return null;
      }
      if (Number(this.#s.answerApproval!.run(decision, by, now, id).changes) === 0) {
        invalid(`the approval is ${a.status}`, { id, status: a.status });
      }
      const { session } = this.#applyIn(a.sessionId, { kind: "approvalAnswered", id }, {});
      return { approval: this.approval(id)!, session };
    });
    if (answered === null) throw approvalExpired(id);
    return answered;
  }

  /**
   * The hourly sweep's part (Decision 21): every open approval past its `expiresAt` is
   * expired and leaves its session's open list. Answers the sessions it changed.
   */
  expireOverdueApprovals(now = this.#clock.now()): Session[] {
    const changed = new Map<string, Session>();
    for (;;) {
      const rows = this.#s.overdueApprovals!.all(now, SWEEP_PAGE) as Array<{ id: string; sessionId: string }>;
      for (const r of rows) {
        const s = this.#cs.tx(() => this.#expireIn(r.id, r.sessionId));
        if (s !== undefined) changed.set(s.id, s);
      }
      if (rows.length < SWEEP_PAGE) break;
    }
    return [...changed.values()];
  }

  /** Expires open approval `id` now (its provider withdrew the request); the session when it changed. */
  expireApproval(id: string): Session | undefined {
    const a = this.approval(id);
    return a?.status === "open" ? this.#cs.tx(() => this.#expireIn(id, a.sessionId)) : undefined;
  }

  /** Expires one open approval in the caller's tx; the session's state when it changed. */
  #expireIn(id: string, sessionId: string): Session | undefined {
    const row = this.#s.session!.get(sessionId) as unknown as SessionRow | undefined;
    if (row === undefined || row.status === "ended") {
      this.#s.expireApproval!.run(id);
      return undefined;
    }
    return this.#applyIn(sessionId, { kind: "approvalExpired", id }, {}).session;
  }

  /** Marks an answered approval used (a one-time grant); false when it was not answered or is used. */
  consumeApproval(id: string): boolean {
    return Number(this.#s.consumeApproval!.run(this.#clock.now(), id).changes) > 0;
  }

  approvals(sessionId: string): Approval[] {
    return (this.#s.approvalsOf!.all(sessionId) as unknown as ApprovalRow[]).map(toApproval);
  }

  approval(id: string): Approval | undefined {
    const r = this.#s.approval!.get(id) as unknown as ApprovalRow | undefined;
    return r === undefined ? undefined : toApproval(r);
  }

  // Transactions and branch state -----------------------------------------------------------

  /**
   * Whether the session ever became `idle` (its credential was minted first). False only for
   * one a crash stopped between its record and its credential.
   */
  everReady(sessionId: string): boolean {
    const r = this.#s.readyAt!.get(sessionId) as { readyAt: number | null } | undefined;
    return r !== undefined && r.readyAt !== null;
  }

  /** Records an agent principal's transaction under its session (the principal id). */
  recordTx(t: { txId: string; sessionId: string; callId: string | null; attribution: SessionTx["attribution"]; at: number }): void {
    this.#txs.record(t);
  }

  sessionOfTx(txIds: ReadonlyArray<string>): Map<string, SessionTx> {
    return this.#txs.of(txIds);
  }

  txsOfSession(sessionId: string, since?: number): SessionTx[] {
    return this.#txs.bySession(sessionId, since);
  }

  txsOfCall(callId: string): SessionTx[] {
    return this.#txs.byCall(callId);
  }

  setOutcome(txId: string, outcome: TxOutcome): boolean {
    return this.#txs.setOutcome(txId, outcome);
  }

  /**
   * Removes `session_txs` rows (owner decision: they outlive their session, so the outcome
   * tracker can attribute late merges): every row of a project that is removed, or of one
   * session by an explicit admin purge. Neither a session's delete nor its retention sweep does.
   * Answers how many rows went.
   */
  async purgeTxs(of: { readonly projectId: string } | { readonly sessionId: string }): Promise<number> {
    const [column, value] = "projectId" in of ? ["projectId", of.projectId] : ["sessionId", of.sessionId];
    // Their outcomes first, while the rows still name them.
    await Effect.runPromise(
      this.#cs.deleteInBatches(
        `DELETE FROM session_tx_outcomes WHERE seq IN (SELECT o.seq FROM session_tx_outcomes o JOIN session_txs t USING (txId) WHERE t.${column} = ? LIMIT ?)`,
        [value],
      ),
    );
    return Effect.runPromise(
      this.#cs.deleteInBatches(`DELETE FROM session_txs WHERE txId IN (SELECT txId FROM session_txs WHERE ${column} = ? LIMIT ?)`, [value]),
    );
  }

  state(sessionId: string): SessionBranchState {
    this.#row(sessionId);
    return this.#states.get(sessionId) ?? sessionNotFound(sessionId);
  }

  #live(sessionId: string): { row: SessionRow; state: SessionBranchState } {
    const row = this.#row(sessionId);
    if (row.status === "ended") throw finError("SESSION_ENDED", "the session has ended", { status: row.status }, "start a new session");
    return { row, state: this.#states.get(row.id) ?? sessionNotFound(sessionId) };
  }

  setDefaultBranch(sessionId: string, branch: string): SessionBranchState {
    return this.#cs.tx(() => {
      const { row, state } = this.#live(sessionId);
      return this.#states.setDefault(sessionId, state, row.rootBranch ?? row.baseBranch, branch);
    });
  }

  recordTry(sessionId: string, branch: string): SessionBranchState {
    return this.#cs.tx(() => this.#states.recordTry(sessionId, this.#live(sessionId).state, branch));
  }

  clearTry(sessionId: string, branch: string): SessionBranchState {
    return this.#cs.tx(() => this.#states.clearTry(sessionId, this.#live(sessionId).state, branch));
  }

  /** The budget ledger over `budgets` and `budget_days` (Task 4's `LedgerStore`). */
  ledger(): LedgerStore {
    return this.#ledger;
  }

  // The outbox ------------------------------------------------------------------------------

  /** Effects still owed outside `control.db`, oldest first (all sessions, or one). */
  outbox(sessionId?: string): OutboxItem[] {
    const rows = (sessionId === undefined ? this.#s.outboxAll!.all() : this.#s.outboxOf!.all(sessionId)) as unknown as Array<{
      id: number;
      sessionId: string;
      effect: string;
      at: number;
    }>;
    return rows.map((r) => ({ id: r.id, sessionId: r.sessionId, effect: JSON.parse(r.effect) as OutboxEffect, at: r.at }));
  }

  /** The effect is done: it is not redone at the next start. */
  outboxDone(id: number): void {
    this.#s.outboxDone!.run(id);
  }

  // Recovery, retention, delete -------------------------------------------------------------

  /**
   * Decision 3, at start before anything else uses the store: every `starting`, `running` or
   * `waitingApproval` session becomes `interrupted` (`serverRestart`), its run closed, its open
   * calls given a synthetic result and its approvals expired; a run still open under any other
   * session is closed the same way. Each session is one transaction; running it again finds
   * nothing to do. The journal entries owed are in `outbox()`, with those an earlier start left.
   */
  recoverAfterCrash(): Recovery {
    const interrupted: string[] = [];
    const before = this.#closed;
    // Money and tokens held for model calls that died with the last process: no call settles
    // them now, so they are returned before anything else (the outbox included) runs.
    // Children's carves are held again, and settled for children that have ended (`carve.ts`).
    this.#cs.tx(() => this.#ledger.recover());
    for (const { id } of this.#s.live!.all() as Array<{ id: string }>) {
      this.#cs.tx(() => this.#applyIn(id, { kind: "interrupted", reason: SERVER_RESTART }, {}));
      interrupted.push(id);
    }
    for (const r of this.#s.openRuns!.all() as unknown as RunRow[]) {
      this.#cs.tx(() => {
        const now = this.#clock.now();
        this.#closeCalls(r.sessionId, r.id, SERVER_RESTART, now);
        this.#s.closeRun!.run("interrupted", now, SERVER_RESTART, r.id);
      });
    }
    return { interrupted, closedCalls: this.#closed - before };
  }

  /**
   * Removes the session directories no row names and the blobs no node names, when older than
   * `graceMs` (at start, before any session work, 0). Answers how many entries went.
   */
  async sweepOrphans(graceMs = ORPHAN_GRACE_MS): Promise<number> {
    const now = this.#clock.now();
    let removed = 0;
    for (const id of await listSessionDirs(this.root)) {
      if (this.#s.exists!.get(id) === undefined) {
        if (await dirOlderThan(this.root, id, now, graceMs)) {
          await removeSessionDir(this.root, id);
          removed++;
        }
        continue;
      }
      // Appends of one session are serialised: wait for those in flight, then look.
      await this.#queues.get(id);
      removed += await sweepBlobs(this.root, id, new Set(this.#blobRefs(id)), now, graceMs);
    }
    return removed;
  }

  /**
   * Decision 4 (hourly): the product state of sessions ended more than `retentionDays` ago,
   * rows first, then their directories, then the orphans. Answers how many sessions went.
   */
  async sweep(now: number, retentionDays = RETENTION_DAYS, graceMs = ORPHAN_GRACE_MS): Promise<number> {
    const cutoff = now - retentionDays * DAY_MS;
    let removed = 0;
    for (;;) {
      const ids = (this.#s.expired!.all(cutoff, SWEEP_PAGE) as Array<{ id: string }>).map((r) => r.id);
      for (const id of ids) {
        await this.#removeRows(id);
        await removeSessionDir(this.root, id);
        removed++;
      }
      if (ids.length < SWEEP_PAGE) break;
    }
    await this.sweepOrphans(graceMs);
    return removed;
  }

  /**
   * `agent.session.delete` (owner): ends a live session first (its credential is revoked
   * through the outbox, which outlives the session), then removes its rows and directory.
   * Answers the effects of that end, for the caller to stop the provider.
   */
  async delete(sessionId: string): Promise<{ effects: ReadonlyArray<SessionEffect> }> {
    const row = this.#row(sessionId);
    const effects = row.status === "ended" ? [] : this.apply(sessionId, { kind: "ended", reason: "deleted" }).effects;
    await this.#queues.get(sessionId);
    await this.#removeRows(sessionId);
    await removeSessionDir(this.root, sessionId);
    return { effects };
  }

  /** A session's product state; its `session_txs` rows stay (`purgeTxs`). */
  async #removeRows(sessionId: string): Promise<void> {
    await Effect.runPromise(
      this.#cs.deleteInBatches("DELETE FROM nodes WHERE id IN (SELECT id FROM nodes WHERE sessionId = ? LIMIT ?)", [sessionId]),
    );
    this.#cs.tx(() => {
      const db = this.#cs.db;
      db.prepare("DELETE FROM nodes WHERE sessionId = ?").run(sessionId);
      for (const t of ["runs", "approvals", "budgets", "budget_carves", "session_state", "agent_rules", "agent_remembered"]) db.prepare(`DELETE FROM ${t} WHERE sessionId = ?`).run(sessionId);
      db.prepare("DELETE FROM sessions WHERE id = ?").run(sessionId);
    });
  }
}
