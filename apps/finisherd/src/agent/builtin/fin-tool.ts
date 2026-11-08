// The built-in agent's native `fin` tool (plan 7A Decisions 11, 14b, 14c; spec §4.3, §8.5).
//
// - **No shell:** `fin` runs with `execFile` semantics, argv as given (an argument such as
//   `; rm -rf x` reaches `fin` verbatim, which refuses it as usage), `cwd` the session's
//   workspace. `argv[0]` is a command word, never a path.
// - **Environment:** the session's (`childEnv` plus `FIN_SERVER`, `FIN_SESSION`, `FIN_CREDENTIAL`)
//   with `FIN_CALL` (this call's id) and `FIN_CALL_IDEM=1` (one `fin` per call, so `FIN_CALL`
//   is the write's idempotency key); `FIN_SERVER_DATA`, `FIN_AUTHOR` and `FIN_TRAINING` are
//   removed whatever the caller passed.
// - **Timeout:** 120 s (`agents.toolTimeoutS`), then the process tree is killed.
// - **Result:** the JSON `fin` printed (text when it printed something else), its exit code and
//   the last 1 KiB of stderr.
// - **Paths:** arguments the `fin schema commands` index marks as paths are checked before `fin`
//   starts (the same confinement `fin` applies itself under an agent credential, §5.6).
import { type ChildProcess, execFile, spawn } from "node:child_process";
import { realpath, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { NEVER_PASSED } from "../../env.ts";
import { finError } from "../../errors.ts";
import type { ToolOutcome } from "../runtime-deps.ts";

export const TOOL_TIMEOUT_MS = 120_000;
const STDERR_TAIL = 1024;
/** The most stdout kept (a larger answer is cut and marked). */
const STDOUT_MAX = 8 * 1024 * 1024;

/** Variables the `fin` tool never passes on (spec §8.5; Decision 14b), whatever its caller gave. */
export const REMOVED_VARS = [...NEVER_PASSED, "FIN_OFFLINE_SNAPSHOT", "FIN_CALL_IDEM"] as const;

export interface SessionEnv {
  readonly finPath: string;
  /** `workspace/<sessionId>/`. */
  readonly cwd: string;
  /** The session environment (`OpenEnvironment.env` with `FIN_CREDENTIAL`). */
  readonly env: Readonly<Record<string, string>>;
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
  /** Where a path argument may point: the workspace and the project folder. */
  readonly roots?: readonly string[];
  readonly index?: FinIndex;
  /** Compare paths case-insensitively (default: Windows and macOS). */
  readonly fold?: boolean;
}

/** One command of `fin schema commands`. */
export interface FinCommand {
  readonly name: string;
  readonly write: boolean;
  readonly paths: ReadonlyArray<{ readonly arg: string; readonly flag?: string; readonly position?: number | null }>;
}

/** What the tool knows of `fin`'s commands: which write, which take paths (argKinds). */
export class FinIndex {
  readonly #byName: ReadonlyMap<string, FinCommand>;

  constructor(commands: ReadonlyArray<FinCommand>) {
    this.#byName = new Map(commands.map((c) => [c.name, c]));
  }

  /** The command `argv` names: the longest run of leading words that is a command name. */
  command(argv: ReadonlyArray<string>): (FinCommand & { readonly words: number }) | undefined {
    let found: (FinCommand & { words: number }) | undefined;
    for (let n = 1; n <= Math.min(argv.length, 3); n++) {
      if (argv[n - 1]!.startsWith("-")) break;
      const c = this.#byName.get(argv.slice(0, n).join("."));
      if (c) found = { ...c, words: n };
    }
    return found;
  }
}

const indexes = new Map<string, Promise<FinIndex>>();

/** `fin schema commands`, once per `fin` binary (its path and modification time). */
export async function finIndex(finPath: string): Promise<FinIndex> {
  const st = await stat(finPath);
  const key = `${finPath}\0${st.mtimeMs}\0${st.size}`;
  let p = indexes.get(key);
  if (p === undefined) {
    p = new Promise<FinIndex>((ok, fail) => {
      execFile(finPath, ["schema", "commands"], { env: { PATH: process.env.PATH ?? "" }, timeout: 30_000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout) => {
        if (err) return fail(err);
        try {
          ok(new FinIndex((JSON.parse(stdout) as { commands: FinCommand[] }).commands));
        } catch (e) {
          fail(e);
        }
      });
    });
    indexes.set(key, p);
    p.catch(() => indexes.delete(key));
  }
  return p;
}

/** The tool's environment: the session's, plus the call, minus what it never passes. */
export function toolEnv(env: Readonly<Record<string, string>>, callId: string): Record<string, string> {
  const out: Record<string, string> = { ...env };
  for (const k of REMOVED_VARS) delete out[k];
  out.FIN_CALL = callId;
  out.FIN_CALL_IDEM = "1";
  return out;
}

const COMMAND_WORD = /^[a-z][a-z0-9-]*$/;

/** The path values of `argv` the index recognises (flags `--x v`, `--x=v`, and positionals). */
export function pathValues(argv: ReadonlyArray<string>, index: FinIndex): string[] {
  const cmd = index.command(argv);
  if (cmd === undefined || cmd.paths.length === 0) return [];
  const rest = argv.slice(cmd.words);
  const out: string[] = [];
  const positionals: string[] = [];
  for (let i = 0; i < rest.length; i++) {
    const a = rest[i]!;
    const p = cmd.paths.find((x) => x.flag !== undefined && (a === x.flag || a.startsWith(`${x.flag}=`)));
    if (p) {
      if (a.includes("=")) out.push(a.slice(a.indexOf("=") + 1));
      else if (i + 1 < rest.length) out.push(rest[++i]!);
    } else if (a.startsWith("--")) {
      // Another flag; its value (if any) is not a positional. Unknown here, so skip nothing.
    } else {
      positionals.push(a);
    }
  }
  for (const p of cmd.paths) if (p.flag === undefined && typeof p.position === "number" && positionals[p.position] !== undefined) out.push(positionals[p.position]!);
  return out;
}

async function realOf(p: string): Promise<string> {
  const rest: string[] = [];
  let cur = p;
  for (;;) {
    try {
      return join(await realpath(cur), ...rest.reverse());
    } catch {
      const parent = dirname(cur);
      if (parent === cur) return p;
      rest.push(cur.slice(parent.length).replace(/^[\\/]+/, ""));
      cur = parent;
    }
  }
}

/** `PATH_NOT_ALLOWED` for a path value outside every root (resolved against `cwd`). */
export async function checkPaths(values: ReadonlyArray<string>, cwd: string, roots: ReadonlyArray<string>, fold: boolean): Promise<void> {
  const f = (s: string) => (fold ? s.toLowerCase() : s);
  const real = await Promise.all(roots.map((r) => realOf(resolve(r))));
  for (const v of values) {
    // `fin` expands a leading `~` to the home folder: so does the check.
    const home = v === "~" || v.startsWith("~/") || v.startsWith("~\\") ? join(homedir(), v.slice(2)) : undefined;
    const abs = home ?? (isAbsolute(v) ? v : resolve(cwd, v));
    const p = f(await realOf(abs));
    if (!real.some((r) => p === f(r) || p.startsWith(f(r.endsWith(sep) ? r : r + sep)))) {
      throw finError("PATH_NOT_ALLOWED", `${v}: an agent session reads files in its workspace or the project folder only`, { path: v, reason: "outside" }, "write the file into the workspace first, then name it by a path inside it");
    }
  }
}

/** Ends `child` and every process it started (its process group; `taskkill /T` on Windows). */
export function killTree(child: ChildProcess): void {
  if (child.pid === undefined || child.exitCode !== null) return;
  if (process.platform === "win32") {
    spawn("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true }).on("error", () => child.kill("SIGKILL"));
    return;
  }
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch {
    child.kill("SIGKILL");
  }
}

function parseOut(text: string): unknown {
  const t = text.trim();
  if (t === "") return null;
  try {
    return JSON.parse(t) as unknown;
  } catch {
    return t;
  }
}

/** Runs one `fin` call (module docs). Usage errors of the call itself are thrown as `FinError`s. */
export async function runFinTool(call: { callId: string; argv: string[]; stdin?: string }, env: SessionEnv): Promise<ToolOutcome> {
  const { argv } = call;
  if (argv.length > 0 && !COMMAND_WORD.test(argv[0]!)) {
    throw finError("USAGE", "argv[0] must be a fin command word (like `view` or `item`), never a path or an option", { argv0: argv[0] }, "run `fin` with no arguments for the command map");
  }
  if (env.roots !== undefined && env.index !== undefined) {
    await checkPaths(pathValues(argv, env.index), env.cwd, env.roots, env.fold ?? (process.platform === "win32" || process.platform === "darwin"));
  }
  const timeoutMs = env.timeoutMs ?? TOOL_TIMEOUT_MS;
  return new Promise<ToolOutcome>((done, fail) => {
    const child = spawn(env.finPath, argv, {
      cwd: env.cwd,
      env: toolEnv(env.env, call.callId),
      shell: false,
      windowsHide: true,
      // Its own process group, so the timeout ends whatever it started too.
      detached: process.platform !== "win32",
      stdio: ["pipe", "pipe", "pipe"],
    });
    const out: Buffer[] = [];
    let outBytes = 0;
    let cut = false;
    let err = Buffer.alloc(0);
    let timedOut = false;
    let aborted = false;
    child.stdout!.on("data", (b: Buffer) => {
      if (outBytes + b.length > STDOUT_MAX) {
        cut = true;
        b = b.subarray(0, Math.max(0, STDOUT_MAX - outBytes));
      }
      outBytes += b.length;
      if (b.length > 0) out.push(b);
    });
    child.stderr!.on("data", (b: Buffer) => {
      err = Buffer.concat([err, b]);
      if (err.length > 4 * STDERR_TAIL) err = err.subarray(err.length - STDERR_TAIL);
    });
    child.stdin!.on("error", () => {});
    child.stdin!.end(call.stdin ?? "");
    const timer = setTimeout(() => {
      timedOut = true;
      killTree(child);
    }, timeoutMs);
    const onAbort = () => {
      aborted = true;
      killTree(child);
    };
    env.signal?.addEventListener("abort", onAbort, { once: true });
    child.on("error", (e) => {
      clearTimeout(timer);
      env.signal?.removeEventListener("abort", onAbort);
      fail(finError("PROVIDER_UNAVAILABLE", `cannot run fin: ${e.message}`, { finPath: env.finPath }, "check agents.finPath"));
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      env.signal?.removeEventListener("abort", onAbort);
      const stderr = err.subarray(Math.max(0, err.length - STDERR_TAIL)).toString("utf8");
      const tail = stderr === "" ? {} : { stderrTail: stderr };
      if (timedOut || aborted) {
        const e = timedOut
          ? finError("TOOL_TIMEOUT", `fin ran longer than ${Math.round(timeoutMs / 1000)} s and was stopped`, { timeoutMs }, "split the work into smaller commands")
          : finError("SESSION_INTERRUPTED", "the fin call was stopped", {});
        return done({ ok: false, output: { exit: code, signal, error: { code: e.code, message: e.message, details: e.details, ...(e.fix ? { fix: e.fix } : {}) }, ...tail } });
      }
      const stdout = parseOut(Buffer.concat(out).toString("utf8"));
      done({ ok: code === 0, output: { exit: code, ...(signal === null ? {} : { signal }), stdout, ...(cut ? { stdoutTruncated: true } : {}), ...tail } });
    });
  });
}
