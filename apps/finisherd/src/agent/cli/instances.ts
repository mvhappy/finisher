// A CLI session's processes behind one `ProviderSession` (plan 7D Task 10; Decisions 12, 17;
// lifted from `claude/instances.ts` in plan 7E Task 1: the harness's own process is behind
// `CliInstance`, started by the adapter's `start(resume)`).
//
// - **Idle close** (Decision 12): 7B's live limits count sessions in a run, not processes, so a
//   process whose session has had no turn for `agents.<harness>.idleCloseS` (300 s) is closed
//   (the instance's `close`: Claude Code's stdin end, `CLOSE_GRACE_MS`, then the kill) while the
//   session stays `idle`; the next turn starts a new process resuming the same `provider/`
//   directory (`start(true)`). The runtime sees one instance throughout: the closed process's
//   events are dropped (its sink made inert) and its end is not an exit.
// - **A real exit** (a crash, an exit mid-turn) is the instance's `exited`, and the stream ends:
//   the runtime fails the turn `PROVIDER_EXITED` and interrupts the session (resumable).
// - **One process at a time:** a new one starts only once the closed one is confirmed gone (its
//   exit seen and its end recorded in `provider.pid`; a kill alone is not an exit: under the
//   launchers the group kill sends SIGHUP and reaps later), so two processes never write one
//   transcript and the next one reads how the last one ended. A closed process still running
//   `exitWaitMs` after its kill fails the reopen (`PROVIDER_UNAVAILABLE {reason:
//   previousProcessAlive}`) instead of overlapping it; a later turn tries again.
// - **The resumed instance's first user input** carries the runtime's synthetic results
//   (`resume.ts`), once, ahead of the run's facts; once that input's write reports success a
//   `notice {interruptedDelivered}` records which calls it told, so a later resume does not tell
//   them again (and one that never wrote it does).
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../../errors.ts";
import type { BoundEvent, ProviderRef, ProviderSession, RuntimeAnswer, TurnInput, UserInput } from "../adapter.ts";
import { type CliHarness, HARNESS_NAMES } from "./account.ts";

/** Where an instance's events go instead of its own stream (`CliSession`, 7D Task 10). */
export interface EventSink {
  offer(event: ProviderEvent): void;
  /** The process is gone (after its `exited`), or the instance closed. */
  end(): void;
}

/** One process of a CLI session, as the loop drives it (Claude Code's `ClaudeInstance`). */
export interface CliInstance {
  session(): ProviderSession;
  /** A turn is open, or a call is running: not closed for idleness yet. */
  busy(): boolean;
  /** Ends the process; true once it is confirmed gone. */
  close(): Promise<boolean>;
  exited(): boolean;
  /** Whether the last `startTurn`'s input was written (its write reported success). */
  turnWritten(): Promise<boolean>;
  /** Emits one of the adapter's own events on this instance. */
  notice(event: ProviderEvent): void;
}

/** Decision 12's default: a session's process is closed after this long without a turn. */
export const IDLE_CLOSE_MS = 300_000;

export interface CliSessionOptions<I extends CliInstance = CliInstance> {
  readonly harness: CliHarness;
  /** The session's one instance id, on every event of every process. */
  readonly instance: string;
  readonly ref: ProviderRef;
  /**
   * Starts a process writing to `sink`: `resume` false only for the session's first process
   * when the session had none; `rerun` answers whether a `tool_use_id` is an earlier process's.
   */
  readonly start: (o: { readonly resume: boolean; readonly sink: EventSink; readonly rerun: (toolUseId: string) => boolean }) => Effect.Effect<I, FinError>;
  /** How long a process may sit with no turn before it is closed (≤ 0: never), read at each turn's end. */
  readonly idleCloseMs: () => number;
  /** Calls earlier instances announced (a resumed session's `toolCall` nodes). */
  readonly known?: ReadonlySet<string>;
  /** The first user line's leading block (`interruptedNote`). */
  readonly note?: string | undefined;
  /** The calls `note` tells of (their `tool_use_id`s): the `interruptedDelivered` notice's. */
  readonly told?: readonly string[];
  readonly log?: (message: string, details: Record<string, unknown>) => void;
  /** `rollback` while no process runs (after an idle close). */
  readonly rollbackIdle: (toTurn: number) => Effect.Effect<void, FinError>;
}

const delivered = (harness: CliHarness, toolUseIds: readonly string[]): ProviderEvent => ({ type: "notice", name: "interruptedDelivered", details: { harness, toolUseIds: [...toolUseIds] } });
/** The log's word for each harness. */
const LOG_NAMES: Readonly<Record<CliHarness, string>> = { claudeCode: "claude", codex: "codex" };

export class CliSession<I extends CliInstance = CliInstance> {
  readonly #o: CliSessionOptions<I>;
  readonly #queue: Queue.Queue<BoundEvent, Cause.Done>;
  /** The running process, or null between an idle close and the next turn. */
  #current: I | null = null;
  /** The current process's sink (made inert when it is closed for idleness). */
  #sink: { live: boolean } | null = null;
  /** The idle-closed process and its close (true: confirmed gone), awaited before the next one starts. */
  #closing: Promise<boolean> = Promise.resolve(true);
  #closed: I | null = null;
  #timer: ReturnType<typeof setTimeout> | null = null;
  #ended = false;
  /** `close()` was called: no process starts from now on (one starting is closed instead). */
  #stopping = false;
  /** A process being started (its `start` not settled yet), awaited by `close()` and other turns. */
  #spawning: Promise<void> | null = null;
  #note: string | undefined;
  /** Every call any process announced, and the ones before the current process. */
  readonly #known: Set<string>;
  #before: ReadonlySet<string>;

  private constructor(o: CliSessionOptions<I>, queue: Queue.Queue<BoundEvent, Cause.Done>) {
    this.#o = o;
    this.#queue = queue;
    this.#note = o.note;
    this.#known = new Set(o.known ?? []);
    this.#before = new Set(this.#known);
  }

  /** Opens the session's first process (`resume`: the session has a provider reference). */
  static open<I extends CliInstance>(o: CliSessionOptions<I>, resume: boolean): Effect.Effect<CliSession<I>, FinError> {
    return Effect.flatMap(Queue.unbounded<BoundEvent, Cause.Done>(), (q) => {
      const s = new CliSession<I>(o, q);
      return Effect.as(s.#spawn(resume), s);
    });
  }

  #spawn(resume: boolean): Effect.Effect<I, FinError> {
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
    let settled!: () => void;
    const spawning = new Promise<void>((r) => (settled = r));
    this.#spawning = spawning;
    return Effect.flatMap(this.#o.start({ resume, sink, rerun: (id) => before.has(id) }), (i) => {
      if (this.#stopping || this.#ended) {
        // The session closed while this process started: it is closed too, never the current one.
        flag.live = false;
        return Effect.flatMap(
          Effect.promise(() => i.close()),
          (gone) => {
            if (!gone) this.#o.log?.(`${LOG_NAMES[this.#o.harness]} process still running after its close`, { instance: this.#o.instance });
            return Effect.fail(this.#interrupted());
          },
        );
      }
      this.#current = i;
      this.#sink = flag;
      return Effect.succeed(i);
    }).pipe(
      Effect.ensuring(
        Effect.sync(() => {
          if (this.#spawning === spawning) this.#spawning = null;
          settled();
        }),
      ),
    );
  }

  #interrupted(): FinError {
    return finError("SESSION_INTERRUPTED", `this ${HARNESS_NAMES[this.#o.harness]} instance is closed`, { instance: this.#o.instance });
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
    if (!(ms > 0) || this.#ended || this.#stopping) return;
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
    this.#o.log?.(`${LOG_NAMES[this.#o.harness]} instance closed for idleness`, { instance: this.#o.instance });
    this.#closed = i;
    this.#closing = i.close();
  }

  /** The process for a turn: the current one, or (after an idle close) a resumed one. */
  #process(): Effect.Effect<I, FinError> {
    return Effect.suspend(() => {
      if (this.#ended || this.#stopping) return Effect.fail(this.#interrupted());
      if (this.#current !== null) return Effect.succeed(this.#current);
      // Another turn is starting the process: this one takes it (or its failure) once it settles.
      const starting = this.#spawning;
      if (starting !== null) return Effect.flatMap(Effect.promise(() => starting), () => this.#process());
      return Effect.flatMap(
        Effect.promise(() => this.#closing),
        (gone) => {
          // Rechecked after the wait: the session may have closed, or another turn reopened it.
          if (this.#ended || this.#stopping) return Effect.fail(this.#interrupted());
          if (this.#current !== null) return Effect.succeed(this.#current);
          if (this.#spawning !== null) return this.#process();
          if (!gone && this.#closed !== null && !this.#closed.exited()) {
            this.#o.log?.(`${LOG_NAMES[this.#o.harness]} closed process still running`, { instance: this.#o.instance });
            return Effect.fail(finError("PROVIDER_UNAVAILABLE", `the session's closed ${HARNESS_NAMES[this.#o.harness]} process has not exited yet`, { harness: this.#o.harness, reason: "previousProcessAlive" }, "prompt again in a moment"));
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
    this.#stopping = true;
    this.#disarm();
    const closed = await this.#closing;
    // A process starting now is closed by its own start (`#stopping`); wait for that.
    while (this.#spawning !== null) await this.#spawning;
    const i = this.#current;
    this.#current = null;
    const gone = i === null ? closed || this.#closed === null || this.#closed.exited() : await i.close();
    // Still running after its wait: the next open refuses to start another beside it (the
    // adapter's `aliveProcesses` check over `provider.pid`).
    if (!gone) this.#o.log?.(`${LOG_NAMES[this.#o.harness]} process still running after its close`, { instance: this.#o.instance });
    this.#end();
  }

  session(): ProviderSession {
    const o = this.#o;
    const gone = Effect.fail(finError("SESSION_INTERRUPTED", `this ${HARNESS_NAMES[o.harness]} instance has no process`, { instance: o.instance }));
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
                if (told.length > 0) p.notice(delivered(this.#o.harness, told));
              });
            }),
          );
        }),
      steer: (i: UserInput) => this.#with((p) => p.steer(i), gone),
      interrupt: () => Effect.suspend(() => (this.#current === null ? Effect.void : this.#current.session().interrupt())),
      respond: (id: string, a: RuntimeAnswer) => this.#with((p) => p.respond(id, a), gone),
      rollback: (toTurn: number) => this.#with((p) => p.rollback(toTurn), o.rollbackIdle(toTurn)),
      ref: () => o.ref,
    };
  }

  /** For tests: whether a process is running now. */
  running(): boolean {
    return this.#current !== null;
  }
}
