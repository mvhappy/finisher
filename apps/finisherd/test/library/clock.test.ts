// The catalog threads' default clock holds forward jumps (plan 5D Task 7b review).
import { expect, test } from "vitest";
import { guardedClock } from "../../src/library/catalog/clock.ts";

test("guarded clock: follows the wall, holds a forward jump, accepts one that persists", () => {
  let wall = 1_000_000;
  let mono = 0;
  const clock = guardedClock({ wall: () => wall, mono: () => mono, slackMs: 60_000, persistMs: 600_000 });
  expect(clock()).toBe(1_000_000);
  wall += 5_000;
  mono += 5_000;
  expect(clock()).toBe(1_005_000);
  // The wall clock jumps a day ahead: catalog time keeps the monotonic pace.
  wall += 86_400_000;
  mono += 1_000;
  expect(clock()).toBe(1_006_000);
  // Corrected back within the hold: the wall again.
  wall = 1_007_000;
  mono += 1_000;
  expect(clock()).toBe(1_007_000);
  // A jump that persists past `persistMs` is taken.
  wall += 86_400_000;
  mono += 1_000;
  expect(clock()).toBe(1_008_000);
  wall += 300_000;
  mono += 300_000;
  expect(clock()).toBe(1_308_000);
  wall += 300_000;
  mono += 300_000;
  expect(clock()).toBe(wall);
  // Backward jumps pass through (the indexer keeps catalog time from going back).
  wall -= 3_600_000;
  mono += 1_000;
  expect(clock()).toBe(wall);
});
