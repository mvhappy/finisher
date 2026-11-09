// Live limits per server (sub-project 7 §5.5; plan 7B Decision 19): at most
// `agents.maxLiveProviders` (4) provider processes and `agents.maxLiveBuiltin` (8) built-in
// sessions in a run (`running`, `waitingApproval`) at once. Children count like any session.
//
// - **Never a failed call.** A run over the limit is recorded as every run is (the run, its
//   `userMessage`), then waits: a `notice` node `{code: SESSION_QUEUED, kind, limit, position}`,
//   and its provider turn starts when a slot frees, oldest first per kind. *As built:* the
//   queued run's session reads `running` (the run exists; the plan's "waits in `idle`" would
//   leave `agent.session.prompt` nothing to answer), and its budget's run clock starts only
//   when the turn does.
// - **Kinds:** `builtin` (the built-in loop, and the test-only `fake`, which runs in this process
//   too) and `provider` (a CLI harness's process, 7D/7E).
// - **What counts:** sessions in a run whose turn was admitted: the store's `running` and
//   `waitingApproval` sessions minus the queued ones. A queued run that is interrupted or ended
//   leaves the queue (`drop`, and `next` skips what is no longer the session's active run).
// - Steers to a queued run are recorded (their `userMessage` nodes) and held; they are steered
//   into its turn once it starts, as a steer into a running turn is.
// - In memory only: a restart interrupts every running session, queued ones too.
import type { Harness } from "@finisher/contracts";

export type LimitKind = "builtin" | "provider";

export const limitKindOf = (harness: Harness | string): LimitKind => (harness === "builtin" || harness === "fake" ? "builtin" : "provider");

export interface QueuedRun {
  readonly sessionId: string;
  readonly runId: string;
  readonly kind: LimitKind;
  /** The first turn's text, then every steer held for it. */
  readonly texts: string[];
}

export interface LiveLimitDeps {
  readonly max: () => { readonly builtin: number; readonly provider: number };
  /** The sessions in a run now (`SessionTree.active`). */
  readonly active: () => ReadonlyArray<{ readonly id: string; readonly harness: string }>;
  /** Whether `runId` is still the session's active run. */
  readonly current: (sessionId: string, runId: string) => boolean;
}

export class LiveLimits {
  readonly #d: LiveLimitDeps;
  readonly #queue: QueuedRun[] = [];

  constructor(deps: LiveLimitDeps) {
    this.#d = deps;
  }

  #admitted(kind: LimitKind, except?: string): number {
    const queued = new Set(this.#queue.map((q) => q.sessionId));
    return this.#d.active().filter((a) => limitKindOf(a.harness) === kind && a.id !== except && !queued.has(a.id)).length;
  }

  #limit(kind: LimitKind): number {
    return this.#d.max()[kind];
  }

  /**
   * Whether the run `runId` of `sessionId` (already `running` in the store) may start its turn
   * now; when not, it is queued, and the answer says where.
   */
  admit(sessionId: string, harness: string, runId: string, text: string): { admitted: true } | { admitted: false; kind: LimitKind; limit: number; position: number } {
    const kind = limitKindOf(harness);
    const limit = this.#limit(kind);
    const waiting = this.#queue.some((q) => q.kind === kind);
    if (!waiting && this.#admitted(kind, sessionId) < limit) return { admitted: true };
    this.#queue.push({ sessionId, runId, kind, texts: [text] });
    return { admitted: false, kind, limit, position: this.#queue.filter((q) => q.kind === kind).length };
  }

  /** Holds a steer for the session's queued run; false when it has none. */
  hold(sessionId: string, text: string): boolean {
    const q = this.#queue.find((x) => x.sessionId === sessionId);
    if (q === undefined) return false;
    q.texts.push(text);
    return true;
  }

  isQueued(sessionId: string): boolean {
    return this.#queue.some((q) => q.sessionId === sessionId);
  }

  /** The session's queued run leaves the queue (it was interrupted or ended). */
  drop(sessionId: string): void {
    const i = this.#queue.findIndex((q) => q.sessionId === sessionId);
    if (i >= 0) this.#queue.splice(i, 1);
  }

  /** The queued runs that may start now, oldest first per kind; they leave the queue. */
  next(): QueuedRun[] {
    const out: QueuedRun[] = [];
    for (const kind of ["builtin", "provider"] as const) {
      for (;;) {
        const i = this.#queue.findIndex((q) => q.kind === kind);
        if (i < 0) break;
        const q = this.#queue[i]!;
        if (!this.#d.current(q.sessionId, q.runId)) {
          this.#queue.splice(i, 1);
          continue;
        }
        if (this.#admitted(kind) >= this.#limit(kind)) break;
        this.#queue.splice(i, 1);
        out.push(q);
      }
    }
    return out;
  }
}
