// The built-in loop waits on an approval itself (sub-project 7B Task 5, Decision 11; spec §7.3):
// a `fin` call that answers `APPROVAL_PENDING` (exit 8) is not handed to the model as a
// failure. The call stays open while the card waits (the session is `waitingApproval`, as the
// card made it), in waits of at most 90 s each, until a person answers or the card expires:
//
// - **allow:** the same call runs again, with the same `FIN_CALL` (its idempotency key), and
//   consumes the one-time grant; its own result is the tool result (another card, for a request
//   that changed in between, is waited on again, at most `MAX_ROUNDS` times);
// - **deny** or **expiry:** the tool result is `APPROVAL_DENIED` (exit 7: do not retry), with
//   the approval and how it ended.
// An interrupt (the call's abort signal) stops the wait: `SESSION_INTERRUPTED`, as any call.
import type { FinError } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { finError } from "../../errors.ts";
import type { WaitOutcome } from "../approvals.ts";
import type { ToolCall, ToolExecutor, ToolOutcome } from "../runtime-deps.ts";

/** One wait on the card (the `agent.approval.wait` cap). */
export const WAIT_SLICE_MS = 90_000;
/** Cards one call waits on at most (a call whose request keeps changing stops asking). */
export const MAX_ROUNDS = 4;

export interface ApprovalWaiter {
  wait(id: string, timeoutMs: number, signal?: AbortSignal): Effect.Effect<WaitOutcome, FinError>;
}

const record = (v: unknown): Record<string, unknown> | undefined => (v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined);

/** The card a tool result waits on: a `fin` answer of `APPROVAL_PENDING` with its approval id. */
export function pendingApproval(o: ToolOutcome): string | undefined {
  if (o.ok) return undefined;
  const out = record(o.output);
  const error = record(record(out?.stdout)?.error) ?? record(out?.error);
  const id = record(error?.details)?.approval;
  return error?.code === "APPROVAL_PENDING" && typeof id === "string" ? id : undefined;
}

function shape(e: FinError): Record<string, unknown> {
  return { code: e.code, message: e.message, details: e.details, ...(e.fix === undefined ? {} : { fix: e.fix }) };
}

/** `APPROVAL_DENIED` as a tool result (exit 7). */
function denied(id: string, how: "deny" | "expired"): ToolOutcome {
  const e = finError(
    "APPROVAL_DENIED",
    how === "deny" ? `a person denied approval ${id}` : `approval ${id} expired before a person answered it`,
    { approval: id, decision: how },
    "Do not repeat this request: do something else, or ask the person what they want.",
  );
  return { ok: false, output: { exit: 7, error: shape(e) } };
}

const interrupted = (): ToolOutcome => ({ ok: false, output: { exit: null, error: shape(finError("SESSION_INTERRUPTED", "the tool call was stopped", {})) } });

/** `exec`, waiting on the approvals its `fin` calls meet (module docs). */
export function waitingOnApprovals(exec: ToolExecutor, approvals: () => ApprovalWaiter | undefined, slice = WAIT_SLICE_MS): ToolExecutor {
  return {
    run: async (call: ToolCall, signal: AbortSignal) => {
      let outcome = await exec.run(call, signal);
      for (let round = 0; round < MAX_ROUNDS; round++) {
        const id = pendingApproval(outcome);
        const waiter = approvals();
        if (id === undefined || waiter === undefined) return outcome;
        let o: WaitOutcome = "pending";
        while (o === "pending") {
          if (signal.aborted) return interrupted();
          const exit = await Effect.runPromiseExit(waiter.wait(id, slice, signal));
          if (Exit.isFailure(exit)) return outcome;
          o = exit.value;
        }
        if (o !== "allow") return denied(id, o);
        if (signal.aborted) return interrupted();
        outcome = await exec.run(call, signal);
      }
      return outcome;
    },
  };
}
