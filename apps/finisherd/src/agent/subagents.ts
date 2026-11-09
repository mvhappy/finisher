// Subagents (sub-project 7 §6.3; plan 7B Decisions 16–21): a running session's children, each
// its own session on a child branch, and what the runtime does for them after every transition.
//
// - **Spawn** (`agent.session.start` from an `agent` principal with a brief; the stage-4 check
//   of `reach.ts` admitted it): the parent must be in a run (`running|waitingApproval`; an idle
//   or interrupted one is `SESSION_INTERRUPTED`, an ended one or one a root's end is ending
//   `SESSION_ENDED`) with a root branch; below a root session
//   at most `agents.subagents.maxDepth` (3) deep and `.maxLive` (8) not ended, and at most the
//   parent role's `spawn.maxChildren` live children, else `DEPTH_EXCEEDED {kind: depth|live|
//   children}` (exit 7). The planner checks the role and the mode (`planner.ts`). The child's
//   root branch is created from the head of the parent's current default branch (its
//   `session_state`), named `<parent root name>.<role>-<n>` (`credentials.ts`), as the parent;
//   the child records `parent`, `rootSession` and `depth`; its credential is the mode's scopes
//   within the parent's. The payload names no parent and no `base`: an agent cannot attach a
//   session elsewhere. Spawns under one root are serialised, so two cannot both pass a limit,
//   and so is a cascade's count of the children: none is made after it counted. A parent that
//   stopped while its child was being opened stops the child as its cascade would, and the
//   spawn fails.
// - **A brief that ran to its end ends the child** (Decision 20): a child whose run completes
//   with `endTurn`, idle with no open approval, ends itself (`briefDone`); any run of a child,
//   since a child is idle only between runs its parent drives. Its summary is the final
//   assistant message of its last run (text blocks, at most `SUMMARY_MAX` characters).
// - **The parent hears it** as a `subagent` node `{child, status, reason?, branch?, summary?}`
//   in its active run (else its last one) when a child ends or is interrupted, never one the
//   parent's own end or interrupt caused; nothing is injected into a running turn (the parent
//   asks with `fin agent wait`).
// - **Cascade:** a parent that is interrupted, ended or deleted interrupts its live children
//   (they stay resumable by a person; in-flight tool calls finish unheard and get the
//   interrupt's synthetic results). A root session that ends (not deleted) ends every live
//   descendant **after its current tool call**: one with calls in flight goes on until they have
//   their results (recorded as any; its credential holds until its end), then ends
//   (`rootEnded`); past the stop grace (`stopGraceMs`, 30 s) it ends anyway and the calls get
//   the synthetic results. A session it is ending spawns nothing.
// - **Wait** (`agent.session.wait`, Decision 21): until every child (`any`: one) is done (idle,
//   interrupted or ended), at most 90 s, else `WAIT_TIMEOUT` (exit 5, repeatable); 4 waits at a
//   time per caller (`TOO_MANY_WATCHES`, exit 5).
import type { ChildResult, FinError, Principal, Session } from "@finisher/contracts";
import { SESSION_WAIT_MAX_MS } from "@finisher/contracts";
import { finError } from "../errors.ts";
import type { Clock } from "./clock.ts";
import type { LiveLimits, QueuedRun } from "./live-limits.ts";
import type { Role } from "./roles.ts";
import type { SessionLimits } from "./settings.ts";
import type { AgentStore, Applied, NewNode } from "./store.ts";

/** The longest summary a `subagent` node or a wait answer carries, in characters. */
export const SUMMARY_MAX = 8_000;
/** Waits at a time per caller. */
export const MAX_SESSION_WAITS = 4;
const WAIT_POLL_MS = 250;
/** The reasons of a cascade: the parent hears nothing of what it caused. */
const CASCADE = new Set(["parentInterrupted", "parentEnded", "rootEnded"]);

/** What `spawnChild` passes to the runtime's start for a child. */
export interface ChildOpen {
  readonly parent: string;
  readonly rootSession: string;
  readonly depth: number;
  /** The parent's current default branch (an id): the child's base, at its head. */
  readonly base: string;
  /** The parent's root branch: its name prefixes the child's. */
  readonly under?: string;
}

/** What the subagents ask of the runtime: each runs in the named session's queue. */
export interface SubagentOps {
  background(sessionId: string, what: string, f: () => Promise<void>): void;
  /** The session's tool calls without a result. */
  inFlight(sessionId: string): number;
  /** Ends the session (its provider interrupted first), unless it has ended. */
  endIn(sessionId: string, reason: string): Promise<void>;
  /** Interrupts the session, unless it is interrupted or ended. */
  interruptIn(sessionId: string, reason: string): Promise<void>;
  append(n: NewNode): Promise<void>;
  /** Starts a queued run's turn (`live-limits.ts`). */
  startQueued(q: QueuedRun): void;
}

export interface SubagentDeps {
  readonly store: AgentStore;
  readonly roles: ReadonlyMap<string, Role>;
  readonly limits: () => SessionLimits;
  readonly live: LiveLimits;
  readonly clock: Clock;
  /** How long a root's end waits for a descendant's calls in flight. */
  readonly graceMs: number;
  /** A test's pause after a child's open, before the spawn re-reads its parent. */
  readonly afterOpened?: (childId: string) => Promise<void>;
}

const depthExceeded = (message: string, details: Record<string, unknown>, fix: string): FinError =>
  finError("DEPTH_EXCEEDED", message, details, fix);

const notFound = (sessionId: string): FinError =>
  finError("SESSION_NOT_FOUND", "no such agent session", { sessionId }, "list your children with `fin agent ls`");

export class Subagents {
  readonly #d: SubagentDeps;
  readonly #spawns = new Map<string, Promise<unknown>>();
  readonly #wakers = new Set<() => void>();
  readonly #waits = new Map<string, number>();
  /** Sessions a root's end is ending once their calls in flight are done, with the grace's timer. */
  readonly #ending = new Map<string, ReturnType<typeof setTimeout>>();
  #ops: SubagentOps | undefined;

  constructor(deps: SubagentDeps) {
    this.#d = deps;
  }

  bind(ops: SubagentOps): void {
    this.#ops = ops;
  }

  close(): void {
    for (const t of this.#ending.values()) clearTimeout(t);
    this.#ending.clear();
  }

  // Spawning ---------------------------------------------------------------------------------

  /** Runs `f` after every earlier spawn under the same root session (the limits hold). */
  serialSpawn<A>(parent: Session, f: () => Promise<A>): Promise<A> {
    const root = parent.rootSession ?? parent.id;
    const prev = this.#spawns.get(root) ?? Promise.resolve();
    const done = prev.then(f, f);
    const tail = done.then(
      () => {},
      () => {},
    );
    this.#spawns.set(root, tail);
    void tail.then(() => {
      if (this.#spawns.get(root) === tail) this.#spawns.delete(root);
    });
    return done;
  }

  /** The checks of a spawn by `parent` (beside the planner's), and what the child is opened with. */
  childOf(parent: Session): ChildOpen {
    const { store } = this.#d;
    const stopped = this.#stopped(parent);
    if (stopped !== undefined) throw stopped;
    if (parent.rootBranch === undefined) {
      throw finError("MODE_FORBIDS", "a plan session has no branch to spawn children from", { mode: parent.mode }, "a draft or auto session spawns children");
    }
    const limits = this.#d.limits();
    const depth = (parent.depth ?? 0) + 1;
    if (depth > limits.maxDepth) {
      throw depthExceeded(`children go at most ${limits.maxDepth} deep below a root session`, { kind: "depth", depth, max: limits.maxDepth }, "do this yourself, or ask your parent to spawn it");
    }
    const rootSession = parent.rootSession ?? parent.id;
    const live = store.tree.liveDescendants(rootSession);
    if (live >= limits.maxLive) {
      throw depthExceeded(`this session tree already has ${live} live children, its most`, { kind: "live", live, max: limits.maxLive, rootSession }, "wait for children to finish (`fin agent wait`), or stop one");
    }
    const max = this.#d.roles.get(parent.role)?.spawn?.maxChildren;
    const mine = store.tree.liveChildren(parent.id).length;
    if (max !== undefined && mine >= max) {
      throw depthExceeded(`a ${parent.role} session has at most ${max} live children`, { kind: "children", children: mine, max, role: parent.role }, "wait for one to finish (`fin agent wait`), or stop one");
    }
    return { parent: parent.id, rootSession, depth, base: store.state(parent.id).defaultBranch, under: parent.rootBranch };
  }

  /** Why `parent` cannot spawn: it spawns from a run of its own that no root's end is ending. */
  #stopped(parent: Session | undefined): FinError | undefined {
    if (parent === undefined) return notFound("parent");
    const { status } = parent;
    if (status === "ended" || this.#ending.has(parent.id)) {
      return finError("SESSION_ENDED", status === "ended" ? "the session has ended" : "the session is ending: its root session ended", { status }, "start a new session");
    }
    if (status === "running" || status === "waitingApproval") return undefined;
    return finError("SESSION_INTERRUPTED", "a session spawns children from a run of its own", { status }, status === "interrupted" ? "a person resumes the session first" : "spawn from a run");
  }

  /** What a stopped `parent` does to a live child: a root's end ends it, anything else interrupts it. */
  #cascadeOf(parent: Session | undefined): { end: boolean; reason: string } {
    if (parent !== undefined && (this.#ending.has(parent.id) || parent.statusReason === "rootEnded" || (parent.status === "ended" && parent.parent === undefined && parent.statusReason !== "deleted"))) {
      return { end: true, reason: "rootEnded" };
    }
    return { end: false, reason: parent?.status === "interrupted" ? "parentInterrupted" : "parentEnded" };
  }

  /**
   * After a child's open, in its queue, the spawn's lock held: a parent that stopped meanwhile
   * (its cascade may have counted its children before this one existed) stops the child as the
   * cascade would, and the spawn fails. A parent whose run ended by itself keeps the child.
   */
  async adopt(parentId: string, childId: string): Promise<void> {
    await this.#d.afterOpened?.(childId);
    const p = this.#d.store.get(parentId);
    const stopped = p?.status === "idle" && !this.#ending.has(parentId) ? undefined : this.#stopped(p);
    if (stopped === undefined) return;
    const how = this.#cascadeOf(p);
    if (how.end) await this.#ops!.endIn(childId, how.reason);
    else await this.#ops!.interruptIn(childId, how.reason);
    throw stopped;
  }

  /** `id` ends once its calls in flight have their results, or once the grace is over. */
  #endAfterCalls(id: string): void {
    const ops = this.#ops!;
    if (this.#ending.has(id)) return;
    const timer = setTimeout(() => ops.background(id, "a root's end past its grace", async () => void (await this.settled(id))), this.#d.graceMs);
    timer.unref?.();
    this.#ending.set(id, timer);
    ops.background(id, "a root's end", async () => {
      if (ops.inFlight(id) === 0) await this.settled(id);
    });
  }

  /** The session's calls in flight have their results: an end waiting for them goes on. Whether one did. */
  async settled(id: string): Promise<boolean> {
    if (!this.#forget(id)) return false;
    await this.#ops!.endIn(id, "rootEnded");
    return true;
  }

  #forget(id: string): boolean {
    const timer = this.#ending.get(id);
    if (timer === undefined) return false;
    clearTimeout(timer);
    return this.#ending.delete(id);
  }

  // After each transition ---------------------------------------------------------------------

  /** What follows a transition of `applied.session` (the runtime's `after`, in its queue). */
  observe(applied: Applied): void {
    const ops = this.#ops;
    if (ops === undefined) return;
    const s = applied.session;
    const { store, live } = this.#d;
    if (s.status !== "running" && s.status !== "waitingApproval") live.drop(s.id);
    for (const q of live.next()) ops.startQueued(q);
    if (s.parent !== undefined) this.#wake();
    const phase = applied.effects.find((e) => e.kind === "journal")?.phase;
    // A brief that ran to its end.
    const closed = applied.effects.find((e) => e.kind === "closeRun");
    if (s.parent !== undefined && s.status === "idle" && closed?.kind === "closeRun" && closed.status === "completed" && closed.stop === "endTurn") {
      ops.background(s.id, "a brief's end", async () => {
        const now = store.get(s.id);
        if (now?.status !== "idle" || store.runs(s.id).at(-1)?.id !== closed.runId) return;
        if (store.approvals(s.id).some((a) => a.status === "open")) return;
        await ops.endIn(s.id, "briefDone");
      });
    }
    if (phase !== "ended" && phase !== "interrupted") return;
    // An end that waited for calls: the session stopped otherwise (its provider exited, a person).
    const ending = this.#ending.has(s.id);
    if (ending) {
      if (phase === "ended") this.#forget(s.id);
      else ops.background(s.id, "a root's end", async () => void (await this.settled(s.id)));
    }
    // The parent hears what became of its child.
    if (s.parent !== undefined && !CASCADE.has(s.statusReason ?? "")) {
      const parent = s.parent;
      ops.background(parent, "a child's result", async () => {
        const p = store.get(parent);
        const runId = p === undefined || p.status === "ended" ? undefined : (store.activeRun(parent) ?? store.runs(parent).at(-1)?.id);
        if (runId === undefined) return;
        const child = store.get(s.id);
        if (child !== undefined) await ops.append({ sessionId: parent, runId, kind: "subagent", payload: await this.result(child) });
      });
    }
    // The cascade, counted in the spawns' order: no child is made after the count. A session a
    // root's end is ending (or ended) leaves its descendants to that end.
    if (ending || s.statusReason === "rootEnded") return;
    const rootEnd = phase === "ended" && s.parent === undefined && s.statusReason !== "deleted";
    const reason = phase === "ended" ? "parentEnded" : "parentInterrupted";
    void this.serialSpawn(s, async () => {
      if (rootEnd) for (const id of store.tree.liveDescendantIds(s.id)) this.#endAfterCalls(id);
      else for (const id of store.tree.liveChildren(s.id)) ops.background(id, "a parent's stop", () => ops.interruptIn(id, reason));
    });
  }

  // Results and waits -------------------------------------------------------------------------

  /** The final assistant message of `sessionId`'s last run, text only, capped. */
  async summary(sessionId: string): Promise<string | undefined> {
    const { store } = this.#d;
    const seq = store.tree.lastAssistantSeq(sessionId);
    const node = seq === undefined ? undefined : store.nodes(sessionId, seq, 1)[0];
    if (node === undefined) return undefined;
    const last = store.runs(sessionId).at(-1);
    if (last !== undefined && node.runId !== last.id) return undefined;
    const payload = (await store.payload(node)) as { blocks?: ReadonlyArray<{ type?: unknown; text?: unknown }> } | null;
    const text = (payload?.blocks ?? []).flatMap((b) => (b.type === "text" && typeof b.text === "string" ? [b.text] : [])).join("\n");
    return text === "" ? undefined : text.slice(0, SUMMARY_MAX);
  }

  async result(child: Session): Promise<ChildResult> {
    const summary = await this.summary(child.id);
    return {
      child: child.id,
      status: child.status,
      ...(child.statusReason === undefined ? {} : { reason: child.statusReason }),
      ...(child.rootBranch === undefined ? {} : { branch: child.rootBranch }),
      ...(summary === undefined ? {} : { summary }),
    };
  }

  /** Done: idle after a run, interrupted or ended, and no run of it waits for a slot. */
  #done(s: Session): boolean {
    return (s.status === "idle" || s.status === "interrupted" || s.status === "ended") && !this.#d.live.isQueued(s.id);
  }

  #wake(): void {
    for (const w of [...this.#wakers]) w();
  }

  /**
   * `fin agent wait`: the children `ids` (each one the caller reaches; `reach.ts` has kept a
   * session to its own) once all (`any`: one) are done, else `WAIT_TIMEOUT` after `timeoutMs`.
   */
  async wait(by: Principal, ids: ReadonlyArray<string>, any: boolean, timeoutMs: number, signal?: AbortSignal): Promise<ChildResult[]> {
    const { store, clock } = this.#d;
    const seen = (id: string) => {
      const s = store.get(id);
      if (s === undefined || (by.projects !== "*" && !by.projects.includes(s.projectId))) throw notFound(id);
      return s;
    };
    for (const id of ids) seen(id);
    const key = `${by.kind}:${by.id}`;
    const waits = this.#waits.get(key) ?? 0;
    if (waits >= MAX_SESSION_WAITS) {
      throw finError("TOO_MANY_WATCHES", `${waits} waits are already running for this caller, its most`, { waits, limit: MAX_SESSION_WAITS }, "let a running wait end before starting another");
    }
    this.#waits.set(key, waits + 1);
    try {
      const ms = Math.max(0, Math.min(timeoutMs, SESSION_WAIT_MAX_MS));
      const deadline = clock.now() + ms;
      const realDeadline = Date.now() + ms;
      for (;;) {
        const now = ids.map(seen);
        const done = now.filter((s) => this.#done(s));
        if (any ? done.length > 0 : done.length === now.length) return Promise.all((any ? done : now).map((s) => this.result(s)));
        if (clock.now() >= deadline || Date.now() >= realDeadline || signal?.aborted === true) {
          throw finError(
            "WAIT_TIMEOUT",
            `${any ? "no child" : "not every child"} was done within ${ms} ms`,
            { ids: [...ids], timeoutMs: ms, children: now.map((s) => ({ child: s.id, status: s.status, ...(this.#d.live.isQueued(s.id) ? { queued: true } : {}) })) },
            "the children keep working: wait for them again (`fin agent wait`), or stop one",
          );
        }
        await this.#sleep(Math.max(1, Math.min(WAIT_POLL_MS, deadline - clock.now(), realDeadline - Date.now())), signal);
      }
    } finally {
      const left = (this.#waits.get(key) ?? 1) - 1;
      if (left <= 0) this.#waits.delete(key);
      else this.#waits.set(key, left);
    }
  }

  #sleep(ms: number, signal?: AbortSignal): Promise<void> {
    return new Promise<void>((resolve) => {
      const done = () => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", done);
        this.#wakers.delete(done);
        resolve();
      };
      const timer = setTimeout(done, ms);
      this.#wakers.add(done);
      signal?.addEventListener("abort", done, { once: true });
    });
  }
}
