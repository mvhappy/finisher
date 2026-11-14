// The brokered shell's tool (plan 7D Task 13, Decision 21): `shell {command, timeoutMs?}` of the
// `finisher` SDK MCP server (`mcp-host.ts` serves it), run by `finisherd` itself.
//
// - **Through the session sandbox only** (Review Focus 3, `scripts/check-agent-spawn.mjs`): every
//   call is a fresh process of the session's sandbox (`SessionSandbox.spawn`: the `agent`
//   profile's launcher, or `direct` where it is not ready), started in the session's workspace,
//   with the session environment plus `FIN_CALL` = the runtime's id of the call, so the `fin`
//   it runs is attributed `exact` (spec §12.3), never inferred. No helper runs inside the
//   container and no new pipe exists.
// - **One interface, two shells:** PowerShell on Windows (Git Bash cannot start in an
//   AppContainer, arch §27.6), `/bin/sh -c` elsewhere, so every case is tested on Linux.
// - **Output** is capped as the CLI's own `Bash` caps it (`BASH_MAX_OUTPUT_LENGTH`, default
//   30,000 characters): stdout keeps its first `SHELL_OUTPUT_MAX` characters, stderr its last
//   (`stderrTail`: an error is at the end); `truncated` says either was cut. A "character" is a
//   UTF-16 code unit (a JavaScript string's `length`, what the CLI's cap counts), and a cut
//   never splits a surrogate pair (`capOutput`): a pair across the boundary is dropped whole, so
//   a cut output may be one unit short and is always well formed. Only that much (in bytes,
//   four per character at most) is ever held; a UTF-8 sequence the byte hold splits decodes to
//   U+FFFD past the cut, never inside what is kept.
// - **Timeout** as the CLI's `Bash`: `timeoutMs` defaults to `BASH_DEFAULT_TIMEOUT_MS`' 120 s and
//   is capped at its 600 s maximum. At the timeout (or an abort: the CLI cancelled the call, the
//   turn ended, the instance closed) the sandbox kills the process's whole group (Windows: its
//   job object), so nothing the command started outlives it.
//
// TODO(7D Task 13 Steps 0 and 3, a Windows host): `pwsh.exe` resolved inside the session's
// container (the toolchain grant names its install), PowerShell's exit codes, and a `fin view`
// that runs and a write outside the grants that is refused there.
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import type { SessionSandbox } from "../sandbox.ts";

/** The most characters of stdout (its head) and of stderr (its tail) the CLI gets: the CLI's `Bash` default. */
export const SHELL_OUTPUT_MAX = 30_000;
/** `timeoutMs` when the call names none: the CLI's `BASH_DEFAULT_TIMEOUT_MS` (Decision 5 sets it so). */
export const SHELL_DEFAULT_TIMEOUT_MS = 120_000;
/** The longest a call may run: the CLI's `Bash` maximum (`BASH_MAX_TIMEOUT_MS`). */
export const SHELL_MAX_TIMEOUT_MS = 600_000;
/** How long the output may take to drain after the process ended (a descendant holding the pipe). */
const DRAIN_MS = 500;
/** UTF-8 takes at most four bytes a character: what is held of each stream. */
const HOLD_BYTES = SHELL_OUTPUT_MAX * 4;

/** What the CLI gets back. `exit` is null when the process was killed (`timedOut`, `cancelled`) or did not start. */
export interface ShellResult {
  readonly exit: number | null;
  readonly stdout: string;
  readonly stderrTail: string;
  readonly signal?: string;
  readonly timedOut?: true;
  readonly cancelled?: true;
  readonly truncated?: true;
}

/** The shell a command runs in. */
export interface Shell {
  readonly name: "pwsh" | "sh";
  argv(command: string): string[];
}

export const SH: Shell = { name: "sh", argv: (command) => ["/bin/sh", "-c", command] };
export const PWSH: Shell = { name: "pwsh", argv: (command) => ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command", command] };

/** PowerShell on Windows, `sh` elsewhere (Decision 21). */
export const shellFor = (os: NodeJS.Platform): Shell => (os === "win32" ? PWSH : SH);

export interface ShellInput {
  readonly command: string;
  readonly timeoutMs: number;
}

/** The call's arguments as a `ShellInput`, or why not (the tool's own error). */
export function shellInput(args: unknown): ShellInput | string {
  if (typeof args !== "object" || args === null || Array.isArray(args)) return "shell takes {command, timeoutMs?}";
  const a = args as Record<string, unknown>;
  const extra = Object.keys(a).filter((k) => k !== "command" && k !== "timeoutMs");
  if (extra.length > 0) return `shell takes {command, timeoutMs?}, not ${extra[0]!.slice(0, 32)}`;
  if (typeof a.command !== "string" || a.command === "") return "shell needs a command";
  if (a.timeoutMs === undefined) return { command: a.command, timeoutMs: SHELL_DEFAULT_TIMEOUT_MS };
  if (typeof a.timeoutMs !== "number" || !Number.isSafeInteger(a.timeoutMs) || a.timeoutMs < 1) return "timeoutMs is a whole number of milliseconds";
  return { command: a.command, timeoutMs: Math.min(a.timeoutMs, SHELL_MAX_TIMEOUT_MS) };
}

/** What the tool needs of the session sandbox. */
export type ShellBox = Pick<SessionSandbox, "workspace" | "spawn">;

export interface ShellToolOptions {
  readonly box: ShellBox;
  readonly os: NodeJS.Platform;
  /** Default `shellFor(os)`. */
  readonly shell?: Shell;
}

export interface ShellTool {
  /** Runs `input` as call `callId` (its `FIN_CALL`); never throws. */
  run(input: ShellInput, callId: string | undefined, signal: AbortSignal): Promise<ShellResult>;
}

const isHigh = (c: number) => c >= 0xd800 && c <= 0xdbff;
const isLow = (c: number) => c >= 0xdc00 && c <= 0xdfff;

/** `s`'s first (`head`) or last `SHELL_OUTPUT_MAX` UTF-16 code units, cut at a code point (module docs). */
export function capOutput(s: string, keep: "head" | "tail"): string {
  if (s.length <= SHELL_OUTPUT_MAX) return s;
  if (keep === "head") {
    const end = isHigh(s.charCodeAt(SHELL_OUTPUT_MAX - 1)) ? SHELL_OUTPUT_MAX - 1 : SHELL_OUTPUT_MAX;
    return s.slice(0, end);
  }
  const start = s.length - SHELL_OUTPUT_MAX;
  return s.slice(isLow(s.charCodeAt(start)) ? start + 1 : start);
}

/** Collects a stream's head (`keep: "head"`) or tail, at most `HOLD_BYTES`. */
function collector(keep: "head" | "tail") {
  let chunks: Buffer[] = [];
  let held = 0;
  let cut = false;
  return {
    push(b: Buffer) {
      if (keep === "head") {
        if (held >= HOLD_BYTES) return void (cut = true);
        const part = b.subarray(0, HOLD_BYTES - held);
        if (part.length < b.length) cut = true;
        chunks.push(part);
        held += part.length;
        return;
      }
      chunks.push(b);
      held += b.length;
      if (held > HOLD_BYTES * 2) {
        const all = Buffer.concat(chunks);
        chunks = [all.subarray(all.length - HOLD_BYTES)];
        held = HOLD_BYTES;
        cut = true;
      }
    },
    text(): { text: string; cut: boolean } {
      const s = Buffer.concat(chunks).toString("utf8");
      if (s.length <= SHELL_OUTPUT_MAX) return { text: s, cut };
      return { text: capOutput(s, keep), cut: true };
    },
  };
}

/** The brokered shell over `o.box` (one per instance). */
export function shellTool(o: ShellToolOptions): ShellTool {
  const shell = o.shell ?? shellFor(o.os);
  return {
    async run(input, callId, signal) {
      const spawned = await Effect.runPromise(Effect.result(o.box.spawn(shell.argv(input.command), { cwd: o.box.workspace, stdio: "pipe", env: callId === undefined ? {} : { FIN_CALL: callId } })));
      if (spawned._tag === "Failure") {
        const e = spawned.failure as FinError;
        return { exit: null, stdout: "", stderrTail: `${e.code}: ${e.message}`.slice(0, SHELL_OUTPUT_MAX) };
      }
      const child = spawned.success;
      const out = collector("head");
      const err = collector("tail");
      const pump = (stream: NodeJS.ReadableStream, c: ReturnType<typeof collector>) =>
        new Promise<void>((resolve) => {
          stream.on("data", (b: Buffer) => c.push(b));
          stream.on("end", resolve);
          stream.on("error", () => resolve());
        });
      const drained = Promise.all([pump(child.stdout, out), pump(child.stderr, err)]);
      child.stdin.on("error", () => 0);
      child.stdin.end();
      let stopped: "timedOut" | "cancelled" | undefined;
      const stop = (why: "timedOut" | "cancelled") => {
        if (stopped !== undefined) return;
        stopped = why;
        // The whole group (the sandbox's kill; the job object on Windows).
        child.kill("SIGKILL");
      };
      const timer = setTimeout(() => stop("timedOut"), input.timeoutMs);
      const onAbort = () => stop("cancelled");
      if (signal.aborted) onAbort();
      else signal.addEventListener("abort", onAbort, { once: true });
      try {
        const exit = await child.exited;
        await Promise.race([drained, new Promise((r) => setTimeout(r, DRAIN_MS).unref())]);
        const so = out.text();
        const se = err.text();
        return {
          exit: stopped === undefined ? exit.code : null,
          stdout: so.text,
          stderrTail: se.text,
          ...(stopped === undefined && exit.signal !== null && exit.signal !== undefined ? { signal: exit.signal } : {}),
          ...(stopped === undefined ? {} : { [stopped]: true as const }),
          ...(so.cut || se.cut ? { truncated: true as const } : {}),
        };
      } finally {
        clearTimeout(timer);
        signal.removeEventListener("abort", onAbort);
      }
    },
  };
}
