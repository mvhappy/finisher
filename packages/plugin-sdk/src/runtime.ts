// The job loop of a plugin process (plan 8B Decisions 2, 7, 21): `hello` (`v` 1, this SDK's
// minor, family `plugin`, no `envelope` hash), then one job at a time. A `plugin.command` v1
// job's params are a `CommandRun` and its `extra` the `PluginContext`; the handler's
// `{tx}`/`{data}` becomes the `CommandOut` result. After each job a process started with a
// trailing `--warm` sends `ready`; any other exits. `alive` goes out every `limits.aliveMs` once
// a job has started. `cancel` aborts the job's signal and fails its pending calls; the job ends
// `JOB_CANCELLED` when the handler settles, or after `CANCEL_GRACE_MS` with the process exiting
// (a handler that ignores its signal is not trusted to stop). `shutdown` and stdin EOF end the
// process. An unknown host message type is dropped (a newer host's; never fatal).
import type { CommandOut, CommandRun, HostMethod, JobError, Op, PluginContext, Start } from "@finisher/core-types";
import { Envelope, type EnvelopeIo, type Incoming, LineTooLong, SDK_MINOR } from "./envelope.ts";
import { cancelledError, fromJobError, type PluginError, protocolError, toPluginError } from "./errors.ts";
import { Host } from "./host.ts";

/** The job kind of a command run and its version (`fin_plugin_api::protocol::COMMAND_KIND`). */
export const COMMAND_KIND = "plugin.command";
export const COMMAND_KIND_VERSION = 1;
/** How long a cancelled handler may take to settle before the process gives up on it. */
export const CANCEL_GRACE_MS = 1500;

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

  const env = new Envelope(io, onMessage, () => stop(0));
  env.send({ t: "hello", v: 1, minor: SDK_MINOR, role: "worker", family: "plugin", kinds: [{ kind: COMMAND_KIND, version: COMMAND_KIND_VERSION }], warm, pid: process.pid });

  function stop(code: number): void {
    if (alive !== undefined) clearInterval(alive);
    if (job !== undefined) end(job);
    env.close(code);
  }

  function end(j: Running): void {
    j.ended = true;
    if (!j.abort.signal.aborted) j.abort.abort(cancelledError());
    for (const id of j.calls) {
      pending.get(id)?.reject(cancelledError());
      pending.delete(id);
    }
    j.calls.clear();
  }

  /** Sends a job's result, or `PLUGIN_PROTOCOL {reason: tooLarge}` when it passes the line cap. */
  function sendResult(id: string, value: CommandOut): void {
    try {
      env.send({ t: "result", job: id, value });
    } catch (e) {
      if (!(e instanceof LineTooLong)) throw e;
      env.send({ t: "error", job: id, error: protocolError("the command's result is too large for one envelope line", { reason: "tooLarge", bytes: e.bytes }).toJobError() });
    }
  }

  function call(j: Running, method: HostMethod, params: unknown): Promise<unknown> {
    if (j.ended || j.abort.signal.aborted) return Promise.reject(cancelledError());
    const id = ++nextCall;
    return new Promise((resolve, reject) => {
      try {
        env.send({ t: "call", id, method, params });
      } catch (e) {
        if (!(e instanceof LineTooLong)) throw e;
        return reject(protocolError("a host call is too large for one envelope line", { reason: "tooLarge", method, bytes: e.bytes }));
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
    if (j.cancelled) env.send({ t: "error", job: j.id, error: cancelledError().toJobError() });
    else if ("error" in outcome) env.send({ t: "error", job: j.id, error: outcome.error });
    else sendResult(j.id, outcome.value);
    if (exit) stop(0);
    else env.send({ t: "ready" });
  }

  async function serve(j: Running, start: Start): Promise<void> {
    const sj = start.job;
    if (sj.kind !== COMMAND_KIND || sj.kindVersion !== COMMAND_KIND_VERSION) {
      throw protocolError(`this plugin serves ${COMMAND_KIND} v${COMMAND_KIND_VERSION} only`, { reason: "unknownKind", kind: sj.kind, kindVersion: sj.kindVersion });
    }
    const run = sj.params as CommandRun;
    if (typeof run !== "object" || run === null || typeof run.command !== "string" || typeof run.rev !== "number") {
      throw protocolError("a command job's params are not a command run", { reason: "badParams" });
    }
    const ctx = sj.extra as PluginContext | undefined;
    if (typeof ctx !== "object" || ctx === null || typeof ctx.limits !== "object") {
      throw protocolError("a command job carries no plugin context", { reason: "noContext" });
    }
    const handler = Object.hasOwn(commands, run.command) ? commands[run.command] : undefined;
    if (handler === undefined) throw protocolError(`this plugin has no command ${JSON.stringify(run.command)}`, { reason: "unknownCommand", command: run.command });
    const host = new Host(ctx, run, j.abort.signal, (m, p) => call(j, m, p));
    return void finish(j, { value: toCommandOut(await handler(run.args, host)) });
  }

  function onStart(start: Start): void {
    if (job !== undefined) {
      env.send({ t: "error", job: start.job.id, error: protocolError("a job started while another runs", { reason: "busy" }).toJobError() });
      return;
    }
    const aliveMs = start.limits?.aliveMs;
    if (alive === undefined && typeof aliveMs === "number" && aliveMs > 0) {
      alive = setInterval(() => env.send({ t: "alive" }), aliveMs);
      alive.unref();
    }
    const j: Running = { id: start.job.id, abort: new AbortController(), calls: new Set(), cancelled: false, ended: false };
    job = j;
    serve(j, start).catch((e: unknown) => finish(j, { error: toPluginError(e).toJobError() }));
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
    const grace = setTimeout(() => finish(j, { error: cancelledError().toJobError() }, true), CANCEL_GRACE_MS);
    grace.unref();
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

