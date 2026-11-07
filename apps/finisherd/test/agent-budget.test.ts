// Session budgets (plan 7A Task 4, Decisions 10, 20–21; review H3, B2, L4).
import { describe, expect, it } from "vitest";
import { Budget, type BudgetLimits, DEFAULT_BUDGET } from "../src/agent/budget.ts";
import { type Usage, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock } from "../src/agent/clock.ts";
import { MemoryLedger, utcDay } from "../src/agent/ledger.ts";

const usage = (u: Partial<Usage>): Usage => ({ ...ZERO_USAGE, ...u });

const limits = (l: Partial<BudgetLimits>): BudgetLimits => ({ ...DEFAULT_BUDGET, ...l });

describe("budget", () => {
  const price = (u: Usage) => u.input + u.output * 10; // 1 micro per input token, 10 per output

  it("stops before a call that cannot fit, by projection", () => {
    const clock = manualClock();
    const b = new Budget(new MemoryLedger(), "s1", limits({ tokens: 1000, moneyMicros: 5000 }), clock, { price });
    b.startRun();
    const first = b.canCall(usage({ input: 100, output: 400 }));
    expect(first).toEqual({ ok: true, reservation: { micros: 4100, day: utcDay(clock.now()) } });
    b.release(first.ok ? first.reservation : null);
    expect(b.canCall(usage({ input: 100, output: 901 }))).toMatchObject({ ok: false, kind: "tokens", used: 0, limit: 1000 });
    b.charge(usage({ input: 100, output: 100 }), 1100);
    expect(b.canCall(usage({ input: 10, output: 390 }))).toMatchObject({ ok: false, kind: "money", used: 1100, limit: 5000 });
    expect(b.canCall(usage({ input: 10, output: 300 }))).toMatchObject({ ok: true });
  });

  it("notices at 80 % once per kind, stops at 100 %, and raise lets the run go on", () => {
    const ledger = new MemoryLedger();
    const b = new Budget(ledger, "s1", limits({ tokens: 1000 }), manualClock(), { price });
    b.startRun();
    expect(b.charge(usage({ input: 700 }), 0)).toEqual({ notices: [], stop: null });
    const at80 = b.charge(usage({ input: 100 }), 0);
    expect(at80.notices).toEqual([{ kind: "tokens", used: 800, limit: 1000 }]);
    expect(at80.stop).toBeNull();
    expect(b.charge(usage({ input: 100 }), 0).notices).toEqual([]);
    const at100 = b.charge(usage({ input: 100 }), 0);
    expect(at100.stop).toEqual({ kind: "tokens", used: 1000, limit: 1000 });
    expect(b.canCall(usage({ input: 1 }))).toMatchObject({ ok: false, kind: "tokens" });
    b.raise("tokens", 1000);
    expect(ledger.read("s1", "tokens").limit).toBe(2000);
    expect(b.canCall(usage({ input: 1 }))).toMatchObject({ ok: true });
    expect(b.check()).toEqual({ notices: [], stop: null });
    // The notice may come again after a raise.
    expect(b.charge(usage({ input: 600 }), 0).notices.map((n) => n.kind)).toEqual(["tokens"]);
  });

  it("counts cache reads at their price ratio", () => {
    const b = new Budget(new MemoryLedger(), "s1", limits({}), manualClock(), { cacheReadWeight: 0.1 });
    expect(b.tokensOf(usage({ input: 10, output: 5, cacheRead: 1000, cacheWrite: 7, reasoning: 3 }))).toBe(10 + 5 + 7 + 100);
  });

  it("caps money per server per UTC day across sessions, and the day rolls over", () => {
    const clock = manualClock(Date.UTC(2026, 10, 7, 23, 0, 0));
    const ledger = new MemoryLedger();
    const l = limits({ dailyMoneyMicros: 1000 });
    const a = new Budget(ledger, "a", l, clock, { price });
    const b = new Budget(ledger, "b", l, clock, { price });
    a.startRun();
    b.startRun();
    a.charge(ZERO_USAGE, 600);
    expect(b.canCall(usage({ input: 500 }))).toMatchObject({ ok: false, kind: "dailyMoney", used: 600, limit: 1000 });
    expect(b.charge(ZERO_USAGE, 400).stop).toEqual({ kind: "dailyMoney", used: 1000, limit: 1000 });
    expect(ledger.daily("2026-11-07")).toEqual({ used: 1000, reserved: 0 });
    b.endRun();
    clock.advance(60 * 60_000); // past midnight UTC, between runs
    expect(utcDay(clock.now())).toBe("2026-11-08");
    b.startRun();
    expect(b.canCall(usage({ input: 500 }))).toMatchObject({ ok: true });
  });

  it("counts wall clock only while a run is active", () => {
    const clock = manualClock();
    const ledger = new MemoryLedger();
    const b = new Budget(ledger, "s1", limits({ wallMs: 10_000 }), clock);
    clock.advance(60_000); // idle: not counted
    b.startRun();
    clock.advance(4_000);
    b.endRun();
    clock.advance(60_000);
    expect(ledger.read("s1", "wallMs").used).toBe(4_000);
    b.startRun();
    clock.advance(4_000);
    expect(b.check().notices.map((n) => n.kind)).toEqual(["wallMs"]);
    clock.advance(2_000);
    expect(b.check().stop).toEqual({ kind: "wallMs", used: 10_000, limit: 10_000 });
    expect(b.canCall(ZERO_USAGE)).toMatchObject({ ok: false, kind: "wallMs" });
  });

  it("caps model and tool calls per run, without stopping the session", () => {
    const b = new Budget(new MemoryLedger(), "s1", limits({ modelCallsPerRun: 2, toolCallsPerRun: 1 }), manualClock());
    b.startRun();
    b.charge(ZERO_USAGE, 0);
    const second = b.charge(ZERO_USAGE, 0);
    expect(second.stop).toBeNull();
    expect(second.notices.map((n) => n.kind)).toContain("modelCalls");
    expect(b.canCall(ZERO_USAGE)).toMatchObject({ ok: false, kind: "modelCalls", used: 2, limit: 2 });
    expect(b.canTool()).toMatchObject({ ok: true });
    expect(b.chargeTool().stop).toBeNull();
    expect(b.canTool()).toMatchObject({ ok: false, kind: "toolCalls" });
    b.startRun();
    expect(b.canCall(ZERO_USAGE)).toMatchObject({ ok: true });
    expect(b.canTool()).toMatchObject({ ok: true });
  });

  it("keeps a limit already in the ledger (a resumed session) and starts from the defaults otherwise", () => {
    const ledger = new MemoryLedger();
    ledger.setLimit("s1", "money", 42);
    new Budget(ledger, "s1", DEFAULT_BUDGET, manualClock());
    expect(ledger.read("s1", "money").limit).toBe(42);
    expect(ledger.read("s1", "tokens").limit).toBe(2_000_000);
    expect(ledger.read("s1", "wallMs").limit).toBe(3_600_000);
  });
});

describe("reservations and attempts (review H3, B2)", () => {
  const price = (u: Usage) => u.input + u.output * 10;

  it("concurrent calls hold their projected money, so they cannot overspend together", () => {
    const clock = manualClock();
    const ledger = new MemoryLedger();
    const l = limits({ dailyMoneyMicros: 1000, moneyMicros: 800 });
    const a = new Budget(ledger, "a", l, clock, { price });
    const b = new Budget(ledger, "b", l, clock, { price });
    const c = new Budget(ledger, "c", l, clock, { price });
    // Three sessions ask at once, before any of them is charged.
    const ra = a.canCall(usage({ output: 40 }));
    const rb = b.canCall(usage({ output: 40 }));
    const rc = c.canCall(usage({ output: 40 }));
    expect(ra).toMatchObject({ ok: true, reservation: { micros: 400 } });
    expect(rb).toMatchObject({ ok: true, reservation: { micros: 400 } });
    expect(rc).toMatchObject({ ok: false, kind: "dailyMoney", used: 800, limit: 1000 });
    expect(ledger.daily(utcDay(clock.now()))).toEqual({ used: 0, reserved: 800 });
    // A settles cheaper than projected: the difference is free again.
    a.charge(usage({ output: 10 }), 100, { reservation: ra.ok ? ra.reservation : null });
    expect(ledger.daily(utcDay(clock.now()))).toEqual({ used: 100, reserved: 400 });
    expect(ledger.read("a", "money")).toMatchObject({ used: 100, reserved: 0 });
    expect(c.canCall(usage({ output: 40 }))).toMatchObject({ ok: true });
    // Within one session the session's own limit counts what it holds.
    const again = b.canCall(usage({ output: 41 }));
    expect(again).toMatchObject({ ok: false, kind: "money", used: 400, limit: 800 });
    b.release(rb.ok ? rb.reservation : null);
    expect(ledger.read("b", "money").reserved).toBe(0);
  });

  it("an unknown price reserves nothing: the call is charged after it", () => {
    const ledger = new MemoryLedger();
    const b = new Budget(ledger, "s", limits({ moneyMicros: 10 }), manualClock(), { price: () => null });
    expect(b.canCall(usage({ output: 1_000_000 }))).toEqual({ ok: true, reservation: null });
    expect(b.charge(usage({ output: 5 }), 50).stop).toEqual({ kind: "money", used: 50, limit: 10 });
  });

  it("a failed attempt is charged without counting a model call", () => {
    const b = new Budget(new MemoryLedger(), "s", limits({ modelCallsPerRun: 1 }), manualClock());
    b.startRun();
    b.charge(usage({ input: 10, output: 3 }), 7, { call: false });
    expect(b.canCall(ZERO_USAGE)).toMatchObject({ ok: true });
    b.charge(usage({ input: 10, output: 3 }), 7);
    expect(b.canCall(ZERO_USAGE)).toMatchObject({ ok: false, kind: "modelCalls" });
    expect(b.readings().find((r) => r.kind === "money")?.used).toBe(14);
  });
});
