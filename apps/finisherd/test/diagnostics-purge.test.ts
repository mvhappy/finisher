// A purge across both stores (plan 7A Task 10 review H2): the agent-trace store first, both
// always attempted, a failed store retried, and a per-store report; a store that still fails
// fails the call, naming it, with what the other store removed.
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { expect, test } from "vitest";
import { PURGE_ATTEMPTS, purgeStores } from "../src/diagnostics-purge.ts";

const report = (removed: number) => JSON.stringify({ removed, files: 1, blobs: removed > 0 ? 1 : 0 });
const failure = Object.assign(new Error("disk full"), { code: "JOURNAL_UNAVAILABLE", finNode: true, details: {} });

/** A store whose purge fails the first `fails` times (Infinity: always), recording the calls. */
function store(name: string, calls: string[], removed: number, fails = 0) {
  let n = 0;
  return {
    purge: async (sel: string) => {
      calls.push(`${name} ${sel}`);
      if (n++ < fails) throw failure;
      return report(removed);
    },
  };
}

test("the trace store goes first; both report; their sums answer", async () => {
  const calls: string[] = [];
  const out = await Effect.runPromise(purgeStores({ agentTraces: store("traces", calls, 3), diagnostics: store("diag", calls, 5) }, "all"));
  expect(calls).toEqual(['traces "all"', 'diag "all"']);
  expect(out).toEqual({
    removed: 8,
    files: 2,
    blobs: 2,
    stores: { agentTraces: { removed: 3, files: 1, blobs: 1 }, diagnostics: { removed: 5, files: 1, blobs: 1 } },
  });
});

test("a store that fails once is retried, and the purge succeeds", async () => {
  const calls: string[] = [];
  const out = await Effect.runPromise(purgeStores({ agentTraces: store("traces", calls, 3, 1), diagnostics: store("diag", calls, 5) }, { project: "p" }));
  expect(calls).toEqual(['traces {"project":"p"}', 'traces {"project":"p"}', 'diag {"project":"p"}']);
  expect(out.removed).toBe(8);
});

test("the trace store failing for good: the diagnostics journal is still purged, and the call fails naming the store", async () => {
  const calls: string[] = [];
  const r = await Effect.runPromise(Effect.result(purgeStores({ agentTraces: store("traces", calls, 3, Infinity), diagnostics: store("diag", calls, 5) }, "all")));
  expect(calls).toEqual([...Array(PURGE_ATTEMPTS).fill('traces "all"'), 'diag "all"']);
  expect(r._tag).toBe("Failure");
  const e = (r as { failure: FinError }).failure;
  expect(e.message).toContain("agentTraces");
  expect(e.details).toMatchObject({ stores: { diagnostics: { removed: 5 } }, failed: ["agentTraces"] });
});

test("the diagnostics journal failing for good: the trace store's report is in the error's details", async () => {
  const calls: string[] = [];
  const r = await Effect.runPromise(Effect.result(purgeStores({ agentTraces: store("traces", calls, 3), diagnostics: store("diag", calls, 5, Infinity) }, "all")));
  expect(r._tag).toBe("Failure");
  const e = (r as { failure: FinError }).failure;
  expect(e.code).toBe("JOURNAL_UNAVAILABLE");
  expect(e.details).toEqual({
    stores: { agentTraces: { removed: 3, files: 1, blobs: 1 }, diagnostics: { error: { code: "JOURNAL_UNAVAILABLE", message: expect.stringContaining("disk full") } } },
    failed: ["diagnostics"],
  });
  expect(e.message).toContain("diagnostics");
});
