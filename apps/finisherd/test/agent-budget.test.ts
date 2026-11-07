// The model catalogue and session budgets (plan 7A Task 4, Decisions 10, 19–21).
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { Budget, type BudgetLimits, DEFAULT_BUDGET } from "../src/agent/budget.ts";
import { BUNDLED_CATALOGUE, Catalogue, type Usage, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { manualClock } from "../src/agent/clock.ts";
import { MemoryLedger, utcDay } from "../src/agent/ledger.ts";

const FIXTURE = fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url));
const usage = (u: Partial<Usage>): Usage => ({ ...ZERO_USAGE, ...u });

describe("catalogue", () => {
  const cat = Catalogue.load(FIXTURE, [
    { id: "acme/new-model", window: 64000 },
    { id: "acme/open-one", window: 64000, weights: "open" },
    { id: "llama3.3:70b", provider: "local", window: 128000 },
  ]);

  it("prices input, output, cache reads and cache writes, rounded up", () => {
    // 1000 in × 3 + 200 out × 15 + 5000 cache read × 0.3 + 100 cache write × 3.75 (USD per MTok)
    const u = usage({ input: 1000, output: 200, cacheRead: 5000, cacheWrite: 100, reasoning: 50 });
    expect(cat.price("test/closed-explicit", u)).toEqual({ micros: 3000 + 3000 + 1500 + 375, basis: "priced" });
    expect(cat.price("test/closed-explicit", usage({ input: 1 })).micros).toBe(3);
    expect(cat.price("test/closed-explicit", usage({ cacheRead: 1 })).micros).toBe(1);
  });

  it("is reported when the provider gave a cost, priced from the catalogue otherwise, else unknown", () => {
    const u = usage({ input: 1000 });
    expect(cat.price("test/closed-explicit", u, 1234.2)).toEqual({ micros: 1235, basis: "reported" });
    expect(cat.price("test/closed-explicit", u)).toEqual({ micros: 3000, basis: "priced" });
    expect(cat.price("acme/new-model", u)).toEqual({ micros: 0, basis: "unknown" });
    expect(cat.price("acme/new-model", u, 77)).toEqual({ micros: 77, basis: "reported" });
    expect(cat.price("not/listed", u)).toEqual({ micros: 0, basis: "unknown" });
    // A local model costs nothing.
    expect(cat.price("llama3.3:70b", usage({ input: 10_000, output: 999 }))).toEqual({ micros: 0, basis: "priced" });
  });

  it("an extra model's weights class is unknown unless set, and it is marked extra", () => {
    expect(cat.get("acme/new-model")).toMatchObject({ weights: "unknown", extra: true, provider: "openrouter", harness: "builtin" });
    expect(cat.get("acme/open-one")?.weights).toBe("open");
    expect(cat.get("llama3.3:70b")).toMatchObject({ provider: "local", weights: "unknown" });
    expect(cat.get("test/closed-explicit")?.extra).toBeUndefined();
  });

  it("names the wire model with its variant, trims the base URL and weighs cache reads", () => {
    expect(cat.wireModel(cat.get("test/open-auto")!)).toBe("test/open-auto:free");
    expect(cat.wireModel(cat.get("test/closed-explicit")!)).toBe("test/closed-explicit");
    expect(cat.openrouterBaseUrl).toBe("http://127.0.0.1:9/api/v1");
    expect(cat.cacheReadWeight("test/closed-explicit")).toBeCloseTo(0.1);
    expect(cat.cacheReadWeight("acme/new-model")).toBe(1);
  });

  it("refuses a malformed catalogue or a duplicate id", () => {
    const code = (f: () => unknown) => {
      try {
        f();
      } catch (e) {
        return (e as { code?: string }).code;
      }
      return "none";
    };
    expect(code(() => Catalogue.parse("{"))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(JSON.stringify({ asOf: "x", openrouterBaseUrl: "https://a", models: [] })))).toBe("CATALOGUE_INVALID");
    const one = JSON.parse(JSON.stringify(Catalogue.load(FIXTURE).get("test/no-tools")));
    const doc = (models: unknown[]) => JSON.stringify({ asOf: "2026-01-01", openrouterBaseUrl: "https://a", models });
    expect(code(() => Catalogue.parse(doc([one, one])))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(doc([{ ...one, weights: "unknown" }])))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(doc([{ ...one, maker: "" }])))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(doc([{ ...one, id: "Bad Id" }])))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(doc([one]), [{ id: "test/no-tools", window: 1 }]))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(doc([one]), [{ id: "x", window: 0 }]))).toBe("CATALOGUE_INVALID");
  });

  it("the bundled catalogue: every entry has a maker and a weights class; open and closed built-in models", () => {
    const bundled = Catalogue.load(BUNDLED_CATALOGUE);
    expect(bundled.asOf).toMatch(/^\d{4}-\d{2}-\d{2}$/);
    expect(bundled.openrouterBaseUrl).toBe("https://openrouter.ai/api/v1");
    const entries = bundled.entries();
    for (const e of entries) {
      expect(e.maker, e.id).not.toBe("");
      expect(["open", "closed"], e.id).toContain(e.weights);
      expect(e.price, e.id).toBeDefined();
      if (e.harness === "builtin") {
        expect(e.provider, e.id).toBe("openrouter");
        expect(e.capabilities.tools, e.id).toBe(true);
        expect(e.id, e.id).toMatch(/^[a-z0-9-]+\/[a-z0-9.-]+$/);
      }
    }
    const builtin = entries.filter((e) => e.harness === "builtin");
    expect(builtin.some((e) => e.weights === "open")).toBe(true);
    expect(builtin.some((e) => e.weights === "closed")).toBe(true);
    for (const id of ["claude-fable-5-1", "claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]) {
      expect(bundled.get(id)?.harness).toBe("claudeCode");
    }
    expect(entries.some((e) => e.harness === "codex")).toBe(true);
  });
});

const limits = (l: Partial<BudgetLimits>): BudgetLimits => ({ ...DEFAULT_BUDGET, ...l });

describe("budget", () => {
  const price = (u: Usage) => u.input + u.output * 10; // 1 micro per input token, 10 per output

  it("stops before a call that cannot fit, by projection", () => {
    const clock = manualClock();
    const b = new Budget(new MemoryLedger(), "s1", limits({ tokens: 1000, moneyMicros: 5000 }), clock, { price });
    b.startRun();
    expect(b.canCall(usage({ input: 100, output: 400 }))).toEqual({ ok: true });
    expect(b.canCall(usage({ input: 100, output: 901 }))).toMatchObject({ ok: false, kind: "tokens", used: 0, limit: 1000 });
    b.charge(usage({ input: 100, output: 100 }), 1100);
    expect(b.canCall(usage({ input: 10, output: 390 }))).toMatchObject({ ok: false, kind: "money", used: 1100, limit: 5000 });
    expect(b.canCall(usage({ input: 10, output: 300 }))).toEqual({ ok: true });
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
    expect(b.canCall(usage({ input: 1 }))).toEqual({ ok: true });
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
    expect(ledger.dailyMoney("2026-11-07")).toBe(1000);
    b.endRun();
    clock.advance(60 * 60_000); // past midnight UTC, between runs
    expect(utcDay(clock.now())).toBe("2026-11-08");
    b.startRun();
    expect(b.canCall(usage({ input: 500 }))).toEqual({ ok: true });
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
    expect(b.canTool()).toEqual({ ok: true });
    expect(b.chargeTool().stop).toBeNull();
    expect(b.canTool()).toMatchObject({ ok: false, kind: "toolCalls" });
    b.startRun();
    expect(b.canCall(ZERO_USAGE)).toEqual({ ok: true });
    expect(b.canTool()).toEqual({ ok: true });
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
