// The pure agent session state machine (plan 7A Task 3a, Decision 2).
import fc from "fast-check";
import { describe, expect, it } from "vitest";
import {
  initialState,
  next,
  SESSION_STATUSES,
  type SessionEvent,
  type SessionState,
  wellFormed,
} from "../src/agent/session.ts";

const ids = fc.constantFrom("r1", "r2", "r3");
const approvalIds = fc.constantFrom("a1", "a2");
const reasons = fc.constantFrom("serverRestart", "BUDGET_EXCEEDED", "PROVIDER_EXITED", "user");

const event: fc.Arbitrary<SessionEvent> = fc.oneof(
  fc.constant({ kind: "ready" as const }),
  ids.map((runId) => ({ kind: "runStarted" as const, runId })),
  fc.record({ runId: ids, stop: fc.constantFrom("endTurn", "cap") }).map((r) => ({ kind: "runEnded" as const, ...r })),
  approvalIds.map((id) => ({ kind: "approvalOpened" as const, id })),
  approvalIds.map((id) => ({ kind: "approvalAnswered" as const, id })),
  reasons.map((reason) => ({ kind: "interrupted" as const, reason })),
  fc.constant({ kind: "resumed" as const }),
  reasons.map((reason) => ({ kind: "ended" as const, reason })),
);

/** Runs `events` from the initial state, keeping the state on an error (the request is refused). */
function run(events: SessionEvent[]): { states: SessionState[]; errors: string[] } {
  let s = initialState;
  const states = [s];
  const errors: string[] = [];
  for (const e of events) {
    const t = next(s, e);
    if ("error" in t) errors.push(t.error.code);
    else s = t.state;
    states.push(s);
  }
  return { states, errors };
}

/** Every state reachable by a random sequence (the arbitrary the per-state properties use). */
const reachable = fc.array(event, { maxLength: 30 }).map((es) => run(es).states.at(-1)!);

function step(s: SessionState, ...events: SessionEvent[]): SessionState {
  for (const e of events) {
    const t = next(s, e);
    if ("error" in t) throw new Error(`${e.kind} from ${s.status}: ${t.error.code}`);
    s = t.state;
  }
  return s;
}

describe("state_machine_properties", () => {
  it("random event sequences never reach an undefined or inconsistent state", () => {
    fc.assert(
      fc.property(fc.array(event, { maxLength: 60 }), (es) => {
        for (const s of run(es).states) {
          expect(SESSION_STATUSES).toContain(s.status);
          expect(wellFormed(s)).toBe(true);
        }
      }),
      { numRuns: 2000 },
    );
  });

  it("ended absorbs: every event after it is SESSION_ENDED and changes nothing", () => {
    fc.assert(
      fc.property(reachable, reasons, event, (s, reason, e) => {
        const t = next(s, { kind: "ended", reason });
        if ("error" in t) {
          expect(s.status).toBe("ended");
          expect(t.error.code).toBe("SESSION_ENDED");
        } else {
          expect(t.state.status).toBe("ended");
          const after = next(t.state, e);
          expect("error" in after && after.error.code).toBe("SESSION_ENDED");
        }
      }),
    );
  });

  it("a prompt while running or waiting is SESSION_BUSY", () => {
    fc.assert(
      fc.property(reachable, ids, (s, runId) => {
        const t = next(s, { kind: "runStarted", runId });
        if (s.status === "running" || s.status === "waitingApproval" || s.status === "starting") {
          expect("error" in t && t.error.code).toBe("SESSION_BUSY");
        } else if (s.status === "interrupted") {
          expect("error" in t && t.error.code).toBe("SESSION_INTERRUPTED");
        } else if (s.status === "idle") {
          expect("state" in t && t.state.status).toBe("running");
        }
      }),
    );
  });

  it("resume transitions only from interrupted", () => {
    fc.assert(
      fc.property(reachable, (s) => {
        const t = next(s, { kind: "resumed" });
        if ("error" in t) {
          expect(["running", "waitingApproval", "starting", "ended"]).toContain(s.status);
        } else if (s.status === "interrupted") {
          expect(t.state.status).toBe("idle");
          expect(t.effects.map((x) => x.kind)).toContain("refreshCredential");
        } else {
          expect(t.state).toEqual(s);
          expect(t.effects).toEqual([]);
        }
      }),
    );
  });

  it("every effect list is a function of the transition alone", () => {
    // Two states with the same status and run, whatever else differs, give the same effects
    // and the same next status for the same event.
    const key = (s: SessionState) => `${s.status}/${s.runId}/${s.approvals.join(",")}`;
    fc.assert(
      fc.property(fc.array(reachable, { minLength: 2, maxLength: 8 }), event, (states, e) => {
        const seen = new Map<string, string>();
        for (const s of states) {
          const t = next(s, e);
          const out = "error" in t ? `error:${t.error.code}` : `${t.state.status}:${JSON.stringify(t.effects)}`;
          const k = key(s);
          if (seen.has(k)) expect(out).toBe(seen.get(k));
          seen.set(k, out);
        }
      }),
    );
  });

  it("an error never changes the state", () => {
    fc.assert(
      fc.property(reachable, event, (s, e) => {
        const before = JSON.stringify(s);
        const t = next(s, e);
        expect(JSON.stringify(s)).toBe(before);
        if ("error" in t) expect(t.error.message.length).toBeGreaterThan(0);
      }),
    );
  });
});

describe("transitions", () => {
  it("a run's life: start, approval, answer, end", () => {
    let s = step(initialState, { kind: "ready" });
    expect(s.status).toBe("idle");
    const started = next(s, { kind: "runStarted", runId: "r1" });
    expect("state" in started && started.effects).toEqual([{ kind: "openRun", runId: "r1" }]);
    s = step(s, { kind: "runStarted", runId: "r1" }, { kind: "approvalOpened", id: "a1" });
    expect(s).toMatchObject({ status: "waitingApproval", approvals: ["a1"] });
    s = step(s, { kind: "approvalAnswered", id: "a1" });
    expect(s.status).toBe("running");
    // A stale end of another run changes nothing.
    expect(step(s, { kind: "runEnded", runId: "r9", stop: "endTurn" })).toEqual(s);
    const ended = next(s, { kind: "runEnded", runId: "r1", stop: "endTurn" });
    expect(ended).toEqual({
      state: { status: "idle", runId: null, approvals: [], reason: null },
      effects: [{ kind: "closeRun", runId: "r1", status: "completed", stop: "endTurn" }],
    });
  });

  it("an interrupt mid-run closes the run and its open tool calls, keeps nothing open", () => {
    const s = step(initialState, { kind: "ready" }, { kind: "runStarted", runId: "r1" }, { kind: "approvalOpened", id: "a1" });
    const t = next(s, { kind: "interrupted", reason: "serverRestart" });
    expect(t).toEqual({
      state: { status: "interrupted", runId: null, approvals: [], reason: "serverRestart" },
      effects: [
        { kind: "closeOpenToolCalls", runId: "r1" },
        { kind: "closeRun", runId: "r1", status: "interrupted", stop: "serverRestart" },
        { kind: "expireApprovals", ids: ["a1"] },
        { kind: "stopProvider" },
        { kind: "journal", phase: "interrupted", reason: "serverRestart" },
      ],
    });
  });

  it("a budget stop: interrupted, a budget approval, answered, resumed", () => {
    let s = step(initialState, { kind: "ready" }, { kind: "runStarted", runId: "r1" });
    s = step(s, { kind: "interrupted", reason: "BUDGET_EXCEEDED" }, { kind: "approvalOpened", id: "b1" });
    expect(s).toMatchObject({ status: "interrupted", approvals: ["b1"], reason: "BUDGET_EXCEEDED" });
    s = step(s, { kind: "approvalAnswered", id: "b1" });
    const t = next(s, { kind: "resumed" });
    expect(t).toEqual({
      state: { status: "idle", runId: null, approvals: [], reason: null },
      effects: [{ kind: "refreshCredential" }, { kind: "journal", phase: "resumed", reason: null }],
    });
  });

  it("resuming past an unanswered approval expires it", () => {
    const s = step(
      initialState,
      { kind: "ready" },
      { kind: "runStarted", runId: "r1" },
      { kind: "interrupted", reason: "BUDGET_EXCEEDED" },
      { kind: "approvalOpened", id: "b1" },
    );
    const t = next(s, { kind: "resumed" });
    expect("state" in t && t.effects[0]).toEqual({ kind: "expireApprovals", ids: ["b1"] });
  });

  it("ending revokes the credential; starting is journalled once", () => {
    const t = next(initialState, { kind: "ready" });
    expect("state" in t && t.effects).toEqual([{ kind: "journal", phase: "started", reason: null }]);
    const s = step(initialState, { kind: "ready" });
    expect(next(s, { kind: "ready" })).toEqual({ state: s, effects: [] });
    const end = next(s, { kind: "ended", reason: "user" });
    expect("state" in end && end.effects.map((e) => e.kind)).toEqual(["stopProvider", "revokeCredential", "journal"]);
  });

  it("a crash while starting interrupts the session", () => {
    const t = next(initialState, { kind: "interrupted", reason: "serverRestart" });
    expect("state" in t && t.state.status).toBe("interrupted");
  });
});
