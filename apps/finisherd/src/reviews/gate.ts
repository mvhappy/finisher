// The policy gate (5E plan Decision 18): a per-project readers–writer lock that serialises a
// project's policy writes with its in-flight writes. Every write RPC holds it shared from the
// chain's policy stage to its answer; `review.policy` (and sub-project 10's auto-merge
// enabling) holds it exclusive, waiting at most `POLICY_WAIT_MS` (`POLICY_BUSY`). So a write
// either completes under the old policy or is checked under the new one.
//
// Fair in arrival order: a shared request that arrives while an exclusive one waits queues
// behind it, so a stream of writes cannot starve a policy write, and a write that starts after
// a policy write began is checked under the new policy.
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";

/** How long a policy write waits for the project's in-flight writes. */
export const POLICY_WAIT_MS = 30_000;

interface Waiter {
  readonly exclusive: boolean;
  readonly grant: () => void;
}

interface Lock {
  readers: number;
  writer: boolean;
  readonly queue: Waiter[];
}

export function policyBusy(projectId: string, waitedMs: number): FinError {
  return finError(
    "POLICY_BUSY",
    "the project's policy could not be changed: writes to it are still running",
    { projectId, waitedMs },
    "Writes to this project took too long to finish. Try again in a moment.",
  );
}

export class PolicyGate {
  readonly #locks = new Map<string, Lock>();
  readonly #waitMs: number;

  constructor(options: { readonly waitMs?: number } = {}) {
    this.#waitMs = options.waitMs ?? POLICY_WAIT_MS;
  }

  /** `f` with the project's gate held shared (a write). */
  shared<A, E, R>(projectId: string, f: Effect.Effect<A, E, R>): Effect.Effect<A, E, R> {
    return Effect.acquireUseRelease(this.acquireShared(projectId), () => f, (release) => Effect.sync(release));
  }

  /** `f` with the project's gate held exclusive (a policy write); `POLICY_BUSY` past `timeoutMs`. */
  exclusive<A, E, R>(projectId: string, f: Effect.Effect<A, E, R>, timeoutMs?: number): Effect.Effect<A, E | FinError, R> {
    return Effect.acquireUseRelease(this.#acquire(projectId, true, timeoutMs ?? this.#waitMs), () => f, (release) => Effect.sync(release));
  }

  /**
   * Takes the gate shared and answers its release (call it once). The chain holds it across
   * stages and the handler (`chain.ts`); prefer `shared` elsewhere.
   */
  acquireShared(projectId: string): Effect.Effect<() => void> {
    return this.#acquire(projectId, false, undefined) as Effect.Effect<() => void>;
  }

  #lock(projectId: string): Lock {
    let l = this.#locks.get(projectId);
    if (l === undefined) {
      l = { readers: 0, writer: false, queue: [] };
      this.#locks.set(projectId, l);
    }
    return l;
  }

  #release(projectId: string, l: Lock, exclusive: boolean): () => void {
    let done = false;
    return () => {
      if (done) return;
      done = true;
      if (exclusive) l.writer = false;
      else l.readers--;
      this.#drain(projectId, l);
    };
  }

  /** Grants what the queue's head allows, in order; forgets an idle project. */
  #drain(projectId: string, l: Lock): void {
    while (l.queue.length > 0 && !l.writer) {
      const head = l.queue[0]!;
      if (head.exclusive && l.readers > 0) break;
      l.queue.shift();
      if (head.exclusive) l.writer = true;
      else l.readers++;
      head.grant();
    }
    if (!l.writer && l.readers === 0 && l.queue.length === 0) this.#locks.delete(projectId);
  }

  #acquire(projectId: string, exclusive: boolean, timeoutMs: number | undefined): Effect.Effect<() => void, FinError> {
    return Effect.callback<() => void, FinError>((resume) => {
      const l = this.#lock(projectId);
      const free = !l.writer && l.queue.length === 0 && (!exclusive || l.readers === 0);
      if (free) {
        if (exclusive) l.writer = true;
        else l.readers++;
        resume(Effect.succeed(this.#release(projectId, l, exclusive)));
        return;
      }
      let granted = false;
      let timer: ReturnType<typeof setTimeout> | undefined;
      const waiter: Waiter = {
        exclusive,
        grant: () => {
          granted = true;
          if (timer !== undefined) clearTimeout(timer);
          resume(Effect.succeed(this.#release(projectId, l, exclusive)));
        },
      };
      l.queue.push(waiter);
      const leave = () => {
        const i = l.queue.indexOf(waiter);
        if (i >= 0) l.queue.splice(i, 1);
        this.#drain(projectId, l);
      };
      if (timeoutMs !== undefined) {
        timer = setTimeout(() => {
          if (granted) return;
          leave();
          resume(Effect.fail(policyBusy(projectId, timeoutMs)));
        }, timeoutMs);
      }
      // Interrupted while queued: leave the queue; granted already: give the hold back.
      return Effect.sync(() => {
        if (timer !== undefined) clearTimeout(timer);
        if (granted) this.#release(projectId, l, exclusive)();
        else leave();
      });
    });
  }
}
