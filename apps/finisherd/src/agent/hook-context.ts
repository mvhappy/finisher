// Hook context in a session's runs (7F plan Task 6, Decision 16): what the hooks add for the
// model, and their notices, per session, over the runtime's internals (`Runtime` passes them in).
//
// - **A run's start** (`open`, from `promptIn`): what is held, in the order it came (notices,
//   then each context as a `notice {hookContext: {event, hooks, text}}` node), then the prompt's
//   own `UserPrompt` context; the texts go to the provider after the person's text as one
//   `\n\n<hook-context>…</hook-context>` block. The `userMessage` node keeps the text as typed;
//   `historyFromNodes` joins the block back from the `hookContext` notices, so a resumed
//   built-in conversation is the one the model saw.
// - **Context of an event without a prompt** (`deliver`): while the session runs and its provider
//   takes steers, a steer of the block (a `userMessage {trigger: "hook"}` node), else held for the
//   next run. It never starts a run.
// - **Notices** go into the active run, else are held for the next. An ended session's are
//   logged and dropped; at most `HELD_MAX` items wait per session (the rest are logged).
import type { NodeKind, Session } from "@finisher/contracts";
import type { ContextPort, HookContextItem } from "../hooks/session.ts";
import { call, type Live } from "./runtime-live.ts";

export type { HookContextItem } from "../hooks/session.ts";

/** Items held per session at most. */
export const HELD_MAX = 64;

/** The block the provider gets after a prompt's text (or as a steer). */
export const hookContextBlock = (texts: readonly string[]): string => `<hook-context>\n${texts.join("\n\n")}\n</hook-context>`;

type Held = { readonly notice: Readonly<Record<string, unknown>> } | { readonly context: HookContextItem };

/** The runtime internals the context needs. */
export interface ContextInternals {
  session(sessionId: string): Session | undefined;
  activeRun(sessionId: string): string | null;
  live(sessionId: string): Live | undefined;
  append(n: { sessionId: string; runId: string; kind: NodeKind; payload: unknown }): Promise<unknown>;
  /** In the session's queue (`RuntimeEngine.background`). */
  background(sessionId: string, what: string, f: () => Promise<void>): void;
  log(message: string, details: Record<string, unknown>): void;
}

export class HookContexts {
  readonly #d: ContextInternals;
  readonly #held = new Map<string, Held[]>();

  constructor(d: ContextInternals) {
    this.#d = d;
  }

  /** The port the hooks' session bridge is bound to. */
  port(): ContextPort {
    return {
      session: (id) => this.#d.session(id),
      notice: (id, payload) => this.#notice(id, payload),
      steerHook: (id, item) => this.#d.background(id, "hook context", () => this.deliver(id, item)),
      hold: (id, item) => this.#hold(id, { context: item }),
    };
  }

  /** At a run's start: writes what is held and `extra`'s notice; answers the provider's suffix ("" for none). */
  async open(sessionId: string, runId: string, extra?: HookContextItem): Promise<string> {
    const items: Held[] = [...(this.#held.get(sessionId) ?? []), ...(extra === undefined ? [] : [{ context: extra }])];
    this.#held.delete(sessionId);
    const texts: string[] = [];
    for (const h of items) {
      if ("notice" in h) {
        await this.#d.append({ sessionId, runId, kind: "notice", payload: h.notice });
        continue;
      }
      const { event, hooks, text } = h.context;
      await this.#d.append({ sessionId, runId, kind: "notice", payload: { hookContext: { event, hooks, text } } });
      texts.push(text);
    }
    return texts.length === 0 ? "" : `\n\n${hookContextBlock(texts)}`;
  }

  /** Steers `item` into the running turn when its provider takes steers, else holds it (in the session's queue). */
  async deliver(sessionId: string, item: HookContextItem): Promise<void> {
    const s = this.#d.session(sessionId);
    const live = this.#d.live(sessionId);
    if ((s?.status === "running" || s?.status === "waitingApproval") && live?.runId != null && live.caps.steer) {
      const text = hookContextBlock([item.text]);
      await this.#d.append({ sessionId, runId: live.runId, kind: "userMessage", payload: { text, trigger: "hook", hookContext: { event: item.event, hooks: item.hooks } } });
      const err = await call(live.provider.steer({ text }));
      if (err !== null) this.#d.log("agent runtime: the provider refused hook context", { sessionId, code: err.code });
      return;
    }
    this.#hold(sessionId, { context: item });
  }

  /** `deliver`s `item` (when any), then answers `a`. */
  async deliverThen<A>(sessionId: string, item: HookContextItem | undefined, a: A): Promise<A> {
    if (item !== undefined) await this.deliver(sessionId, item);
    return a;
  }

  /** The session ended: what it held is dropped. */
  forget(sessionId: string): void {
    this.#held.delete(sessionId);
  }

  #notice(sessionId: string, payload: Readonly<Record<string, unknown>>): void {
    if (this.#d.activeRun(sessionId) === null) return this.#hold(sessionId, { notice: payload });
    this.#d.background(sessionId, "a hook notice", async () => {
      const runId = this.#d.activeRun(sessionId);
      if (runId === null) this.#hold(sessionId, { notice: payload });
      else await this.#d.append({ sessionId, runId, kind: "notice", payload });
    });
  }

  #hold(sessionId: string, h: Held): void {
    const s = this.#d.session(sessionId);
    const held = this.#held.get(sessionId) ?? [];
    if (s === undefined || s.status === "ended" || held.length >= HELD_MAX) {
      this.#d.log("agent runtime: hook context or notice dropped", { sessionId, reason: s === undefined || s.status === "ended" ? "ended" : "full", item: "notice" in h ? "notice" : "context" });
      return;
    }
    held.push(h);
    this.#held.set(sessionId, held);
  }
}
