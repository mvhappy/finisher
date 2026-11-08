// The agent session state machine (sub-project 7 §3.2; plan 7A Decision 2): a pure function
// from a state and an event to the next state and the effects the caller must apply. No I/O:
// the store (7A-2) applies the effects and the new state in one `ControlStore.tx`.
//
//   starting → idle ⇄ running → ended; running ⇄ waitingApproval;
//   any live state → interrupted (provider exit, server restart, budget stop);
//   interrupted → idle by resume; ended is final and absorbs every event.
import { FinError, type SessionStatus } from "@finisher/contracts";

export type { SessionStatus };

export const SESSION_STATUSES: readonly SessionStatus[] = [
  "starting",
  "idle",
  "running",
  "waitingApproval",
  "interrupted",
  "ended",
];

/** The states a session can be live in: everything but `ended`. */
export const LIVE_STATUSES: readonly SessionStatus[] = SESSION_STATUSES.filter((s) => s !== "ended");

export interface SessionState {
  readonly status: SessionStatus;
  /** The active run (`running`, `waitingApproval`), else null. */
  readonly runId: string | null;
  /**
   * Approvals opened and not yet answered: those of the active run (`waitingApproval`), or a
   * budget approval opened for an `interrupted` session. Empty in every other status.
   */
  readonly approvals: readonly string[];
  /** The code that interrupted or ended the session, else null. */
  readonly reason: string | null;
}

export type SessionEvent =
  | { kind: "ready" }
  | { kind: "runStarted"; runId: string }
  | { kind: "runEnded"; runId: string; stop: string }
  | { kind: "approvalOpened"; id: string }
  | { kind: "approvalAnswered"; id: string }
  /** An open approval passed its `expiresAt` (the hourly sweep, or an answer that came too late). */
  | { kind: "approvalExpired"; id: string }
  | { kind: "interrupted"; reason: string }
  | { kind: "resumed" }
  | { kind: "ended"; reason: string };

/** The `agent.session` phases the journal records. */
export type JournalPhase = "started" | "resumed" | "interrupted" | "ended";

/** What the caller does besides storing the new state, in this order. */
export type SessionEffect =
  | { kind: "openRun"; runId: string }
  | { kind: "closeRun"; runId: string; status: "completed" | "interrupted"; stop: string }
  /** Give every announced tool call of the run without a result a synthetic `SESSION_INTERRUPTED` one. */
  | { kind: "closeOpenToolCalls"; runId: string }
  /** Mark these approvals `expired` (their run is gone, or the session moved on). */
  | { kind: "expireApprovals"; ids: string[] }
  | { kind: "stopProvider" }
  | { kind: "refreshCredential" }
  | { kind: "revokeCredential" }
  | { kind: "journal"; phase: JournalPhase; reason: string | null };

export type Transition = { state: SessionState; effects: SessionEffect[] } | { error: FinError };

export const initialState: SessionState = { status: "starting", runId: null, approvals: [], reason: null };

function fail(code: string, message: string, s: SessionState, fix?: string): Transition {
  return {
    error: new FinError({ code, message, details: { status: s.status }, ...(fix === undefined ? {} : { fix }) }),
  };
}

const ended = (s: SessionState) =>
  fail("SESSION_ENDED", "the session has ended", s, "start a new session");

const busy = (s: SessionState) =>
  fail("SESSION_BUSY", `the session is ${s.status}`, s, "wait for the run to end, or steer it");

const unchanged = (s: SessionState): Transition => ({ state: s, effects: [] });

/** Closes the active run, if any, and expires the open approvals. */
function closeAll(s: SessionState, reason: string): SessionEffect[] {
  const effects: SessionEffect[] = [];
  if (s.runId !== null) {
    effects.push({ kind: "closeOpenToolCalls", runId: s.runId });
    effects.push({ kind: "closeRun", runId: s.runId, status: "interrupted", stop: reason });
  }
  if (s.approvals.length > 0) effects.push({ kind: "expireApprovals", ids: [...s.approvals] });
  return effects;
}

function interrupt(s: SessionState, reason: string): Transition {
  const effects = closeAll(s, reason);
  effects.push({ kind: "stopProvider" }, { kind: "journal", phase: "interrupted", reason });
  return { state: { status: "interrupted", runId: null, approvals: [], reason }, effects };
}

function end(s: SessionState, reason: string): Transition {
  const effects = closeAll(s, reason);
  effects.push(
    { kind: "stopProvider" },
    { kind: "revokeCredential" },
    { kind: "journal", phase: "ended", reason },
  );
  return { state: { status: "ended", runId: null, approvals: [], reason }, effects };
}

/** The next state of `s` after `e`, or the error the request that caused `e` answers. */
export function next(s: SessionState, e: SessionEvent): Transition {
  if (s.status === "ended") return ended(s);
  switch (e.kind) {
    case "ready":
      if (s.status !== "starting") return unchanged(s);
      return { state: { ...s, status: "idle" }, effects: [{ kind: "journal", phase: "started", reason: null }] };

    case "runStarted":
      switch (s.status) {
        case "idle":
          return { state: { ...s, status: "running", runId: e.runId }, effects: [{ kind: "openRun", runId: e.runId }] };
        case "interrupted":
          return fail("SESSION_INTERRUPTED", "the session is interrupted", s, "resume it first");
        default:
          return busy(s);
      }

    case "runEnded":
      // A late or stale end (another run's, or after an interrupt) changes nothing.
      if (s.runId !== e.runId) return unchanged(s);
      return {
        state: { ...s, status: "idle", runId: null, approvals: [] },
        effects: [
          { kind: "closeRun", runId: e.runId, status: "completed", stop: e.stop },
          // A run that ends while it waits takes its approvals with it.
          ...(s.approvals.length > 0 ? [{ kind: "expireApprovals" as const, ids: [...s.approvals] }] : []),
        ],
      };

    case "approvalOpened": {
      // An approval opens during a run, or (a budget approval) for an interrupted session.
      // Anywhere else nothing can wait on it: it is expired at once, never dropped silently.
      const open = s.status === "running" || s.status === "waitingApproval" || s.status === "interrupted";
      if (!open) return { state: s, effects: [{ kind: "expireApprovals", ids: [e.id] }] };
      if (s.approvals.includes(e.id)) return unchanged(s);
      const status = s.status === "interrupted" ? "interrupted" : "waitingApproval";
      return { state: { ...s, status, approvals: [...s.approvals, e.id] }, effects: [] };
    }

    case "approvalAnswered":
    case "approvalExpired": {
      // An expiry is an answer no one gave: the approval leaves the open list either way.
      const expire: SessionEffect[] = e.kind === "approvalExpired" ? [{ kind: "expireApprovals", ids: [e.id] }] : [];
      if (!s.approvals.includes(e.id)) return { state: s, effects: expire };
      const approvals = s.approvals.filter((a) => a !== e.id);
      const status = s.status === "waitingApproval" && approvals.length === 0 ? "running" : s.status;
      return { state: { ...s, status, approvals }, effects: expire };
    }

    case "interrupted":
      if (s.status === "interrupted") return unchanged(s);
      return interrupt(s, e.reason);

    case "resumed":
      switch (s.status) {
        case "interrupted": {
          // Resuming moves past any approval still open for the interruption.
          const effects: SessionEffect[] =
            s.approvals.length > 0 ? [{ kind: "expireApprovals", ids: [...s.approvals] }] : [];
          effects.push({ kind: "refreshCredential" }, { kind: "journal", phase: "resumed", reason: null });
          return { state: { status: "idle", runId: null, approvals: [], reason: null }, effects };
        }
        case "idle":
          return unchanged(s);
        default:
          return busy(s);
      }

    case "ended":
      return end(s, e.reason);
  }
}

/** Whether `s` is internally consistent: the invariants every reachable state keeps. */
export function wellFormed(s: SessionState): boolean {
  if (!SESSION_STATUSES.includes(s.status)) return false;
  const hasRun = s.status === "running" || s.status === "waitingApproval";
  if (hasRun !== (s.runId !== null)) return false;
  if (s.status === "waitingApproval" && s.approvals.length === 0) return false;
  if (s.approvals.length > 0 && s.status !== "waitingApproval" && s.status !== "interrupted") return false;
  if ((s.status === "interrupted" || s.status === "ended") !== (s.reason !== null)) return false;
  if (new Set(s.approvals).size !== s.approvals.length) return false;
  return true;
}
