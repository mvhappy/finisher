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
// - **Paths:** argv is read by the `fin schema commands` index (command words with global flags
//   between them, each flag's value count, `--`), and the arguments it marks as paths, and any
//   `--project`, are checked before `fin` starts (the same confinement `fin` applies itself under
//   an agent credential, §5.6). What it cannot read is refused, and without the index nothing
//   runs: the tool fails closed in front of `fin`'s own check.
import { type ChildProcess, execFile, spawn } from "node:child_process";
import { realpath, stat } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { NEVER_PASSED } from "../../env.ts";
import { finError } from "../../errors.ts";
import type { ToolOutcome } from "../runtime-deps.ts";

export const TOOL_TIMEOUT_MS = 120_000;
const STDERR_TAIL = 1024;
/** The most stdout kept (a larger answer is cut and marked): what the loop can use and store. */
export const STDOUT_MAX = 1024 * 1024;

/** Variables the `fin` tool never passes on (spec §8.5; Decision 14b), whatever its caller gave. */
export const REMOVED_VARS = [...NEVER_PASSED, "FIN_OFFLINE_SNAPSHOT", "FIN_CALL_IDEM"] as const;

/** The runtime's word for the session's project folder: the only one `fin` trusts under an agent credential. */
export const PROJECT_DIR_ENV = "FIN_PROJECT_DIR";

export interface SessionEnv {
  readonly finPath: string;
  /** `workspace/<sessionId>/`. */
  readonly cwd: string;
  /** The session environment (`OpenEnvironment.env` with `FIN_CREDENTIAL`). */
  readonly env: Readonly<Record<string, string>>;
  readonly timeoutMs?: number;
  readonly signal?: AbortSignal;
  /** Where a path argument may point: the workspace and the project folder (the second, when given, is the only `--project` allowed). */
  readonly roots?: readonly string[];
  /** Required with `roots`: a call is never run unchecked. */
  readonly index?: FinIndex;
  /** Compare paths case-insensitively (default: Windows and macOS). */
  readonly fold?: boolean;
}

/** One flag of a command: its spellings and how many values it takes (`null`: an open count). */
export interface FinFlag {
  readonly long?: string;
  readonly short?: string;
  readonly values: number | null;
}

/** One command of `fin schema commands`. */
export interface FinCommand {
  readonly name: string;
  readonly write: boolean;
  readonly paths: ReadonlyArray<{ readonly arg: string; readonly flag?: string; readonly position?: number | null }>;
  /** Its flags, the global ones included (an older `fin` gives none: a call with path arguments is then refused). */
  readonly flags?: ReadonlyArray<FinFlag>;
}

/** What the tool reads of an argv (`FinIndex.parse`). */
export interface ParsedArgv {
  /** The command, when argv names a known one. */
  readonly command?: FinCommand;
  /** The values of its path arguments (flags and positionals). */
  readonly paths: string[];
  /** Every `--project` value. */
  readonly project: string[];
  /** `--dry-run` as a flag (never a value, never after `--`). */
  readonly dryRun: boolean;
}

const cannotParse = (why: string, argv: ReadonlyArray<string>) =>
  finError("USAGE", `the fin tool cannot read this argv: ${why}`, { argv }, "name the command words first (e.g. [\"caption\", \"import\", ...]), then its flags and arguments");

const findFlag = (flags: ReadonlyArray<FinFlag>, word: string): { flag: FinFlag; inline?: string } | undefined => {
  if (word.startsWith("--")) {
    const eq = word.indexOf("=");
    const name = eq < 0 ? word : word.slice(0, eq);
    const flag = flags.find((f) => f.long === name);
    return flag && (eq < 0 ? { flag } : { flag, inline: word.slice(eq + 1) });
  }
  const flag = flags.find((f) => f.short === word);
  return flag && { flag };
};

/** What the tool knows of `fin`'s commands: which write, their flags, which take paths (argKinds). */
export class FinIndex {
  readonly #byName: ReadonlyMap<string, FinCommand>;
  readonly #prefixes: ReadonlySet<string>;
  readonly #globals: ReadonlyArray<FinFlag>;

  constructor(commands: ReadonlyArray<FinCommand>, globals: ReadonlyArray<FinFlag> = []) {
    this.#byName = new Map(commands.map((c) => [c.name, c]));
    const prefixes = new Set<string>();
    for (const c of commands) {
      const words = c.name.split(".");
      for (let n = 1; n < words.length; n++) prefixes.add(words.slice(0, n).join("."));
    }
    this.#prefixes = prefixes;
    this.#globals = globals;
  }

  /**
   * Reads `argv` as `fin` would: the command words (global flags with their values may come
   * before or between them), then the command's flags (by their value counts), `--` and the
   * positionals. A flag before the command words are complete that is no global one, or a flag
   * this index does not know on a command with path arguments, is `USAGE`: the tool fails
   * closed on what it cannot follow (`fin`'s own check, under the agent credential, is the
   * authority).
   */
  parse(argv: ReadonlyArray<string>): ParsedArgv {
    const project: string[] = [];
    let dryRun = false;
    const take = (i: number, hit: { flag: FinFlag; inline?: string }, strict: boolean): number => {
      const { flag, inline } = hit;
      let values: string[];
      let next = i + 1;
      if (inline !== undefined) {
        if (flag.values === 0) throw cannotParse(`${argv[i]} takes no value`, argv);
        values = [inline];
      } else if (flag.values === null) {
        values = [];
        while (next < argv.length && !argv[next]!.startsWith("-")) values.push(argv[next++]!);
      } else {
        if (next + flag.values > argv.length && strict) throw cannotParse(`${argv[i]} needs ${flag.values} value(s)`, argv);
        values = argv.slice(next, next + flag.values);
        next += flag.values;
      }
      if (flag.long === "--project") project.push(...values);
      if (flag.long === "--dry-run") dryRun = true;
      return next;
    };
    // The command words, with the global flags skipped.
    const words: string[] = [];
    let command: FinCommand | undefined;
    let i = 0;
    /** Where the command's own arguments start: after the word that completed it. */
    let end = 0;
    while (i < argv.length) {
      const a = argv[i]!;
      if (a.startsWith("-") && a !== "-") {
        const g = a === "--" ? undefined : findFlag(this.#globals, a);
        if (g !== undefined) {
          i = take(i, g, true);
          continue;
        }
        if (command === undefined) throw cannotParse(`${a} comes before the command words are complete`, argv);
        break;
      }
      const name = [...words, a].join(".");
      const known = this.#byName.get(name);
      if (known === undefined && !this.#prefixes.has(name)) break;
      words.push(a);
      i++;
      if (known !== undefined) {
        command = known;
        end = i;
      }
      if (!this.#prefixes.has(name)) break;
    }
    if (command === undefined) return { paths: [], project, dryRun };
    // Words read past the command that completed no longer one are its positionals (`try b1`).
    i = end;
    const strict = command.paths.length > 0;
    if (strict && command.flags === undefined) throw cannotParse(`this fin gives no flags of ${command.name}`, argv);
    const flags = command.flags ?? this.#globals;
    // The rest: flags by their value counts, `--`, positionals.
    const paths: string[] = [];
    const positionals: string[] = [];
    for (; i < argv.length; ) {
      const a = argv[i]!;
      if (a === "--") {
        positionals.push(...argv.slice(i + 1));
        break;
      }
      if (a.startsWith("-") && a !== "-") {
        const hit = findFlag(flags, a);
        if (hit === undefined) {
          if (strict) throw cannotParse(`${command.name} has no flag ${a}`, argv);
          i++;
          continue;
        }
        const next = take(i, hit, strict);
        const p = command.paths.find((x) => x.flag !== undefined && x.flag === hit.flag.long);
        if (p) paths.push(...(hit.inline !== undefined ? [hit.inline] : argv.slice(i + 1, next)));
        i = next;
        continue;
      }
      positionals.push(a);
      i++;
    }
    for (const p of command.paths) if (p.flag === undefined && typeof p.position === "number" && positionals[p.position] !== undefined) paths.push(positionals[p.position]!);
    return { command, paths, project, dryRun };
  }
}

const indexes = new Map<string, Promise<FinIndex>>();

/** `fin schema commands`, once per `fin` binary (its path and modification time); a failure is retried by the next call. */
export async function finIndex(finPath: string): Promise<FinIndex> {
  const st = await stat(finPath);
  const key = `${finPath}\0${st.mtimeMs}\0${st.size}`;
  let p = indexes.get(key);
  if (p === undefined) {
    p = new Promise<FinIndex>((ok, fail) => {
      execFile(finPath, ["schema", "commands"], { env: { PATH: process.env.PATH ?? "" }, timeout: 30_000, maxBuffer: 4 * 1024 * 1024 }, (err, stdout) => {
        if (err) return fail(err);
        try {
          const doc = JSON.parse(stdout) as { commands: FinCommand[]; globals?: FinFlag[] };
          if (!Array.isArray(doc.commands)) throw new TypeError("no commands");
          ok(new FinIndex(doc.commands, doc.globals ?? []));
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

/** The path values of `argv` the index recognises (`FinIndex.parse`; `USAGE` where it cannot read argv). */
export function pathValues(argv: ReadonlyArray<string>, index: FinIndex): string[] {
  return index.parse(argv).paths;
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

/** `PROJECT_NOT_FOUND` for a `--project` that is not the session's project folder (as `fin` refuses it). */
async function checkProject(values: ReadonlyArray<string>, cwd: string, project: string | undefined, fold: boolean): Promise<void> {
  if (values.length === 0) return;
  const f = (s: string) => (fold ? s.toLowerCase() : s);
  const own = project === undefined ? undefined : f(await realOf(resolve(project)));
  for (const v of values) {
    const abs = v === "~" || v.startsWith("~/") || v.startsWith("~\\") ? join(homedir(), v.slice(2)) : resolve(cwd, v);
    if (own === undefined || f(await realOf(abs)) !== own) {
      throw finError("PROJECT_NOT_FOUND", "this agent session works on its own project only", { dir: v }, "run fin from the session's workspace, without --project");
    }
  }
}

/**
 * Ends `child` and every process it started (its process group; `taskkill /T` on Windows). On
 * POSIX the group is killed even when `fin` itself has exited: a process it left behind holding
 * the output pipe would otherwise keep the call open. (Windows cannot reach the tree of an
 * exited parent by its PID; plan 7A Task 15 covers the Windows case.)
 */
export function killTree(child: ChildProcess): void {
  if (child.pid === undefined) return;
  if (process.platform === "win32") {
    if (child.exitCode !== null || child.signalCode !== null) return;
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
  if (env.roots !== undefined) {
    // Never run a call unchecked: without the index there is nothing to check it with.
    if (env.index === undefined) throw finError("PROVIDER_UNAVAILABLE", "the fin tool has no command index (`fin schema commands` failed)", { finPath: env.finPath }, "check agents.finPath; the next call tries again");
    const fold = env.fold ?? (process.platform === "win32" || process.platform === "darwin");
    const parsed = env.index.parse(argv);
    await checkProject(parsed.project, env.cwd, env.roots[1], fold);
    await checkPaths(parsed.paths, env.cwd, env.roots, fold);
  }
  // Checked right before the spawn: an interrupt during the checks above starts nothing.
  if (env.signal?.aborted) throw finError("SESSION_INTERRUPTED", "the fin call was stopped before it started", {});
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
      // `close` waits for the pipes: whatever still holds them is not waited for.
      child.stdout!.destroy();
      child.stderr!.destroy();
    }, timeoutMs);
    const onAbort = () => {
      aborted = true;
      killTree(child);
      child.stdout!.destroy();
      child.stderr!.destroy();
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
