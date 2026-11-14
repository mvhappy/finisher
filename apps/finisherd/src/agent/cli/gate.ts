// The first-event gate of a CLI instance (plan 7D Decision 8, Review Focus 1; lifted from
// `claude/probe-check.ts` in plan 7E Task 1): nothing an instance emits reaches the runtime until
// the instance's own check passed (Claude Code's `system/init`, `checkInit`; Codex's
// `initialize`/`config/read`/`thread/start` facts, `codex/probe-check.ts` `checkInstance`).
//
// - **Waiting**: every item is held, in arrival order (`push`), except what the caller handles at
//   once itself (a harness's handshake lines never reach the gate).
// - **The check** (`verify`): the checked item passes `check` → the gate opens and releases what
//   it held, the checked item last; it fails → the gate refuses (the caller kills the instance and
//   fails the turn) and drops what it held. Every later checked item is checked again, in any
//   state; once open it is forwarded alone, and a failing one refuses.
// - **Refused**: every later item is dropped.
// - `withdraw` drops a held item (a cancelled request that never reached the runtime);
//   `discard` drops everything held without opening (an instance that ended before its check).
import type { FinError } from "@finisher/contracts";

/** What the caller does with an item: pass these on, nothing yet, or stop the instance. */
export type GateStep<T> =
  | { readonly kind: "forward"; readonly lines: readonly T[] }
  | { readonly kind: "hold" }
  /**
   * The check failed: fail the turn with `error` and kill the instance at once (SIGKILL of its
   * process group through the sandbox, `SandboxedChild.kill`); an interrupt only where no kill is
   * possible. An interrupt lets the turn play on, a kill does not.
   */
  | { readonly kind: "refuse"; readonly error: FinError; readonly kill: true }
  /** After a refusal: the item is dropped. */
  | { readonly kind: "drop" };

export interface EventGate<T> {
  readonly state: "waiting" | "open" | "refused";
  /** Any item but a checked one: held while waiting, forwarded once open, dropped once refused. */
  push(item: T): GateStep<T>;
  /** An item `check` reads: opens (releasing what is held), forwards, or refuses. */
  verify(item: T): GateStep<T>;
  /** Refuses without a check (an item that may not come before the checked one). */
  refuse(error: FinError): GateStep<T>;
  /** Drops the first held item `match` selects; whether there was one. */
  withdraw(match: (item: T) => boolean): boolean;
  /** Drops everything held; the gate stays as it is. */
  discard(): void;
}

/** A gate holding an instance's items until `check` passes on its checked item. */
export function eventGate<T>(check: (first: T) => FinError | null): EventGate<T> {
  let state: EventGate<T>["state"] = "waiting";
  let held: T[] = [];
  const refuse = (error: FinError): GateStep<T> => {
    state = "refused";
    held = [];
    return { kind: "refuse", error, kill: true };
  };
  return {
    get state() {
      return state;
    },
    push(item) {
      if (state === "refused") return { kind: "drop" };
      if (state === "open") return { kind: "forward", lines: [item] };
      held.push(item);
      return { kind: "hold" };
    },
    verify(item) {
      if (state === "refused") return { kind: "drop" };
      const e = check(item);
      if (e !== null) return refuse(e);
      if (state === "open") return { kind: "forward", lines: [item] };
      state = "open";
      const lines = [...held, item];
      held = [];
      return { kind: "forward", lines };
    },
    refuse,
    withdraw(match) {
      const i = held.findIndex(match);
      if (i < 0) return false;
      held.splice(i, 1);
      return true;
    },
    discard() {
      held = [];
    },
  };
}
