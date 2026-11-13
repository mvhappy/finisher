// One Claude Code instance (plan 7D Task 7, Decisions 12, 13, 16): a `claude -p` process the
// adapter started through the session sandbox, its lines as `ProviderEvent`s bound to the
// instance, its turns, steers and interrupt, and its exit.
//
// - **The first-turn gate** (Decision 8, Review Focus 1): every line passes `firstTurnGate`, so
//   nothing reaches the runtime before the instance's `system/init` passed `checkInit`, the
//   adapter's own events (`turnStarted`, notices) included. A refused init fails the turn
//   (`PROVIDER_VERSION`) and kills the process group at once; a line that does not decode does
//   the same with `PROVIDER_PROTOCOL` (drift fails closed, Review Focus 6).
// - **Control lines**: the `finisher` SDK MCP server's handshake is answered at once (Task 0: it
//   starts before the `initialize` answer); every other CLI request (`can_use_tool`,
//   `hook_callback`, a newer CLI's) goes to one place, `RequestPort` (Task 8's
//   `claude/permissions.ts` fills it; the default here denies, failing closed); answers to the
//   host's own requests (`initialize`, `interrupt`) are matched by id.
// - **Turns** (Decision 13): `startTurn` writes the user line (the run's facts first) and the
//   turn ends at its `result` (`map.ts`); a steer is another user line.
// - **Interrupt** (Decision 16): the `interrupt` request, then up to `interruptWaitMs` (5 s) for
//   the turn's `result`; then SIGINT and up to `sigintGraceMs`; then the sandbox's group kill.
//   Never SIGTERM (it records no result).
// - **Exit**: the process's end (after its last line) is `exited {code, signal}`, then the
//   stream ends. Closing the instance's scope drops its later lines and kills it.
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { Cause, Effect, Queue, Stream } from "effect";
import { finError } from "../../errors.ts";
import type { BoundEvent, CallGate, ProjectedUsage, ProviderRef, ProviderSession, RuntimeAnswer, TurnInput, UserInput } from "../adapter.ts";
import type { SandboxedChild } from "../sandbox.ts";
import type { LaunchPlan } from "./launch.ts";
import { LineMapper, type UsageSource } from "./map.ts";
import { type CheckOptions, firstTurnGate, mcpReply } from "./probe-check.ts";
import { type CliLine, type CliRequest, decodeLine, encodeAnswer, encodeControl, encodeUser, lineFramer, type SystemInit } from "./protocol.ts";

/** Decision 16: how long the turn's `result` may take after the `interrupt` request. */
export const INTERRUPT_WAIT_MS = 5_000;
/** How long the turn's `result` (or the exit) may take after SIGINT, before the group kill. */
export const SIGINT_GRACE_MS = 2_000;

/**
 * Where the CLI's requests other than the MCP handshake go (`can_use_tool`, `hook_callback`, a
 * newer CLI's): the seam Task 8's `claude/permissions.ts` fills.
 */
export interface RequestPort {
  /** A CLI request: the events it becomes (a `request` for the runtime) and an answer line to write now. */
  request(requestId: string, r: CliRequest): { readonly events?: readonly ProviderEvent[]; readonly answer?: string };
  /** The runtime's answer to a `request` event: the line to write, or why there is none. */
  respond(id: string, a: RuntimeAnswer): string | FinError;
  /** The CLI withdrew `requestId` (`control_cancel_request`). */
  cancel(requestId: string): readonly ProviderEvent[];
}

/**
 * Task 7's requests: every one refused at once, so nothing runs that the runtime did not decide
 * (Review Focus 3). TODO(7D Task 8): `can_use_tool` becomes a `request` the runtime decides.
 */
export const REFUSE_REQUESTS: RequestPort = {
  request: (id, r) =>
    r.subtype === "can_use_tool"
      ? { answer: encodeAnswer(id, { behavior: "deny", message: "Finisher does not answer permission prompts on this server yet" }) }
      : { answer: encodeAnswer(id, { error: "not served by this host" }) },
  respond: (id) => finError("PROVIDER_PROTOCOL", "no Claude Code request is waiting for this answer", { id: id.slice(0, 64) }),
  cancel: () => [],
};

export interface InstanceOptions {
  /** This instance's id (a ULID): every event carries it. */
  readonly instance: string;
  readonly child: SandboxedChild;
  readonly plan: Pick<LaunchPlan, "argv" | "tools" | "mcpTools">;
  /** The session's UUID (`--session-id`/`--resume=`): every user line names it. */
  readonly uuid: string;
  /** The init predicate's options (`signedIn` as detection saw it). */
  readonly check?: CheckOptions;
  /** The adapter's own events (a dropped tool's notice), released with the first ones the gate lets through. */
  readonly notices?: readonly ProviderEvent[];
  /** The `initialize` answer's body (`control_response.response.response`). */
  readonly onInitialize?: (body: unknown) => void;
  /** The first init that passed the gate: the events it adds (an `untestedVersion` notice). */
  readonly onInit?: (init: SystemInit) => readonly ProviderEvent[];
  readonly requests?: RequestPort;
  readonly usage?: UsageSource;
  /** The runtime's budget check, once per turn (Task 9, Decision 15), with the turn's projection. */
  readonly gate?: CallGate;
  readonly projection?: () => ProjectedUsage;
  readonly interruptWaitMs?: number;
  readonly sigintGraceMs?: number;
  readonly ref: ProviderRef;
}

const INITIALIZE_ID = "finisher-initialize";

/** One running instance; `session()` is its `ProviderSession`. */
export class ClaudeInstance {
  readonly #o: InstanceOptions;
  readonly #queue: Queue.Queue<BoundEvent, Cause.Done>;
  readonly #gate: ReturnType<typeof firstTurnGate>;
  readonly #map: LineMapper;
  readonly #requests: RequestPort;
  /** The adapter's events waiting for the gate. */
  #pending: ProviderEvent[];
  #inited = false;
  #turnOpen = false;
  #interrupting = false;
  /** The gate reserved for the open turn and no `usage` has settled it yet. */
  #reserved = false;
  #closed = false;
  /** The gate refused the init, or a line did not decode: later lines are dropped. */
  #refused = false;
  #exit: { code: number | null; signal: string | null } | null = null;
  #wake: (() => void)[] = [];
  #control = 0;

  private constructor(o: InstanceOptions, queue: Queue.Queue<BoundEvent, Cause.Done>) {
    this.#o = o;
    this.#queue = queue;
    this.#gate = firstTurnGate(o.plan, o.check ?? {});
    this.#map = new LineMapper(o.usage);
    this.#requests = o.requests ?? REFUSE_REQUESTS;
    this.#pending = [...(o.notices ?? [])];
  }

  /** Starts reading `o.child` and sends `initialize` (Decision 11: every instance's models). */
  static start(o: InstanceOptions): Effect.Effect<ClaudeInstance> {
    return Effect.map(Queue.unbounded<BoundEvent, Cause.Done>(), (q) => {
      const i = new ClaudeInstance(o, q);
      i.#listen();
      i.#write(encodeControl(INITIALIZE_ID, { subtype: "initialize" }));
      return i;
    });
  }

  #listen(): void {
    const { child } = this.#o;
    const framer = lineFramer();
    const take = (raws: (string | FinError)[]) => {
      for (const raw of raws) this.#line(typeof raw === "string" ? decodeLine(raw) : raw);
    };
    const ended = new Promise<void>((resolve) => {
      child.stdout.on("data", (b: Buffer) => take(framer.push(b)));
      child.stdout.on("end", () => {
        take(framer.end());
        resolve();
      });
      child.stdout.on("error", () => resolve());
    });
    child.stderr.on("data", () => 0);
    child.stdin.on("error", () => 0);
    void child.exited.then(async (e) => {
      // Its last lines first; a descendant holding stdout open cannot hold the exit back long.
      await Promise.race([ended, new Promise((r) => setTimeout(r, 500).unref())]);
      this.#exit = { code: e.code, signal: e.signal ?? null };
      this.#turnOpen = false;
      this.#notify();
      if (!this.#closed) Queue.offerUnsafe(this.#queue, { instance: this.#o.instance, event: { type: "exited", code: e.code, signal: e.signal ?? null } });
      Queue.endUnsafe(this.#queue);
    });
  }

  #notify(): void {
    for (const w of this.#wake.splice(0)) w();
  }

  #write(line: string): boolean {
    if (this.#exit !== null || this.#closed) return false;
    try {
      this.#o.child.stdin.write(line);
      return true;
    } catch {
      return false;
    }
  }

  /** Events to the runtime now. */
  #deliver(events: readonly ProviderEvent[]): void {
    if (this.#closed) return;
    for (const event of events) Queue.offerUnsafe(this.#queue, { instance: this.#o.instance, event });
  }

  /** The adapter's own events: held until the gate lets the instance's lines through. */
  #emit(event: ProviderEvent): void {
    if (this.#gate.state === "open") this.#deliver([event]);
    else if (this.#gate.state === "waiting") this.#pending.push(event);
  }

  #flush(): void {
    if (this.#pending.length === 0) return;
    const p = this.#pending;
    this.#pending = [];
    this.#deliver(p);
  }

  /** The instance cannot go on: its turn fails with `error`, its group is killed at once. */
  #refuse(error: FinError): void {
    this.#pending = [];
    this.#deliver([{ type: "turnFailed", error: { code: error.code, message: error.message, details: error.details, ...(error.fix === undefined ? {} : { fix: error.fix }) } }]);
    this.#turnOpen = false;
    this.#refused = true;
    this.#notify();
    this.#o.child.kill("SIGKILL");
  }

  #line(l: CliLine | FinError): void {
    if (this.#closed || this.#refused) return;
    if (!("kind" in l)) return this.#refuse(l);
    // The SDK MCP handshake, at once, before and after the gate (Task 0).
    if (l.kind === "controlRequest" && l.request.subtype === "mcp_message") {
      this.#write(encodeAnswer(l.request_id, mcpReply(l.request.server_name, l.request.message, this.#o.plan.mcpTools)));
      return;
    }
    if (l.kind === "controlResponse") {
      if (l.response.request_id === INITIALIZE_ID && l.response.subtype === "success") this.#o.onInitialize?.(l.response.response);
      return;
    }
    if (l.kind === "controlCancel") {
      if (this.#gate.state === "open") this.#deliver(this.#requests.cancel(l.request_id));
      return;
    }
    const step = this.#gate.push(l);
    switch (step.kind) {
      case "hold":
      case "drop":
        return;
      case "refuse":
        return this.#refuse(step.error);
      case "forward":
        for (const f of step.lines) this.#forwarded(f);
    }
  }

  #forwarded(l: CliLine): void {
    if (l.kind === "controlRequest") {
      // The one place a CLI request is dispatched (Task 8: `can_use_tool` routing).
      const r = this.#requests.request(l.request_id, l.request);
      if (r.answer !== undefined) this.#write(r.answer);
      if (r.events !== undefined) this.#deliver(r.events);
      return;
    }
    this.#flush();
    if (l.kind === "init" && !this.#inited) {
      this.#inited = true;
      this.#deliver(this.#o.onInit?.(l) ?? []);
    }
    const events = this.#map.map(l, this.#interrupting);
    if (events.some((e) => e.type === "usage")) this.#reserved = false;
    this.#deliver(events);
    if (l.kind === "result" && (l.is_error || (l.queued_turn_count ?? 0) === 0)) {
      // A turn that reported no usage returns the gate's reservation (as the built-in loop does).
      if (this.#reserved) this.#o.gate?.release();
      this.#reserved = false;
      this.#turnOpen = false;
      this.#notify();
    }
  }

  /** Waits until the open turn ended or the process exited (true), or `ms` passed (false). */
  async #ended(ms: number): Promise<boolean> {
    const deadline = Date.now() + ms;
    for (;;) {
      if (!this.#turnOpen || this.#exit !== null) return true;
      const left = deadline - Date.now();
      if (left <= 0) return false;
      await new Promise<void>((resolve) => {
        const t = setTimeout(resolve, left);
        this.#wake.push(() => {
          clearTimeout(t);
          resolve();
        });
      });
    }
  }

  #gone(): FinError | null {
    if (this.#exit !== null) return finError("PROVIDER_EXITED", "the Claude Code process has exited", { code: this.#exit.code, signal: this.#exit.signal }, "resume the session");
    if (this.#closed || this.#refused) return finError("SESSION_INTERRUPTED", "this Claude Code instance is closed", { instance: this.#o.instance });
    return null;
  }

  /** Decision 16, in order: the request, the `result` (≤ 5 s), SIGINT, the group kill. */
  async #interrupt(): Promise<void> {
    if (!this.#turnOpen || this.#gone() !== null) return;
    const { child } = this.#o;
    this.#interrupting = true;
    // TODO(7D Task 0 Step 1): that the `interrupt` request ends the turn with a `result` (and
    // what the tool's process gets); modelled by fake-claude as an `error_during_execution`
    // result after the `control_response` (`onInterrupt: "result"`, the default).
    this.#write(encodeControl(`finisher-interrupt-${++this.#control}`, { subtype: "interrupt" }));
    if (await this.#ended(this.#o.interruptWaitMs ?? INTERRUPT_WAIT_MS)) return;
    // TODO(7D Task 0 Step 1): that SIGINT ends a turn the request did not; modelled by
    // fake-claude as the same `result` (`onSigint: "result"`), or an exit 130 (`"exit"`).
    child.kill("SIGINT");
    if (await this.#ended(this.#o.sigintGraceMs ?? SIGINT_GRACE_MS)) return;
    // The sandbox's group kill (SIGHUP under the Linux and macOS launchers, then SIGKILL).
    child.kill("SIGKILL");
  }

  /** Ends the instance (its scope closes): later lines are dropped, the process killed. */
  close(): void {
    if (!this.#closed) {
      this.#closed = true;
      this.#pending = [];
      this.#notify();
    }
    try {
      this.#o.child.stdin.end();
    } catch {
      // Already closed.
    }
    if (this.#exit === null) this.#o.child.kill("SIGKILL");
    Queue.endUnsafe(this.#queue);
  }

  session(): ProviderSession {
    const o = this.#o;
    const fail = (e: FinError) => Effect.fail(e);
    return {
      instance: o.instance,
      events: Stream.fromQueue(this.#queue),
      startTurn: (i: TurnInput) =>
        Effect.suspend(() => {
          const gone = this.#gone();
          if (gone !== null) return fail(gone);
          if (this.#turnOpen) return fail(finError("SESSION_BUSY", "a Claude Code turn is running", { instance: o.instance }));
          // Decision 15: the CLI's calls are invisible, so the gate is asked once per turn; the
          // turn's `usage` settles the reservation. A refusal ends the turn before any line.
          const g = o.gate?.beforeCall(o.projection?.() ?? { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 });
          if (g !== undefined && !g.ok) {
            const e = g.error;
            const capped = (e.details as { kind?: unknown } | null)?.kind === "modelCalls";
            this.#deliver([{ type: "turnStarted" }, capped ? { type: "turnCompleted", stopReason: "modelCallCap" } : { type: "turnFailed", error: { code: e.code, message: e.message, details: e.details ?? {}, ...(e.fix === undefined ? {} : { fix: e.fix }) } }]);
            return Effect.void;
          }
          this.#reserved = g !== undefined;
          this.#turnOpen = true;
          this.#interrupting = false;
          this.#emit({ type: "turnStarted" });
          this.#write(encodeUser([...(i.facts === undefined || i.facts === "" ? [] : [i.facts]), i.text], o.uuid));
          return Effect.void;
        }),
      steer: (i: UserInput) =>
        Effect.suspend(() => {
          const gone = this.#gone();
          if (gone !== null) return fail(gone);
          // Decision 13: another user line while the turn runs; the CLI queues it (see map.ts).
          this.#write(encodeUser([i.text], o.uuid));
          return Effect.void;
        }),
      interrupt: () => Effect.promise(() => this.#interrupt()),
      respond: (id: string, a: RuntimeAnswer) =>
        Effect.suspend(() => {
          const gone = this.#gone();
          if (gone !== null) return fail(gone);
          const line = this.#requests.respond(id, a);
          if (typeof line !== "string") return fail(line);
          this.#write(line);
          return Effect.void;
        }),
      // Spec §4.2: Finisher undoes the transactions; the CLI's history is left as it is (7B).
      rollback: () => fail(finError("PROVIDER_UNAVAILABLE", "Claude Code keeps its own history: nothing to roll back there", { harness: "claudeCode", reason: "noRollback" })),
      ref: () => o.ref,
    };
  }
}
