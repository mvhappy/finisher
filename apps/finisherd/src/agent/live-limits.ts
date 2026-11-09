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
// - **The turn** (`enter`, `begin`): an admitted run's provider turn starts here, its budget's
//   run clock with it; a turn that cannot start fails the run (an `error` node, the session
//   `interrupted`).
import type { FinError, Harness, Session } from "@finisher/contracts";
import { toFinError } from "../errors.ts";
import { call, type Live } from "./runtime-live.ts";
import type { NewNode } from "./store.ts";

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
  readonly turns: TurnHost;
}

/** What starting a turn asks of the runtime (each call in the session's queue). */
export interface TurnHost {
  session(sessionId: string): Session;
  /** The session's provider instance, opened at need. */
  open(s: Session): Promise<Live>;
  /** The budget's run clock starts. */
  startClock(s: Session): void;
  facts(s: Session): string;
  append(n: NewNode): Promise<void>;
  interrupt(sessionId: string, reason: string): Promise<void>;
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

  /** A recorded run of `s`: its turn now, or queued with its `notice` node (never a failure). */
  async enter(s: Session, runId: string, text: string): Promise<void> {
    const admitted = this.admit(s.id, s.harness, runId, text);
    if (admitted.admitted) return this.begin({ sessionId: s.id, runId, kind: limitKindOf(s.harness), texts: [text] });
    const { kind, limit, position } = admitted;
    await this.#d.turns.append({ sessionId: s.id, runId, kind: "notice", payload: { code: "SESSION_QUEUED", kind, limit, position } });
  }

  /**
   * The run's provider turn (its budget's run clock starts here); a failure fails the run. The
   * steers held while it waited for a slot are steered in now, as any steer is.
   */
  async begin(q: QueuedRun): Promise<void> {
    const { sessionId, runId } = q;
    const h = this.#d.turns;
    if (!this.#d.current(sessionId, runId)) return;
    const s = h.session(sessionId);
    h.startClock(s);
    let live: Live;
    try {
      live = await h.open(s);
    } catch (e) {
      await this.#fail(sessionId, runId, toFinError(e));
      throw e;
    }
    live.runId = runId;
    const err = await call(live.provider.startTurn({ runId, text: q.texts[0]!, facts: h.facts(h.session(sessionId)) }));
    if (err) {
      live.runId = null;
      await this.#fail(sessionId, runId, err);
      throw err;
    }
    for (const t of q.texts.slice(1)) {
      if (!live.caps.steer) live.held.push(t);
      else if (await call(live.provider.steer({ text: t }))) live.held.push(t);
    }
  }

  /** The run could not start on its provider: an `error` node, and the session `interrupted`. */
  async #fail(sessionId: string, runId: string, e: FinError): Promise<void> {
    await this.#d.turns.append({ sessionId, runId, kind: "error", payload: { code: e.code, message: e.message, details: e.details ?? {} } });
    await this.#d.turns.interrupt(sessionId, e.code);
  }

  /** A steer to the session's queued run: recorded (its `userMessage`) and held; false when it has none. */
  async hold(sessionId: string, text: string): Promise<boolean> {
    const q = this.#queue.find((x) => x.sessionId === sessionId);
    if (q === undefined || !this.#d.current(sessionId, q.runId)) return false;
    q.texts.push(text);
    await this.#d.turns.append({ sessionId, runId: q.runId, kind: "userMessage", payload: { text, trigger: "steer" } });
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
