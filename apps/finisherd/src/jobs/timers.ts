// The engine's timers (9a review): every delay the engine waits (progress throttling, item and
// checkpoint flushes, grace periods, retries, retention) runs on the engine's Effect `Clock`,
// so a test that gives the engine a `TestClock` drives them with `TestClock.adjust`.
import { Duration, Effect, Fiber } from "effect";
import type { RunFork } from "./calls.ts";

export interface Timers {
  /** Calls `f` once `ms` passed on the engine's clock; the function returned cancels it. */
  after(ms: number, f: () => void): () => void;
}

export function clockTimers(run: RunFork, log: (message: string) => void): Timers {
  return {
    after(ms, f) {
      let live = true;
      const fiber = run(
        Effect.sleep(Duration.millis(Math.max(0, ms))).pipe(
          Effect.andThen(
            Effect.sync(() => {
              if (!live) return;
              live = false;
              try {
                f();
              } catch (e) {
                log(`a job engine timer threw: ${(e as Error).stack ?? String(e)}`);
              }
            }),
          ),
        ),
      );
      return () => {
        if (!live) return;
        live = false;
        run(Fiber.interrupt(fiber));
      };
    },
  };
}
