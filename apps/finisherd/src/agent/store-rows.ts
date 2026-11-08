// Rows of the `agent` chain and their mapping to the contracts' records (plan 7A Decision 1).
// Only `store.ts` and `store-tables.ts` use it.
import type { AgentProvider, Approval, ApprovalKind, FinError, Node, NodeKind, Run, RunStatus, RunTrigger, Session, Usage } from "@finisher/contracts";
import type { BudgetSpec, Harness, Mode, SessionStatus, Weights } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { SessionEffect, SessionState } from "./session.ts";

/** Node payloads up to this many bytes of JSON are inline; larger ones are blobs (Decision 1). */
export const INLINE_MAX = 4096;

export const ZERO_USAGE: Usage = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, moneyMicros: 0 };

/** A session as `createSession` takes it: everything but the state the store keeps. */
export interface NewSession {
  /** The session id, which is also the agent credential's id (Decision 15). */
  readonly id: string;
  readonly projectId: string;
  readonly harness: Harness;
  readonly provider: AgentProvider;
  readonly model: string;
  readonly weights: Weights;
  readonly role: string;
  readonly mode: Mode;
  /** The session's branch; required unless `mode` is `plan`, which has none. */
  readonly rootBranch?: string;
  readonly base: { readonly branch: string; readonly seq: number };
  readonly parent?: string;
  readonly budget: BudgetSpec;
  readonly createdBy: Session["createdBy"];
  readonly title?: string;
  readonly providerRef?: string;
}

export interface NewNode {
  readonly sessionId: string;
  readonly runId: string;
  readonly kind: NodeKind;
  readonly providerId?: string;
  /** Required for `toolCall` and `toolResult`: the Finisher call id (`FIN_CALL`). */
  readonly callId?: string;
  readonly payload: unknown;
}

/** What a run starts with besides its id. */
export interface RunStart {
  readonly trigger?: RunTrigger;
  /** The root branch's seq when the run began. */
  readonly headAtStart?: number;
}

/** What `apply` did: the session after it and every effect of the transition. */
export interface Applied {
  readonly session: Session;
  /**
   * All effects, in order. Those inside `control.db` are done; the caller does `stopProvider`
   * now, and drains the rest from `outbox()` (they are queued there in the same transaction).
   */
  readonly effects: ReadonlyArray<SessionEffect>;
}

/**
 * An effect owed outside `control.db`, queued in a transition's transaction and removed by
 * `outboxDone` once done: a crash between the two leaves it for the next start (at least once;
 * the job engine's `announce` marker). `toolResult` is a synthetic result to journal.
 */
export type OutboxEffect =
  | { readonly kind: "journal"; readonly phase: "started" | "resumed" | "interrupted" | "ended"; readonly reason: string | null }
  | { readonly kind: "revokeCredential" }
  | { readonly kind: "refreshCredential" }
  | { readonly kind: "toolResult"; readonly nodeId: string; readonly callId: string; readonly runId: string };

export interface OutboxItem {
  readonly id: number;
  readonly sessionId: string;
  readonly effect: OutboxEffect;
  readonly at: number;
}

export interface SessionRow {
  id: string;
  projectId: string;
  harness: string;
  provider: string;
  model: string;
  weights: string;
  role: string;
  mode: string;
  rootBranch: string | null;
  baseBranch: string;
  baseSeq: number;
  parent: string | null;
  status: string;
  statusReason: string | null;
  runId: string | null;
  approvals: string;
  credentialId: string;
  providerRef: string | null;
  budget: string;
  usage: string;
  createdBy: string;
  createdAt: number;
  endedAt: number | null;
  title: string | null;
}

export const SESSION_COLUMNS =
  "id, projectId, harness, provider, model, weights, role, mode, rootBranch, baseBranch, baseSeq, parent, status, " +
  "statusReason, runId, approvals, credentialId, providerRef, budget, usage, createdBy, createdAt, endedAt, title";

export interface RunRow {
  id: string;
  sessionId: string;
  seq: number;
  trigger: string;
  status: string;
  startedAt: number;
  endedAt: number | null;
  headAtStart: number | null;
  stopReason: string | null;
  usage: string;
}

export const RUN_COLUMNS = "id, sessionId, seq, trigger, status, startedAt, endedAt, headAtStart, stopReason, usage";

export interface NodeRow {
  id: string;
  runId: string;
  sessionId: string;
  seq: number;
  kind: string;
  providerId: string | null;
  callId: string | null;
  payload: string | null;
  blobRef: string | null;
  createdAt: number;
}

export const NODE_COLUMNS = "id, runId, sessionId, seq, kind, providerId, callId, payload, blobRef, createdAt";

export interface ApprovalRow {
  id: string;
  sessionId: string;
  kind: string;
  status: string;
  subject: string;
  argsHash: string;
  requestedAt: number;
  expiresAt: number;
  decision: string | null;
  by: string | null;
  answeredAt: number | null;
  consumedAt: number | null;
}

export const APPROVAL_COLUMNS = "id, sessionId, kind, status, subject, argsHash, requestedAt, expiresAt, decision, by, answeredAt, consumedAt";

const opt = <K extends string, V>(key: K, v: V | null | undefined): { [P in K]?: V } =>
  (v === null || v === undefined ? {} : { [key]: v }) as { [P in K]?: V };

export function toSession(r: SessionRow): Session {
  return {
    id: r.id,
    projectId: r.projectId,
    harness: r.harness as Harness,
    provider: r.provider as AgentProvider,
    model: r.model,
    weights: r.weights as Weights,
    role: r.role,
    mode: r.mode as Mode,
    ...opt("rootBranch", r.rootBranch),
    base: { branch: r.baseBranch, seq: r.baseSeq },
    ...opt("parent", r.parent),
    status: r.status as SessionStatus,
    ...opt("statusReason", r.statusReason),
    credentialId: r.credentialId,
    budget: JSON.parse(r.budget) as BudgetSpec,
    usage: JSON.parse(r.usage) as Usage,
    createdBy: JSON.parse(r.createdBy) as Session["createdBy"],
    createdAt: r.createdAt,
    ...opt("endedAt", r.endedAt),
    ...opt("title", r.title),
  };
}

/** The state machine's view of a row. */
export function stateOf(r: SessionRow): SessionState {
  return {
    status: r.status as SessionStatus,
    runId: r.runId,
    approvals: JSON.parse(r.approvals) as string[],
    reason: r.statusReason,
  };
}

export function toRun(r: RunRow): Run {
  return {
    id: r.id,
    sessionId: r.sessionId,
    seq: r.seq,
    trigger: r.trigger as RunTrigger,
    status: r.status as RunStatus,
    startedAt: r.startedAt,
    ...opt("endedAt", r.endedAt),
    ...opt("headAtStart", r.headAtStart),
    ...opt("stopReason", r.stopReason),
    usage: JSON.parse(r.usage) as Usage,
  };
}

export function toNode(r: NodeRow): Node {
  return {
    id: r.id,
    runId: r.runId,
    sessionId: r.sessionId,
    seq: r.seq,
    kind: r.kind as NodeKind,
    ...opt("providerId", r.providerId),
    ...opt("callId", r.callId),
    ...(r.payload === null ? {} : { payload: JSON.parse(r.payload) as unknown }),
    ...opt("blobRef", r.blobRef),
    createdAt: r.createdAt,
  };
}

export function toApproval(r: ApprovalRow): Approval {
  return {
    id: r.id,
    sessionId: r.sessionId,
    kind: r.kind as Approval["kind"],
    status: r.status as Approval["status"],
    subject: JSON.parse(r.subject) as unknown,
    argsHash: r.argsHash,
    requestedAt: r.requestedAt,
    expiresAt: r.expiresAt,
    ...opt("decision", r.decision as Approval["decision"] | null),
    ...opt("by", r.by),
    ...opt("answeredAt", r.answeredAt),
    ...opt("consumedAt", r.consumedAt),
  };
}

/** What `recoverAfterCrash` changed. */
export interface Recovery {
  /** Sessions that were `starting`, `running` or `waitingApproval`, now `interrupted`. */
  readonly interrupted: string[];
  /** Synthetic `SESSION_INTERRUPTED` results written for calls left without one. */
  readonly closedCalls: number;
}

/** What `appendNode` did: `inserted` is false when the call already had that node (a late result). */
export interface Appended {
  readonly node: Node;
  readonly inserted: boolean;
}

export interface SessionQuery {
  readonly projectId?: string;
  /** Only these projects (the caller's reach); an empty list matches nothing. */
  readonly projects?: ReadonlyArray<string>;
  readonly status?: SessionStatus;
  /** A page's `next`: the last id it held. */
  readonly cursor?: string;
  readonly limit?: number;
}

export interface NewApproval {
  readonly sessionId: string;
  readonly kind: ApprovalKind;
  readonly subject: unknown;
  readonly argsHash: string;
  readonly expiresAt: number;
}

export const SYNTHETIC_RESULT = (reason: string | null) => ({
  ok: false,
  code: "SESSION_INTERRUPTED",
  message: "the session was interrupted before the tool call finished",
  reason,
  synthetic: true,
});

/** `a + b`, field by field; the basis is the latest one given, `estimated` sticks once set. */
export function addUsage(a: Usage, b: Usage): Usage {
  const sum: Usage = {
    input: a.input + b.input,
    output: a.output + b.output,
    cacheRead: a.cacheRead + b.cacheRead,
    cacheWrite: a.cacheWrite + b.cacheWrite,
    reasoning: a.reasoning + b.reasoning,
    moneyMicros: a.moneyMicros + b.moneyMicros,
  };
  const basis = b.costBasis ?? a.costBasis;
  const estimated = a.estimated === true || b.estimated === true;
  return { ...sum, ...(basis === undefined ? {} : { costBasis: basis }), ...(estimated ? { estimated: true } : {}) };
}

export function sessionNotFound(sessionId: string): never {
  throw finError("SESSION_NOT_FOUND", "no such agent session", { sessionId }, "list sessions with `finisherd agent ls`");
}

export function invalid(message: string, details: Record<string, unknown>): never {
  throw finError("INVALID_REQUEST", message, details);
}

/** The outbox effects of a transition's effects, in order. */
export function outboxOf(effects: ReadonlyArray<SessionEffect>): OutboxEffect[] {
  const out: OutboxEffect[] = [];
  for (const e of effects) {
    if (e.kind === "journal") out.push({ kind: "journal", phase: e.phase, reason: e.reason });
    else if (e.kind === "revokeCredential" || e.kind === "refreshCredential") out.push({ kind: e.kind });
  }
  return out;
}

/** `insertSession`'s parameters for a new session, in column order. */
export function sessionInsertArgs(s: NewSession, status: string, now: number): Array<string | number | null> {
  return [
    s.id, s.projectId, s.harness, s.provider, s.model, s.weights, s.role, s.mode, s.rootBranch ?? null, s.base.branch, s.base.seq,
    s.parent ?? null, status, s.id, s.providerRef ?? null, JSON.stringify(s.budget), JSON.stringify(ZERO_USAGE), JSON.stringify(s.createdBy),
    now, s.title ?? null,
  ];
}

/** `AgentStore.list`'s statement for `q` (newest first, one row more than a page). */
export function sessionListQuery(q: SessionQuery): { sql: string; args: string[]; limit: number } {
  const limit = Math.min(Math.max(Math.trunc(q.limit ?? 50), 1), 200);
  const where: string[] = [];
  const args: string[] = [];
  const add = (clause: string, arg: string): void => {
    where.push(clause);
    args.push(arg);
  };
  if (q.projectId !== undefined) add("projectId = ?", q.projectId);
  if (q.projects !== undefined) add("projectId IN (SELECT value FROM json_each(?))", JSON.stringify(q.projects));
  if (q.status !== undefined) add("status = ?", q.status);
  if (q.cursor !== undefined) add("id < ?", q.cursor);
  const sql = `SELECT ${SESSION_COLUMNS} FROM sessions ${where.length > 0 ? `WHERE ${where.join(" AND ")}` : ""} ORDER BY id DESC LIMIT ?`;
  return { sql, args, limit };
}

/** An approval answered after its `expiresAt` (exit 4): it was expired instead. */
export function approvalExpired(id: string): FinError {
  return finError("APPROVAL_EXPIRED", "the approval expired before it was answered", { id }, "Ask for it again: repeat the request, or resume the session.");
}

/** Every statement `AgentStore` prepares, once, by name. */
export const STORE_SQL: Readonly<Record<string, string>> = {
  insertSession: `INSERT INTO sessions (${SESSION_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, '[]', ?, ?, ?, ?, ?, ?, NULL, ?)`,
  session: `SELECT ${SESSION_COLUMNS} FROM sessions WHERE id = ?`,
  setState: "UPDATE sessions SET status = ?, statusReason = ?, runId = ?, approvals = ?, endedAt = COALESCE(endedAt, ?) WHERE id = ?",
  live: "SELECT id FROM sessions WHERE status IN ('starting', 'running', 'waitingApproval') ORDER BY id",
  providerRef: "UPDATE sessions SET providerRef = ? WHERE id = ?",
  sessionUsage: "UPDATE sessions SET usage = ? WHERE id = ?",
  expired: "SELECT id FROM sessions WHERE endedAt IS NOT NULL AND endedAt < ? ORDER BY endedAt LIMIT ?",
  exists: "SELECT 1 AS x FROM sessions WHERE id = ?",
  lastRunSeq: "SELECT seq FROM runs WHERE sessionId = ? ORDER BY seq DESC LIMIT 1",
  insertRun: `INSERT INTO runs (${RUN_COLUMNS}) VALUES (?, ?, ?, ?, 'running', ?, NULL, ?, NULL, ?)`,
  run: `SELECT ${RUN_COLUMNS} FROM runs WHERE id = ?`,
  runs: `SELECT ${RUN_COLUMNS} FROM runs WHERE sessionId = ? ORDER BY seq`,
  closeRun: "UPDATE runs SET status = ?, endedAt = ?, stopReason = ? WHERE id = ? AND status = 'running'",
  openRuns: `SELECT ${RUN_COLUMNS} FROM runs WHERE status = 'running'`,
  runUsage: "UPDATE runs SET usage = ? WHERE id = ?",
  lastNodeSeq: "SELECT seq FROM nodes WHERE sessionId = ? ORDER BY seq DESC LIMIT 1",
  insertNode: `INSERT INTO nodes (${NODE_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  nodes: `SELECT ${NODE_COLUMNS} FROM nodes WHERE sessionId = ? AND seq >= ? ORDER BY seq LIMIT ?`,
  nodeById: `SELECT ${NODE_COLUMNS} FROM nodes WHERE id = ?`,
  // The repeated `kind IN (…)` is `nodes_call`'s own predicate: without it the partial index is not used.
  callNode: `SELECT ${NODE_COLUMNS} FROM nodes WHERE callId = ? AND kind = ? AND kind IN ('toolCall', 'toolResult')`,
  openCalls: `SELECT ${NODE_COLUMNS} FROM nodes c WHERE c.runId = ? AND c.kind = 'toolCall' AND NOT EXISTS ` +
      "(SELECT 1 FROM nodes r WHERE r.callId = c.callId AND r.kind = 'toolResult' AND r.kind IN ('toolCall', 'toolResult')) ORDER BY c.seq",
  blobRefs: "SELECT DISTINCT blobRef FROM nodes WHERE sessionId = ? AND blobRef IS NOT NULL",
  blobUsed: "SELECT 1 AS x FROM nodes WHERE sessionId = ? AND blobRef = ? LIMIT 1",
  insertApproval: `INSERT INTO approvals (${APPROVAL_COLUMNS}) VALUES (?, ?, ?, 'open', ?, ?, ?, ?, NULL, NULL, NULL, NULL)`,
  approval: `SELECT ${APPROVAL_COLUMNS} FROM approvals WHERE id = ?`,
  approvalsOf: `SELECT ${APPROVAL_COLUMNS} FROM approvals WHERE sessionId = ? ORDER BY requestedAt, id`,
  expireApproval: "UPDATE approvals SET status = 'expired' WHERE id = ? AND status = 'open'",
  overdueApprovals: "SELECT id, sessionId FROM approvals WHERE status = 'open' AND expiresAt <= ? ORDER BY expiresAt, id LIMIT ?",
  markReady: "UPDATE sessions SET readyAt = ? WHERE id = ? AND readyAt IS NULL",
  readyAt: "SELECT readyAt FROM sessions WHERE id = ?",
  clearReserved: "UPDATE budgets SET reserved = 0 WHERE reserved != 0",
  clearDailyReserved: "UPDATE budget_days SET reserved = 0 WHERE reserved != 0",
  answerApproval: "UPDATE approvals SET status = 'answered', decision = ?, by = ?, answeredAt = ? WHERE id = ? AND status = 'open'",
  consumeApproval: "UPDATE approvals SET consumedAt = ? WHERE id = ? AND status = 'answered' AND consumedAt IS NULL",
  outboxAdd: "INSERT INTO session_outbox (sessionId, effect, at) VALUES (?, ?, ?)",
  outboxAll: "SELECT id, sessionId, effect, at FROM session_outbox ORDER BY id",
  outboxOf: "SELECT id, sessionId, effect, at FROM session_outbox WHERE sessionId = ? ORDER BY id",
  outboxDone: "DELETE FROM session_outbox WHERE id = ?",
};
