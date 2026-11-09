// The job loop of a plugin process (plan 8B Decisions 2, 7, 21): `hello` (`v` 1, this SDK's
// minor, family `plugin`, no `envelope` hash), then one job at a time. A `plugin.command` v1
// job's params are a `CommandRun` and its `extra` the `PluginContext`; the handler's
// `{tx}`/`{data}` becomes the `CommandOut` result. After each job a process started with a
// trailing `--warm` sends `ready`; any other exits. `alive` goes out every `ALIVE_MS` from
// `hello`, and every `limits.aliveMs` of the latest `start` from then on (as fin-jobs' worker).
// `cancel` aborts the job's signal and fails its pending calls; the job ends `JOB_CANCELLED`
// when the handler settles, or after `CANCEL_GRACE_MS` with the process exiting (a handler that
// ignores its signal is not trusted to stop). `shutdown` ends the process with 0, stdin EOF
// with 70 (the host is gone). An unknown host message type is dropped (a newer host's; never
// fatal).
//
// Every job is answered: a result that does not serialise (a BigInt, a cycle) is
// `PLUGIN_PROTOCOL {reason: badResult}`, one over the line cap `{reason: tooLarge, bytes}`; an
// error line that does not fit (or serialise) is sent again with its message cut to 1 KiB and
// no details, and failing that as a fixed `PLUGIN_PROTOCOL {reason: tooLarge, bytes}`.
import type { CommandOut, CommandRun, HostMethod, JobError, Op, PluginContext, Start, WorkerMsg } from "@finisher/core-types";
import { EXIT_HOST_GONE, Envelope, type EnvelopeIo, type Incoming, LineTooLong, SDK_MINOR } from "./envelope.ts";
import { cancelledError, codes, fromJobError, type PluginError, protocolError, toPluginError } from "./errors.ts";
import { Host } from "./host.ts";

/** The job kind of a command run and its version (`fin_plugin_api::protocol::COMMAND_KIND`). */
export const COMMAND_KIND = "plugin.command";
export const COMMAND_KIND_VERSION = 1;
/** How long a cancelled handler may take to settle before the process gives up on it. */
export const CANCEL_GRACE_MS = 1500;
/** The `alive` interval from `hello` until the first `start` sets its own (fin-jobs' default). */
export const ALIVE_MS = 5000;
/** The longest message an error is cut to when its line does not fit. */
const SHORT_MESSAGE_BYTES = 1024;

/** What a command handler returns: ops to commit (`writes: true` commands) or data. */
export type CommandResult = { readonly tx: Op[] } | { readonly data: unknown } | undefined | void;

/** A command handler: `args` are already checked against the manifest's schema by the host. */
export type CommandHandler = (args: any, host: Host) => CommandResult | Promise<CommandResult>;

export interface PluginDefinition {
  /** Handlers by the command's name in the manifest. */
  readonly commands?: { readonly [name: string]: CommandHandler };
}

/** The `CommandOut` a handler's return value is sent as. */
export function toCommandOut(r: CommandResult): CommandOut {
  if (r === undefined || r === null) return { kind: "data", value: null };
  if (typeof r === "object" && "tx" in r && Array.isArray(r.tx)) return { kind: "tx", ops: r.tx };
  if (typeof r === "object" && "data" in r) return { kind: "data", value: r.data ?? null };
  throw protocolError("a command returned neither {tx} nor {data}", { reason: "badResult" });
}

interface Running {
  readonly id: string;
  readonly abort: AbortController;
  readonly calls: Set<number>;
  /** The host cancelled it: it ends `JOB_CANCELLED` whatever the handler does. */
  cancelled: boolean;
  ended: boolean;
  /** The cancel's grace timer, cleared when the job ends first. */
  grace?: ReturnType<typeof setTimeout>;
}

interface Pending {
  readonly job: Running;
  readonly resolve: (v: unknown) => void;
  readonly reject: (e: PluginError) => void;
}

/** Runs `def` over `io` (the process's stdio unless a test passes its own); `argv` decides `--warm`. */
export function runPlugin(def: PluginDefinition, io: EnvelopeIo, argv: ReadonlyArray<string>): void {
  const warm = argv.at(-1) === "--warm";
  const commands = def.commands ?? {};
  const pending = new Map<number, Pending>();
  let nextCall = 0;
  let job: Running | undefined;
  let alive: ReturnType<typeof setInterval> | undefined;

  const env = new Envelope(io, onMessage, () => stop(EXIT_HOST_GONE));
  env.send({ t: "hello", v: 1, minor: SDK_MINOR, role: "worker", family: "plugin", kinds: [{ kind: COMMAND_KIND, version: COMMAND_KIND_VERSION }], warm, pid: process.pid });
  setAlive(ALIVE_MS);

  function setAlive(ms: number): void {
    if (alive !== undefined) clearInterval(alive);
    alive = setInterval(() => trySend({ t: "alive" }), ms);
    alive.unref();
  }

  function stop(code: number): void {
    if (alive !== undefined) clearInterval(alive);
    if (job !== undefined) end(job);
    env.close(code);
  }

  function end(j: Running): void {
    j.ended = true;
    if (j.grace !== undefined) clearTimeout(j.grace);
    if (!j.abort.signal.aborted) j.abort.abort(cancelledError());
    for (const id of j.calls) {
      pending.get(id)?.reject(cancelledError());
      pending.delete(id);
    }
    j.calls.clear();
  }

  /** Sends `msg`; what stops it (`LineTooLong`, JSON's TypeError on a BigInt or a cycle) is returned, never thrown. */
  function trySend(msg: WorkerMsg): { readonly failure: unknown } | undefined {
    try {
      env.send(msg);
      return undefined;
    } catch (failure) {
      return { failure };
    }
  }

  const tooLarge = (bytes: number): JobError =>
    protocolError("the command's outcome is too large for one envelope line", { reason: "tooLarge", bytes }).toJobError();

  /** Sends a job's result; one that does not serialise is `badResult`, one past the cap `tooLarge`. */
  function sendResult(id: string, value: CommandOut): void {
    const sent = trySend({ t: "result", job: id, value });
    if (sent === undefined) return;
    if (sent.failure instanceof LineTooLong) return sendError(id, tooLarge(sent.failure.bytes));
    sendError(id, protocolError("the command's result cannot be serialised as JSON", { reason: "badResult" }).toJobError());
  }

  /** Sends a job's error; failing that, its code and a message cut to 1 KiB; failing that, `tooLarge`. */
  function sendError(id: string, error: JobError): void {
    const first = trySend({ t: "error", job: id, error });
    if (first === undefined) return;
    const code = typeof error.code === "string" ? error.code : codes.PLUGIN_COMMAND_FAILED;
    const second = trySend({ t: "error", job: id, error: { code, message: cut(String(error.message), SHORT_MESSAGE_BYTES) } });
    if (second === undefined) return;
    const bytes = first.failure instanceof LineTooLong ? first.failure.bytes : second.failure instanceof LineTooLong ? second.failure.bytes : 0;
    trySend({ t: "error", job: id, error: tooLarge(bytes) });
  }

  function call(j: Running, method: HostMethod, params: unknown): Promise<unknown> {
    if (j.ended || j.abort.signal.aborted) return Promise.reject(cancelledError());
    const id = ++nextCall;
    return new Promise((resolve, reject) => {
      const sent = trySend({ t: "call", id, method, params });
      if (sent !== undefined) {
        const e = sent.failure;
        if (e instanceof LineTooLong) return reject(protocolError("a host call is too large for one envelope line", { reason: "tooLarge", method, bytes: e.bytes }));
        return reject(toPluginError(e));
      }
      pending.set(id, { job: j, resolve, reject });
      j.calls.add(id);
    });
  }

  /** Ends `j` with `outcome` (a cancelled job always as `JOB_CANCELLED`); `ready` or exit next. */
  function finish(j: Running, outcome: { readonly error: JobError } | { readonly value: CommandOut }, exit = !warm): void {
    if (j.ended) return;
    end(j);
    job = undefined;
    if (j.cancelled) sendError(j.id, cancelledError().toJobError());
    else if ("error" in outcome) sendError(j.id, outcome.error);
    else sendResult(j.id, outcome.value);
    if (exit) stop(0);
    else trySend({ t: "ready" });
  }

  async function serve(j: Running, start: Start): Promise<void> {
    const sj = start.job;
    if (sj.kind !== COMMAND_KIND || sj.kindVersion !== COMMAND_KIND_VERSION) {
      throw protocolError(`this plugin serves ${COMMAND_KIND} v${COMMAND_KIND_VERSION} only`, { reason: "kind", kind: sj.kind, version: sj.kindVersion });
    }
    const run = sj.params as CommandRun;
    if (typeof run !== "object" || run === null || typeof run.command !== "string" || typeof run.rev !== "number") {
      throw protocolError("a command job's params are not a command run", { reason: "params" });
    }
    const ctx = sj.extra as PluginContext | undefined;
    if (typeof ctx !== "object" || ctx === null || typeof ctx.limits !== "object" || ctx.limits === null) {
      throw protocolError("a command job carries no plugin context", { reason: "context" });
    }
    const handler = Object.hasOwn(commands, run.command) ? commands[run.command] : undefined;
    if (handler === undefined) throw protocolError(`this plugin has no command ${JSON.stringify(run.command)}`, { reason: "unknownCommand", command: run.command });
    const host = new Host(ctx, run, j.abort.signal, (m, p) => call(j, m, p));
    return void finish(j, { value: toCommandOut(await handler(run.args, host)) });
  }

  function onStart(start: Start): void {
    if (job !== undefined) {
      sendError(start.job.id, protocolError("a job started while another runs", { reason: "busy" }).toJobError());
      return;
    }
    const aliveMs = start.limits.aliveMs;
    if (aliveMs > 0) setAlive(aliveMs);
    const j: Running = { id: start.job.id, abort: new AbortController(), calls: new Set(), cancelled: false, ended: false };
    job = j;
    serve(j, start).catch((e: unknown) => finish(j, { error: jobErrorOf(e) }));
  }

  function onCancel(id: string): void {
    const j = job;
    if (j === undefined || j.id !== id || j.cancelled) return;
    j.cancelled = true;
    j.abort.abort(cancelledError());
    for (const cid of j.calls) {
      pending.get(cid)?.reject(cancelledError());
      pending.delete(cid);
    }
    j.calls.clear();
    // A handler that does not settle in time is abandoned with its process.
    j.grace = setTimeout(() => finish(j, { error: cancelledError().toJobError() }, true), CANCEL_GRACE_MS);
    j.grace.unref();
  }

  function onMessage(m: Incoming): void {
    switch (m.t) {
      case "start":
        return onStart(m);
      case "cancel":
        return onCancel(m.job);
      case "reply": {
        const p = pending.get(m.id);
        if (p === undefined) return;
        pending.delete(m.id);
        p.job.calls.delete(m.id);
        if (m.error !== undefined) p.reject(fromJobError(m.error));
        else p.resolve(m.value);
        return;
      }
      case "shutdown":
        return stop(0);
      // `attach` is for helpers; an unknown type is a newer host's, dropped (Decision 21).
      default:
        return;
    }
  }
}

/** A thrown value as a `JobError`, even when reading it throws (a hostile getter). */
function jobErrorOf(e: unknown): JobError {
  try {
    return toPluginError(e).toJobError();
  } catch {
    return { code: codes.PLUGIN_COMMAND_FAILED, message: "the command failed" };
  }
}

/** `s` cut to at most `max` UTF-8 bytes, on a code point boundary. */
function cut(s: string, max: number): string {
  if (Buffer.byteLength(s) <= max) return s;
  let bytes = 0;
  let out = "";
  for (const ch of s) {
    bytes += Buffer.byteLength(ch);
    if (bytes > max) break;
    out += ch;
  }
  return out;
}

/**
 * Starts the plugin on this process's stdin and stdout. Call it once, at the top level of the
 * entry module: `definePlugin({ commands: { "trim-silence": async (args, host) => ({ tx }) } })`.
 */
export function definePlugin(def: PluginDefinition): PluginDefinition {
  runPlugin(
    def,
    {
      input: process.stdin,
      output: process.stdout,
      exit: (code) => process.exit(code),
      warn: (line) => process.stderr.write(`${line}\n`),
    },
    process.argv.slice(2),
  );
  return def;
}

