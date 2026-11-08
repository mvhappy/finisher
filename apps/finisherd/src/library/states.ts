// Small pieces of the library service (plan 5D Task 10): running its Effects, timers, and the
// offline reasons of root states (Decisions 17, 18).
import type { RootState, VolumeIdentity } from "@finisher/core-types";
import { FinError } from "@finisher/contracts";
import { Effect, Exit } from "effect";

export interface ServiceTimers {
  after(ms: number, f: () => void): () => void;
}

export const realTimers: ServiceTimers = {
  after(ms, f) {
    const t = setTimeout(f, ms);
    t.unref?.();
    return () => clearTimeout(t);
  },
};

/** Runs `e`, rejecting with its `FinError`. */
export async function exec<A>(e: Effect.Effect<A, FinError>): Promise<A> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) return exit.value;
  const fail = exit.cause.reasons.find((r) => r._tag === "Fail");
  if (fail !== undefined && fail._tag === "Fail") throw fail.error;
  throw new FinError({ code: "INTERNAL", message: "a library operation failed", details: { cause: String(exit.cause) } });
}

export const toFin = (e: unknown): FinError => (e instanceof FinError ? e : new FinError({ code: "INTERNAL", message: String(e), details: {} }));

/** The offline reason of a root state (codes for `offlineReason` and `library.status`). */
export function offlineReason(state: RootState | undefined): string {
  switch (state?.state) {
    case "unmounted":
      return "ROOT_UNMOUNTED";
    case "volumeChanged":
      return "ROOT_VOLUME_CHANGED";
    case "empty":
      return "ROOT_EMPTY_SUSPECT";
    case "timeout":
      return "ROOT_TIMEOUT";
    case "unreachable":
      return "ROOT_UNREACHABLE";
    default:
      return "LIBRARY_OFFLINE";
  }
}

export const identityOf = (s: Extract<RootState, { state: "online" }>): VolumeIdentity => {
  const { state: _s, ...identity } = s;
  return identity;
};

export const sameVolume = (a: VolumeIdentity, b: VolumeIdentity): boolean => JSON.stringify(a) === JSON.stringify(b);

/** Offline reasons shown as `library.status` warnings. */
export const WARNING_REASONS = new Set(["ROOT_EMPTY_SUSPECT", "ROOT_UNMOUNTED", "ROOT_VOLUME_CHANGED", "RESOURCE_STUCK", "CATALOG_TOO_NEW"]);
