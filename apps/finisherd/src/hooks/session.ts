// The session side of the hooks (7F plan Task 6; Decisions 1, 16, 18; spec §9.1): the session
// events the runtime fires, the engine's `SessionNotices`, and the runtime engine's tap.
//
// - **`UserPrompt`** (blocking before) fires once per person's prompt: at `agent.session.start
//   {prompt}` before anything is opened (no session yet, so a block creates nothing), and for
//   `prompt`, `steer` and `resume {prompt}` on an existing session. A child's brief never fires
//   it. A block throws `HOOK_BLOCKED`; its context comes back for the run the prompt starts.
// - **`SessionStart`** (awaited, never blocks) fires right after the open, before the first run;
//   its context, then the start's `UserPrompt` context, are held for that run (in memory only,
//   Owner question 7). **`SessionEnd`** (observe-only) fires once, at the `ended` phase.
// - **The tap** (Decision 18): `RuntimeEngine` hands every `statusChanged` and `approvalOpened`
//   it publishes and every 80 % budget notice here, watched or not, once at its source.
//   `Notification` fires for a status that differs from the session's last one (`match` names
//   `status.<status>`) and for a budget notice (`budget.<kind>`); `Approval` for a card (`match`
//   names its kind). Both are observe-only, queued off the session's path.
// - **Notices** (`SessionNotices.notice`) become `notice {hook: …}` nodes in the session's
//   active run, or are held for its next run (`ContextPort.notice`); the start's `UserPrompt`
//   notices, from before the session exists, are collected and held once it does. An event with
//   no session (a server event) only logs them.
// - **Context of an event without a prompt** (`SessionNotices.deliver`: a queued event's
//   context, and a `PostCommand` block's reason) reaches the agent as a steer with `trigger:
//   "hook"` while the session runs and its provider steers, else is held for its next run; it
//   never starts a run (`ContextPort.steerHook`, `agent/hook-context.ts`).
// - **Principals:** a prompt's and a start's are the caller's; the tap's and `SessionEnd`'s are
//   the session's own (its credential: the session is the one whose status or budget changed).
import type { Approval, HookEvent, Principal, Session, SessionStatus, WatchItem } from "@finisher/contracts";
import { Effect } from "effect";
import { type HookAt, type HookEngine, type HookNotice, type HookOutcome, hookBlockedError, type SessionNotices } from "./engine.ts";

/** Context for a session's model: the hooks' texts of one event (Decision 16). */
export interface HookContextItem {
  readonly event: HookEvent;
  readonly hooks: readonly string[];
  readonly text: string;
}

/** What the runtime gives the hooks (`Runtime` binds it at construction; `agent/hook-context.ts`). */
export interface ContextPort {
  session(sessionId: string): Session | undefined;
  /** A `notice` node in the session's active run, else held for its next run (dropped once ended). */
  notice(sessionId: string, payload: Readonly<Record<string, unknown>>): void;
  /** Steered into the running turn when its provider takes steers, else held for the next run; never starts one. */
  steerHook(sessionId: string, item: HookContextItem): void;
  /** Held for the session's next run. */
  hold(sessionId: string, item: HookContextItem): void;
}

/** A start's `UserPrompt`, before the session exists: its context and notices, held once it does. */
export interface StartPrompt {
  readonly context?: HookContextItem;
  readonly notices: readonly HookNotice[];
}

/** What the runtime calls (`RuntimeDeps.hooks`). */
export interface SessionHookPort {
  bind(port: ContextPort): void;
  /** `UserPrompt` before a start: `HOOK_BLOCKED` on a block. */
  startPrompt(projectId: string, by: Principal, text: string): Promise<StartPrompt>;
  /** `UserPrompt` on a session (`prompt`, `steer`, `resume`): `HOOK_BLOCKED` on a block; its context. */
  userPrompt(s: Session, by: Principal, text: string, origin: "prompt" | "steer"): Promise<HookContextItem | undefined>;
  /** `SessionStart`, awaited; its context, then `pre`'s, held for the first run. */
  sessionStart(s: Session, by: Principal, pre?: StartPrompt): Promise<void>;
  sessionEnd(s: Session): void;
}

/** The runtime engine's unconditional tap (`RuntimeDeps.sessionEvents`, Decision 18). */
export interface SessionEventTap {
  /** Every item `RuntimeEngine.publish` publishes (only `statusChanged` and `approvalOpened` are read). */
  published(sessionId: string, item: WatchItem): void;
  /** One 80 % budget notice, beside its node. */
  budget(sessionId: string, n: { readonly kind: string; readonly used: number; readonly limit: number }): void;
}

export interface SessionHooksDeps {
  readonly engine: () => HookEngine;
  readonly log?: (message: string) => void;
}

/** A session as the principal of its own events: its credential. */
export const sessionPrincipal = (s: Session): Principal => ({ kind: "agent", id: s.credentialId, scopes: [], projects: [s.projectId], branches: s.rootBranch === undefined ? [] : [s.rootBranch], human: false });

/** A notice's node payload: always `{hook: {event, id, layer, …}}`. */
export const noticePayload = (n: HookNotice): Record<string, unknown> =>
  "code" in n ? { hook: { event: n.event, id: n.hook, layer: n.layer, code: n.code, timeoutS: n.timeoutS } } : n;

/** The bridge between the engine and the runtime: `SessionHookPort`, the engine's `SessionNotices`, the tap. */
export class SessionHooks implements SessionHookPort, SessionNotices, SessionEventTap {
  readonly #d: SessionHooksDeps;
  #port: ContextPort | undefined;
  /** Each session's last status seen (a `Notification` fires when it differs). */
  readonly #status = new Map<string, SessionStatus>();
  /** Notices of a start's `UserPrompt`, by its `HookAt` (no session to write them to yet). */
  readonly #collect = new Map<HookAt, HookNotice[]>();

  constructor(d: SessionHooksDeps) {
    this.#d = d;
  }

  bind(port: ContextPort): void {
    this.#port = port;
  }

  async startPrompt(projectId: string, by: Principal, text: string): Promise<StartPrompt> {
    const at: HookAt = { projectId, principal: by };
    const notices: HookNotice[] = [];
    this.#collect.set(at, notices);
    try {
      return { ...(await this.#prompt(at, text, "start")), notices };
    } finally {
      this.#collect.delete(at);
    }
  }

  async userPrompt(s: Session, by: Principal, text: string, origin: "prompt" | "steer"): Promise<HookContextItem | undefined> {
    // An ended session refuses the prompt anyway: no hook runs for it.
    if (s.status === "ended") return undefined;
    return (await this.#prompt({ projectId: s.projectId, session: s, principal: by }, text, origin)).context;
  }

  async #prompt(at: HookAt, text: string, origin: "start" | "prompt" | "steer"): Promise<{ context?: HookContextItem }> {
    const out = await Effect.runPromise(this.#d.engine().fire("UserPrompt", at, { text, origin }));
    if (out.blocked !== undefined) throw hookBlockedError("UserPrompt", out.blocked);
    const context = itemOf("UserPrompt", out);
    return context === undefined ? {} : { context };
  }

  async sessionStart(s: Session, by: Principal, pre?: StartPrompt): Promise<void> {
    const port = this.#port;
    if (port === undefined) return;
    for (const n of pre?.notices ?? []) port.notice(s.id, noticePayload(n));
    const payload = { ...(s.parent === undefined ? {} : { parent: s.parent }), ...(s.trigger === undefined ? {} : { trigger: s.trigger }) };
    const out = await Effect.runPromise(this.#d.engine().fire("SessionStart", { projectId: s.projectId, session: s, principal: by }, payload));
    const context = itemOf("SessionStart", out);
    if (context !== undefined) port.hold(s.id, context);
    if (pre?.context !== undefined) port.hold(s.id, pre.context);
  }

  sessionEnd(s: Session): void {
    this.#status.delete(s.id);
    this.#d.engine().observe("SessionEnd", this.#at(s), { ...(s.statusReason === undefined ? {} : { reason: s.statusReason }) });
  }

  // The engine's `SessionNotices` --------------------------------------------------------------

  notice(at: HookAt, n: HookNotice): void {
    const collecting = this.#collect.get(at);
    if (collecting !== undefined) return void collecting.push(n);
    if (at.session === undefined || this.#port === undefined) return this.#log(`hooks: a notice of an event with no session: ${JSON.stringify(n)}`);
    this.#port.notice(at.session.id, noticePayload(n));
  }

  deliver(at: HookAt, event: HookEvent, outcome: HookOutcome): void {
    if (at.session === undefined || this.#port === undefined) return this.#log(`hooks: ${event} context with no session to deliver it to`);
    const context = itemOf(event, outcome);
    if (context !== undefined) this.#port.steerHook(at.session.id, context);
    const b = outcome.blocked;
    if (b !== undefined) this.#port.steerHook(at.session.id, { event, hooks: [b.hook], text: `The ${event} hook ${b.hook} blocked this: ${b.reason}` });
  }

  // The tap ------------------------------------------------------------------------------------

  published(sessionId: string, item: WatchItem): void {
    if (item.type !== "statusChanged" && item.type !== "approvalOpened") return;
    const s = this.#port?.session(sessionId);
    if (s === undefined) return;
    if (item.type === "approvalOpened") return this.#approval(s, item.approval);
    if (this.#status.get(sessionId) === item.status) return;
    if (item.status === "ended") this.#status.delete(sessionId);
    else this.#status.set(sessionId, item.status);
    const payload = { kind: "status", status: item.status, ...(item.reason === undefined ? {} : { reason: item.reason }), ...(item.runId === undefined ? {} : { runId: item.runId }) };
    this.#d.engine().observe("Notification", this.#at(s, `status.${item.status}`), payload);
  }

  budget(sessionId: string, n: { readonly kind: string; readonly used: number; readonly limit: number }): void {
    const s = this.#port?.session(sessionId);
    if (s === undefined) return;
    this.#d.engine().observe("Notification", this.#at(s, `budget.${n.kind}`), { kind: "budget", budget: n.kind, used: n.used, limit: n.limit, at: 0.8 });
  }

  #approval(s: Session, a: Approval): void {
    this.#d.engine().observe("Approval", this.#at(s, a.kind), { approval: a });
  }

  #at(s: Session, name?: string): HookAt {
    return { projectId: s.projectId, session: s, principal: sessionPrincipal(s), ...(name === undefined ? {} : { names: [name] }) };
  }

  #log(message: string): void {
    this.#d.log?.(message);
  }
}

const itemOf = (event: HookEvent, out: HookOutcome): HookContextItem | undefined =>
  out.context === undefined ? undefined : { event, hooks: out.contextHooks ?? [], text: out.context };
