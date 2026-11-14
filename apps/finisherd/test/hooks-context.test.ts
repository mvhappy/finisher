// Held hook-context identities (`HookContexts`, 7F Task 6 with 7D Decision 13): context steered to a
// provider is known by its `userMessage` node id while its provider can still hand it back (its
// run and the next one, for a late tail), and at most `HELD_MAX` per session; an identity no
// longer known comes back as a person's steer (its block's text).
import type { Session } from "@finisher/contracts";
import { Effect } from "effect";
import { describe, expect, test } from "vitest";
import { HELD_MAX, type ContextInternals, HookContexts, type HookContextItem } from "../src/agent/hook-context.ts";
import type { Live } from "../src/agent/runtime-live.ts";

function contexts() {
  let n = 0;
  const live = { runId: "r1", caps: { steer: true }, provider: { steer: () => Effect.void } };
  const d: ContextInternals = {
    session: (id) => ({ id, status: "running" }) as Session,
    activeRun: () => live.runId,
    live: () => live as unknown as Live,
    append: async () => ({ node: { id: `n${++n}`, createdAt: n } }),
    queued: async () => undefined,
    background: (_id, _what, f) => void f(),
    log: () => {},
  };
  const c = new HookContexts(d);
  /** Run `runId` starts and takes `k` hook steers; their node ids. */
  const run = async (runId: string, k: number) => {
    live.runId = runId;
    await c.open("s1", runId);
    const ids: string[] = [];
    for (let i = 0; i < k; i++) {
      await c.deliver("s1", item(`${runId}-${i}`));
      ids.push(`n${n}`);
    }
    return ids;
  };
  /** A `steerReturned` notice with these node ids, as `returned` writes it. */
  const back = (ids: readonly string[]) => c.returned({ sessionId: "s1", runId: live.runId, kind: "notice", payload: { steerReturned: { harness: "fake", steers: ids.map((nodeId) => ({ text: `<${nodeId}>`, nodeId })) } } }).payload as { steerReturned?: { steers: Array<{ nodeId: string }> }; hookContextHeld?: { nodeIds: string[] } };
  return { run, back };
}

const item = (text: string): HookContextItem => ({ event: "PostCommand", hooks: ["h"], text });

describe("held hook-context identities", () => {
  test("consumed_context_is_reclaimed: a run's ids live through the next run (a late tail), not the one after", async () => {
    const { run, back } = contexts();
    const r1 = await run("r1", 3);
    const r2 = await run("r2", 2);
    // r1's tail after r2 started: still known.
    expect(back([r1[0]!]).hookContextHeld).toEqual({ nodeIds: [r1[0]] });
    await run("r3", 0);
    // r1's other (consumed) ids are gone; r2's are known.
    const out = back([...r1.slice(1), ...r2]);
    expect(out.hookContextHeld).toEqual({ nodeIds: r2 });
    expect(out.steerReturned!.steers.map((x) => x.nodeId)).toEqual(r1.slice(1));
  });

  test("repeated_consumed_steers_stay_bounded: at most HELD_MAX per session, the oldest evicted first", async () => {
    const { run, back } = contexts();
    const ids = await run("r1", HELD_MAX + 6);
    const out = back(ids);
    expect(out.hookContextHeld!.nodeIds).toEqual(ids.slice(6));
    // An evicted id comes back as a person's steer.
    expect(out.steerReturned!.steers.map((x) => x.nodeId)).toEqual(ids.slice(0, 6));
  });
});
