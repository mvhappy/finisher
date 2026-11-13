// What a resumed Claude Code instance is told, and refused (plan 7D Task 10; Decision 17; Review
// Focus 5: no tool call re-runs on resume).
//
// - **The runtime's synthetic results** (spec §3.4): a call the session's last process left
//   without a result (it was killed mid-tool, or `finisherd` was) has the runtime's synthetic
//   `SESSION_INTERRUPTED` result. The resumed CLI's transcript holds the call; the next user line
//   carries the synthetic results, as a text block before the run's facts, so the model knows the
//   call ended there and was not run again (whatever it changed is already in the project).
// - **No re-run:** a permission prompt for a call an earlier process announced (its
//   `tool_use_id`) is denied at once, never reaching the runtime (`claudePermissions`' `rerun`).
// Verified 2.1.321, Task 0 (s1d): `--resume` after a SIGKILL mid-turn finds the transcript with
// its context intact; (s5C) a CLI left by a SIGKILLed host may finish the turn unwatched, so the
// transcript can hold a whole turn the runtime never saw (`provider-pid.ts`'s `unseen`).
// TODO(7D Task 0 Step 1/5): whether the CLI asks again for a call whose result never came (a
// pending tool call's state on resume); modelled by fake-claude as `onResumeDangling: leave`
// (default) or `reask` (a `can_use_tool` with the same `tool_use_id` at the first turn).
import type { Node } from "@finisher/contracts";

/** A call that ended with the runtime's synthetic result, by the provider's id. */
export interface InterruptedCall {
  readonly providerId: string;
  readonly tool: string;
  readonly code: string;
  readonly reason: string | null;
}

export interface ResumeState {
  /** Every call an earlier instance announced (`toolCall` nodes' provider ids). */
  readonly known: ReadonlySet<string>;
  /** The synthetic results the CLI has not been told of: after the last line it produced. */
  readonly interrupted: readonly InterruptedCall[];
}

/** Nodes the CLI's own lines produced: a synthetic result before one of them was already behind it. */
const FROM_CLI = new Set(["assistantMessage", "reasoning", "usage"]);

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** The session's nodes, oldest first, read for a resumed instance. */
export function resumeState(nodes: readonly Node[]): ResumeState {
  const known = new Set<string>();
  const calls = new Map<string, { providerId: string; tool: string }>();
  let interrupted: InterruptedCall[] = [];
  for (const n of nodes) {
    if (n.kind === "toolCall" && n.providerId !== undefined) {
      known.add(n.providerId);
      const tool = isObject(n.payload) && typeof n.payload.tool === "string" ? n.payload.tool : "tool";
      if (n.callId !== undefined) calls.set(n.callId, { providerId: n.providerId, tool });
    } else if (n.kind === "toolResult" && n.callId !== undefined) {
      const p = n.payload;
      const call = calls.get(n.callId);
      if (isObject(p) && p.synthetic === true && call !== undefined) {
        interrupted.push({ ...call, code: typeof p.code === "string" ? p.code : "SESSION_INTERRUPTED", reason: typeof p.reason === "string" ? p.reason : null });
      } else interrupted = [];
    } else if (FROM_CLI.has(n.kind)) interrupted = [];
  }
  return { known, interrupted };
}

const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

/** The text block the first user line of a resumed instance carries (none when nothing was interrupted). */
export function interruptedNote(calls: readonly InterruptedCall[]): string | undefined {
  if (calls.length === 0) return undefined;
  const list = calls
    .slice(0, 32)
    .map((c) => `- ${clip(c.providerId, 64)} (${clip(c.tool, 64)}): ${c.code}${c.reason === null ? "" : ` (${clip(c.reason, 64)})`}`)
    .join("\n");
  return [
    "Finisher: the session was interrupted while these tool calls were running. Each one's result is the one below; none of them is run again.",
    list,
    "Anything they changed before the interruption is already in the project: check its state before you repeat one.",
  ].join("\n");
}
