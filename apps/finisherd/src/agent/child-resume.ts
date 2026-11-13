// Who may resume a session, and what a resume resends (plan 7B Decision 16 as amended by the
// owner decision 2026-11-13; plan 7D Decision 13).
//
// - **The interrupt's cause** is the session's `statusReason` (the state machine records one on
//   every interrupt and keeps the first while the session stays interrupted): an
//   `agent.session.interrupt` by the session's parent is `parentInterrupt`, by any other agent
//   `agentInterrupt`, by a person `userInterrupt`. Not `parentInterrupted`: that is the cascade
//   of the parent's own interruption (`subagents.ts`), a person's or a budget's doing.
// - **A parent agent resumes its own child only when the child is interrupted by that parent's
//   own interrupt** (`parentInterrupt`, the parent being the session's only one). Every other
//   cause (a budget stop, a person's interrupt, a `finisherd` crash, a provider exit, the
//   cascade) still needs a person: `SCOPE_DENIED {reason: "interruptedByOther"}`; a session not
//   its child `{reason: "notOwnChild"}`. A child that is not interrupted is left to the
//   runtime's own answer (a resume of an idle session changes nothing). People resume anything
//   in reach. Checked at stage 4 (`reach.ts`) and again in the runtime's serialised resume.
// - **`resendReturned`**: the texts of the last `notice {steerReturned}` of the session's last
//   run, in order, joined as the runtime joins held steers (`\n\n`), become the resume's prompt.
import type { FinError, Principal, Session } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { AgentStore } from "./store.ts";

export const PARENT_INTERRUPT = "parentInterrupt";

/** The `statusReason` an `agent.session.interrupt` of `s` by `by` records. */
export function interruptReason(s: Session, by: Principal): string {
  if (by.kind !== "agent") return "userInterrupt";
  return s.parent === by.id ? PARENT_INTERRUPT : "agentInterrupt";
}

/** Why `by` may not resume `s` (`SCOPE_DENIED`), or null. */
export function resumeDenied(s: Session | undefined, by: Principal): FinError | null {
  if (by.kind !== "agent") return null;
  const deny = (reason: string, message: string, extra: Record<string, unknown> = {}) =>
    finError("SCOPE_DENIED", message, { tag: "agent.session.resume", scopes: [], principal: "agent", reason, ...extra }, "Ask a person to resume it (`finisherd agent resume <id>`).");
  if (s === undefined || s.parent !== by.id) return deny("notOwnChild", "a session resumes only its own children");
  if (s.status === "interrupted" && s.statusReason !== PARENT_INTERRUPT) {
    return deny("interruptedByOther", "a session resumes its child only after its own interrupt", { statusReason: s.statusReason ?? null });
  }
  return null;
}

const PAGE = 1000;

/** The texts of the last `steerReturned` notice of `s`'s last run, joined; `INVALID_REQUEST` when there is none. */
export async function returnedSteers(store: AgentStore, s: Session): Promise<string> {
  const run = store.runs(s.id).at(-1);
  let last: unknown;
  if (run !== undefined) {
    for (let from = 1; ; ) {
      const page = store.nodes(s.id, from, PAGE);
      for (const n of page) {
        if (n.runId !== run.id || n.kind !== "notice") continue;
        const p = (await store.payload(n)) as { steerReturned?: unknown } | null;
        if (p?.steerReturned !== undefined) last = p.steerReturned;
      }
      if (page.length < PAGE) break;
      from = page.at(-1)!.seq + 1;
    }
  }
  const steers = (last as { steers?: ReadonlyArray<{ text?: unknown }> } | undefined)?.steers ?? [];
  const texts = steers.flatMap((x) => (typeof x.text === "string" ? [x.text] : []));
  if (texts.length === 0) {
    throw finError("INVALID_REQUEST", "the session's last run handed no steers back", { field: "resendReturned", reason: "noSteerReturned", sessionId: s.id }, "resume it with a prompt instead");
  }
  return texts.join("\n\n");
}

/** A resume's prompt: `given`, or (`resendReturned`) the texts handed back; never both. */
export async function resendPrompt(store: AgentStore, s: Session, given: string | undefined, resendReturned: boolean): Promise<string | undefined> {
  if (!resendReturned) return given;
  if (given !== undefined) throw finError("INVALID_REQUEST", "a resume takes a prompt or resendReturned, not both", { field: "resendReturned", reason: "promptAndResend" }, "drop one of them");
  return returnedSteers(store, s);
}
