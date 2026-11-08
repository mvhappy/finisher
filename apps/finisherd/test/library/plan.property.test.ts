// The poll planner (plan 5D Task 10; Decisions 18, 28–30) as properties over random libraries
// and clocks, and a simulation: applying its actions never runs two jobs of one pool kind for
// one library, never polls more often than the interval, and never scans an offline or paused
// library.
import fc from "fast-check";
import { describe, expect, test } from "vitest";
import { type PlanLibrary, DAY_MS, IDLE_POLL_MS, IN_USE_POLL_MS, RECHECK_MS, fullInterval, plan, pollInterval } from "../../src/library/plan.ts";

const RUNS = Number(process.env.FIN_PLAN_RUNS ?? 300);
const T0 = 1_700_000_000_000;

const libArb = (i: number): fc.Arbitrary<PlanLibrary> =>
  fc.record({
    id: fc.constant(`lib${i}`),
    storage: fc.constantFrom("local", "network", "sync"),
    state: fc.constantFrom("online", "offline", "scanning", "paused"),
    scan: fc.record({ poll: fc.oneof(fc.constant("demand" as const), fc.integer({ min: 30, max: 86_400 })), full: fc.constantFrom("daily", "weekly", "manual") }),
    createdAt: fc.integer({ min: T0 - 30 * DAY_MS, max: T0 }),
    lastPollAt: fc.option(fc.integer({ min: T0 - 2 * DAY_MS, max: T0 }), { nil: undefined }),
    lastFullAt: fc.option(fc.integer({ min: T0 - 10 * DAY_MS, max: T0 }), { nil: undefined }),
    lastFullTry: fc.option(fc.integer({ min: T0 - 2 * DAY_MS, max: T0 }), { nil: undefined }),
    lastRecheckAt: fc.option(fc.integer({ min: T0 - 5 * 60_000, max: T0 }), { nil: undefined }),
    watch: fc.constantFrom("none", "ok", "degraded"),
    running: fc.record({ full: fc.boolean(), poll: fc.boolean(), recheck: fc.boolean() }),
  }) as fc.Arbitrary<PlanLibrary>;

const worldArb = fc.integer({ min: 1, max: 6 }).chain((n) => fc.tuple(...Array.from({ length: n }, (_, i) => libArb(i))));

describe("plan", () => {
  test("plan_at_most_one_job_per_pool_kind_per_library", () => {
    fc.assert(
      fc.property(worldArb, fc.integer({ min: 0, max: DAY_MS }), fc.func(fc.boolean()), (libs, dt, used) => {
        const now = T0 + dt;
        const actions = plan(libs, now, (id) => used(id));
        expect(actions.map((a) => a.library)).toEqual(libs.map((l) => l.id));
        // Pure: the same input plans the same.
        expect(plan(libs, now, (id) => used(id))).toEqual(actions);
        for (const [i, a] of actions.entries()) {
          const lib = libs[i]!;
          expect(a.next).toBeGreaterThan(now);
          if (lib.state === "paused") expect(a.action).toBe("none");
          if (lib.state === "offline") expect(["recheck", "none"]).toContain(a.action);
          else expect(a.action).not.toBe("recheck");
          if (a.action === "full") expect(lib.running.full).toBe(false);
          if (a.action === "poll") expect(lib.running.poll).toBe(false);
          if (a.action === "recheck") expect(lib.running.recheck).toBe(false);
          if (a.action === "full" && lib.storage !== "local" && pollInterval(lib, used(lib.id)) !== undefined) {
            // A remote full scan only at a poll tick.
            const base = Math.max(lib.lastPollAt ?? lib.lastFullAt ?? lib.createdAt, lib.lastFullTry ?? -Infinity);
            expect(now).toBeGreaterThanOrEqual(base + pollInterval(lib, used(lib.id))!);
          }
          if (a.action === "full") expect(fullInterval(lib.scan.full)).toBeDefined();
          if (a.action === "poll") expect(pollInterval(lib, used(lib.id))).toBeDefined();
        }
      }),
      { numRuns: RUNS },
    );
  });

  test("simulated_jobs_never_overlap_or_poll_early", () => {
    fc.assert(
      fc.property(
        worldArb,
        fc.array(fc.record({ dt: fc.integer({ min: 1_000, max: 2 * 3_600_000 }), used: fc.boolean(), jobMs: fc.integer({ min: 0, max: 3_600_000 }) }), { minLength: 5, maxLength: 60 }),
        (initial, steps) => {
          let now = T0;
          const libs: PlanLibrary[] = initial.map(({ lastPollAt: _p, lastFullTry: _t, ...l }) => ({ ...l, running: { full: false, poll: false, recheck: false } }));
          const ends: Array<{ lib: number; kind: "full" | "poll"; at: number }> = [];
          const polls = new Map<number, number[]>();
          for (const step of steps) {
            now += step.dt;
            for (const e of ends.filter((x) => x.at <= now)) {
              const l = libs[e.lib]!;
              libs[e.lib] = { ...l, running: { ...l.running, [e.kind]: false }, ...(e.kind === "poll" ? { lastPollAt: e.at } : { lastFullAt: e.at, lastPollAt: e.at }) };
            }
            ends.splice(0, ends.length, ...ends.filter((x) => x.at > now));
            const actions = plan(libs, now, () => step.used);
            for (const [i, a] of actions.entries()) {
              const l = libs[i]!;
              if (a.action === "poll" || a.action === "full") {
                expect(l.running[a.action]).toBe(false);
                expect(["online", "scanning"]).toContain(l.state);
                libs[i] = { ...l, running: { ...l.running, [a.action]: true }, ...(a.action === "full" ? { lastFullTry: now } : {}) };
                ends.push({ lib: i, kind: a.action, at: now + step.jobMs });
                if (a.action === "poll") polls.set(i, [...(polls.get(i) ?? []), now]);
              }
            }
          }
          // Gaps between polls of one library are never below the shortest interval it can have.
          for (const [i, times] of polls) {
            const l = libs[i]!;
            const floor = Math.min(pollInterval(l, true) ?? Infinity, pollInterval(l, false) ?? Infinity);
            for (let k = 1; k < times.length; k++) expect(times[k]! - times[k - 1]!).toBeGreaterThanOrEqual(floor);
          }
        },
      ),
      { numRuns: Math.ceil(RUNS / 3) },
    );
  });

  test("cadences", () => {
    const base = { storage: "network", scan: { poll: "demand", full: "daily" }, watch: "none" } as const;
    expect(pollInterval(base, true)).toBe(IN_USE_POLL_MS);
    expect(pollInterval(base, false)).toBe(IDLE_POLL_MS);
    // Watched: a safety-net poll every 6 hours, in use or not.
    expect(pollInterval({ ...base, storage: "local", watch: "ok" }, true)).toBe(6 * 3_600_000);
    expect(pollInterval({ ...base, storage: "local", watch: "ok" }, false)).toBe(6 * 3_600_000);
    expect(pollInterval({ ...base, storage: "local", watch: "degraded" }, false)).toBe(5 * 60_000);
    expect(pollInterval({ ...base, storage: "sync", scan: { poll: 600, full: "daily" } }, false)).toBe(600_000);
    const off: PlanLibrary = { id: "x", ...base, state: "offline", createdAt: T0, watch: "none", running: { full: false, poll: false, recheck: false }, lastRecheckAt: T0 };
    expect(plan([off], T0 + RECHECK_MS - 1, () => false)[0]).toMatchObject({ action: "none", next: T0 + RECHECK_MS });
    expect(plan([off], T0 + RECHECK_MS, () => false)[0]!.action).toBe("recheck");
  });
});
