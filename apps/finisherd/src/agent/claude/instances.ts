// A Claude Code session's processes behind one `ProviderSession` (plan 7D Task 10; Decisions 12,
// 17).
//
// - **Idle close** (Decision 12): 7B's live limits count sessions in a run, not processes, so a
//   process whose session has had no turn for `agents.claudeCode.idleCloseS` (300 s) is closed
//   (stdin's end, `CLOSE_GRACE_MS`, then the kill: its transcript is written) while the session
//   stays `idle`; the next turn starts a new process with `--resume=<uuid>` in the same
//   `provider/` directory (`start(true)`). The runtime sees one instance throughout: the closed
//   process's lines are dropped (`ClaudeInstance` closed) and its end is not an exit.
// - **A real exit** (a crash, an exit mid-turn) is the instance's `exited`, and the stream ends:
//   the runtime fails the turn `PROVIDER_EXITED` and interrupts the session (resumable).
// - **One process at a time:** a new one starts only once the closed one is confirmed gone (its
//   exit seen and its end recorded in `provider.pid`; a kill alone is not an exit: under the
//   launchers the group kill sends SIGHUP and reaps later), so two processes never write one
//   transcript and the next one reads how the last one ended. A closed process still running
//   `exitWaitMs` after its kill fails the reopen (`PROVIDER_UNAVAILABLE {reason:
//   previousProcessAlive}`) instead of overlapping it; a later turn tries again.
// - **The resumed instance's first user line** carries the runtime's synthetic results
//   (`resume.ts`), once, ahead of the run's facts; once that line's write reports success a
//   `notice {interruptedDelivered}` records which calls it told, so a later resume does not tell
//   them again (and one that never wrote it does).
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../../errors.ts";
import type { BoundEvent, ProviderRef, ProviderSession, RuntimeAnswer, TurnInput, UserInput } from "../adapter.ts";
import type { ClaudeInstance, EventSink } from "./session.ts";

/** Decision 12's default: a session's process is closed after this long without a turn. */
export const IDLE_CLOSE_MS = 300_000;

export interface ClaudeSessionOptions {
  /** The session's one instance id, on every event of every process. */
  readonly instance: string;
  readonly ref: ProviderRef;
  /**
   * Starts a process writing to `sink`: `resume` false only for the session's first process
   * when the session had none; `rerun` answers whether a `tool_use_id` is an earlier process's.
   */
  readonly start: (o: { readonly resume: boolean; readonly sink: EventSink; readonly rerun: (toolUseId: string) => boolean }) => Effect.Effect<ClaudeInstance, FinError>;
  /** How long a process may sit with no turn before it is closed (≤ 0: never), read at each turn's end. */
  readonly idleCloseMs: () => number;
  /** Calls earlier instances announced (a resumed session's `toolCall` nodes). */
  readonly known?: ReadonlySet<string>;
  /** The first user line's leading block (`interruptedNote`). */
  readonly note?: string | undefined;
  /** The calls `note` tells of (their `tool_use_id`s): the `interruptedDelivered` notice's. */
  readonly told?: readonly string[];
  readonly log?: (message: string, details: Record<string, unknown>) => void;
}

const delivered = (toolUseIds: readonly string[]): ProviderEvent => ({ type: "notice", name: "interruptedDelivered", details: { harness: "claudeCode", toolUseIds: [...toolUseIds] } });

export class ClaudeSession {
  readonly #o: ClaudeSessionOptions;
  readonly #queue: Queue.Queue<BoundEvent, Cause.Done>;
  /** The running process, or null between an idle close and the next turn. */
  #current: ClaudeInstance | null = null;
  /** The current process's sink (made inert when it is closed for idleness). */
  #sink: { live: boolean } | null = null;
  /** The idle-closed process and its close (true: confirmed gone), awaited before the next one starts. */
  #closing: Promise<boolean> = Promise.resolve(true);
  #closed: ClaudeInstance | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #ended = false;
  #note: string | undefined;
  /** Every call any process announced, and the ones before the current process. */
  readonly #known: Set<string>;
  #before: ReadonlySet<string>;

  private constructor(o: ClaudeSessionOptions, queue: Queue.Queue<BoundEvent, Cause.Done>) {
    this.#o = o;
    this.#queue = queue;
    this.#note = o.note;
    this.#known = new Set(o.known ?? []);
    this.#before = new Set(this.#known);
  }

  /** Opens the session's first process (`resume`: the session has a provider reference). */
  static open(o: ClaudeSessionOptions, resume: boolean): Effect.Effect<ClaudeSession, FinError> {
    return Effect.flatMap(Queue.unbounded<BoundEvent, Cause.Done>(), (q) => {
      const s = new ClaudeSession(o, q);
      return Effect.as(s.#spawn(resume), s);
    });
  }

  #spawn(resume: boolean): Effect.Effect<ClaudeInstance, FinError> {
    const flag = { live: true };
    this.#before = new Set(this.#known);
    const before = this.#before;
    const sink: EventSink = {
      offer: (event) => {
        if (!flag.live || this.#ended) return;
        if (event.type === "toolStarted") this.#known.add(event.providerId);
        Queue.offerUnsafe(this.#queue, { instance: this.#o.instance, event });
        if (event.type === "turnCompleted" || event.type === "turnFailed") this.#arm();
      },
      end: () => {
        if (!flag.live) return;
        flag.live = false;
        this.#end();
      },
    };
    return Effect.map(this.#o.start({ resume, sink, rerun: (id) => before.has(id) }), (i) => {
      this.#current = i;
      this.#sink = flag;
      return i;
    });
  }

  #end(): void {
    if (this.#ended) return;
    this.#ended = true;
    this.#disarm();
    Queue.endUnsafe(this.#queue);
  }

  #disarm(): void {
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
  }

  /** After a turn's end: the idle close is scheduled (again). */
  #arm(): void {
    this.#disarm();
    const ms = this.#o.idleCloseMs();
    if (!(ms > 0) || this.#ended) return;
    this.#timer = setTimeout(() => this.#idle(), ms);
    this.#timer.unref?.();
  }

  #idle(): void {
    this.#timer = null;
    const i = this.#current;
    if (i === null || this.#ended) return;
    if (i.busy()) return this.#arm();
    // The process goes; the session does not: its end is not an exit, its later lines are dropped.
    this.#current = null;
    if (this.#sink !== null) this.#sink.live = false;
    this.#sink = null;
    this.#o.log?.("claude instance closed for idleness", { instance: this.#o.instance });
    this.#closed = i;
    this.#closing = i.close();
  }

  /** The process for a turn: the current one, or (after an idle close) a resumed one. */
  #process(): Effect.Effect<ClaudeInstance, FinError> {
    return Effect.suspend(() => {
      if (this.#ended) return Effect.fail(finError("SESSION_INTERRUPTED", "this Claude Code instance is closed", { instance: this.#o.instance }));
      if (this.#current !== null) return Effect.succeed(this.#current);
      return Effect.flatMap(
        Effect.promise(() => this.#closing),
        (gone) => {
          if (!gone && this.#closed !== null && !this.#closed.exited()) {
            this.#o.log?.("claude closed process still running", { instance: this.#o.instance });
            return Effect.fail(finError("PROVIDER_UNAVAILABLE", "the session's closed Claude Code process has not exited yet", { harness: "claudeCode", reason: "previousProcessAlive" }, "prompt again in a moment"));
          }
          return this.#spawn(true);
        },
      );
    });
  }

  /** The running process's session, or the instance's error when there is none. */
  #with<A>(f: (p: ProviderSession) => Effect.Effect<A, FinError>, none: Effect.Effect<A, FinError>): Effect.Effect<A, FinError> {
    return Effect.suspend(() => (this.#current === null ? none : f(this.#current.session())));
  }

  /** The scope's end: the timer stops, the process (if any) is closed, the stream ends. */
  async close(): Promise<void> {
    this.#disarm();
    const i = this.#current;
    this.#current = null;
    await this.#closing;
    if (i !== null) await i.close();
    this.#end();
  }

  session(): ProviderSession {
    const o = this.#o;
    const gone = Effect.fail(finError("SESSION_INTERRUPTED", "this Claude Code instance has no process", { instance: o.instance }));
    return {
      instance: o.instance,
      events: Stream.fromQueue(this.#queue),
      startTurn: (i: TurnInput) =>
        Effect.flatMap(this.#process(), (p) => {
          this.#disarm();
          const note = this.#note;
          const input = note === undefined ? i : { ...i, facts: i.facts === undefined || i.facts === "" ? note : `${note}\n\n${i.facts}` };
          return Effect.tap(p.session().startTurn(input), () =>
            Effect.sync(() => {
              // Told once: only once the user line's write reported success (a turn the gate
              // refused wrote none; an asynchronous EPIPE is no delivery), and the session's nodes
              // say so for a later resume (`resume.ts`). Until then the note stays due.
              if (note === undefined) return;
              const told = this.#o.told ?? [];
              void p.turnWritten().then((ok) => {
                if (!ok) return;
                if (this.#note === note) this.#note = undefined;
                if (told.length > 0) p.notice(delivered(told));
              });
            }),
          );
        }),
      steer: (i: UserInput) => this.#with((p) => p.steer(i), gone),
      interrupt: () => Effect.suspend(() => (this.#current === null ? Effect.void : this.#current.session().interrupt())),
      respond: (id: string, a: RuntimeAnswer) => this.#with((p) => p.respond(id, a), gone),
      rollback: (toTurn: number) => this.#with((p) => p.rollback(toTurn), Effect.fail(finError("PROVIDER_UNAVAILABLE", "Claude Code keeps its own history: nothing to roll back there", { harness: "claudeCode", reason: "noRollback" }))),
      ref: () => o.ref,
    };
  }

  /** For tests: whether a process is running now. */
  running(): boolean {
    return this.#current !== null;
  }
}
