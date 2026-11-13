// One command hook's process (7F plan Task 3; Decisions 2, 6, 9, 10, 12; spec §8.10, §9.2).
//
// - **Mode** from the `hook` profile's status (`sandboxMode`, as the session sandbox does for
//   `agent`): `launcher` where it is enforced (or Landlock is disabled, F2), `direct` where it is
//   not ready on this OS (unconfined, with the persistent `SANDBOX_DISABLED` warning handed to
//   `onStart`), and where it is unavailable the hook **fails** (`unavailable`), nothing starts.
//   Under the launcher the process gets `LaunchRequest {profile: "hook", input: HookInput}`
//   (`sandbox/launch.ts`, the one place `hooks/` reaches `child_process`): the project
//   read-only, its own file, the toolchain base plus its program (and `node`'s install
//   directory), its private temp the only write root, no network (`fin`: Task 5), and
//   `timeoutMs` as `Limits.wall_ms`. On Windows the launch's table (`sandboxHookGrants`) is
//   granted through the ACL broker under owner `call:<ulid>` before the start and revoked at the
//   exit and on a failed start.
// - **The process:** argv `[program, ...run.slice(1)]` without a shell (boundaries kept); the
//   environment cleared to `PATH` (the toolchain base's folders), `HOME` and
//   `TMPDIR`/`TEMP`/`TMP` (the private temp), `LANG=C.UTF-8`, `FIN_HOOK_EVENT` (Windows adds
//   `SystemRoot`, which Node needs to start); cwd the private temp
//   `<server-data>/hooks/run/<ulid>/` (0700, removed after the call); stdin one `HookRequest`
//   line (a payload over 1 MiB as `{payloadOmitted: true, bytes}`), then EOF.
// - **The deadline** (Decision 10) is finisherd's: the hook's `timeoutS` from the call, slot
//   wait included (at most `HOOK_LIMITS.processes` at once, `HookSlots`); at it the process is
//   killed with its tree (`SIGKILL` of its group, or the launcher hung up and then its group).
// - **The reply** (Decision 2): exit 0 with empty stdout is `allow`; else exactly one JSON line
//   that `decodeHookReply` accepts, `reason` ≤ 1 KiB and `text` ≤ 16 KiB after control
//   characters (but `\n`, `\t`) are stripped. Anything else is a `HookFailure`: `timeout`,
//   `exit`, `signal`, `invalid`, `tooLarge` (stdout over 64 KiB: killed at once; a reason or
//   text too long) or `unavailable` (the profile, the program, the input or the start).
// - **The check before the start:** `recheck` (the engine's: `HookConfig.applies`) runs after
//   the slot is taken, right before the process starts, so a hook whose file (or, for a project
//   hook, a script its `hooksHash` binds) changed while it waited does not run. What remains is
//   the time from that check to the program's own open of its files (the launcher's start, the
//   interpreter's read): a write in it by someone who may write the project is not seen.
import { accessSync, constants, statSync } from "node:fs";
import { mkdir, rm } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join, posix, win32 } from "node:path";
import { decodeHookReply, HOOK_LIMITS, type HookReply } from "@finisher/contracts";
import type { AclGrant, HookInput, HookRequest } from "@finisher/core-types";
import { sandboxHookCheck, sandboxHookGrants, sandboxToolchainBase } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { canonical, type SandboxMode, sandboxMode, within } from "../agent/sandbox-input.ts";
import { ulid } from "../control/ulid.ts";
import type { AclBroker } from "../sandbox/broker.ts";
import { grantTable } from "../sandbox/broker.ts";
import { resolveSandboxExecPath } from "../sandbox/exec.ts";
import { type ChildExit, LAUNCHER_UNAVAILABLE, type LaunchVia, type SandboxedChild, startProcess } from "../sandbox/launch.ts";
import { currentSandboxStatus, profileWarning, type SandboxStatus, type SandboxWarning } from "../sandbox/status.ts";
import type { ConfiguredHook, HookProgram } from "./config.ts";

/** A command hook as `hooks(projectId)` answers it: its `run` and resolved program. */
export type ResolvedHook = ConfiguredHook & { readonly run: readonly string[]; readonly program: HookProgram };

/** A command hook (not a plugin's or a synthetic one). */
export const isCommandHook = (h: ConfiguredHook): h is ResolvedHook => h.run !== undefined && h.program !== undefined;

export type HookFailureKind = "timeout" | "exit" | "signal" | "invalid" | "tooLarge" | "unavailable";

/** Why a hook gave no reply (Decision 3 decides what it answers). */
export interface HookFailure {
  readonly kind: HookFailureKind;
  readonly detail: string;
  /** The start of what it wrote on stderr (at most 512 bytes), for an `exit` or `signal`. */
  readonly stderr?: string;
}

export interface HookProcessDeps {
  /** Server data (its `hooks/run/` holds the private temps). */
  readonly serverData: string;
  /** The server's process slots (one `HookSlots` per server). */
  readonly slots: HookSlots;
  readonly status?: () => SandboxStatus;
  readonly launcher?: () => string;
  readonly platform?: NodeJS.Platform;
  /** The toolchain base (default: fin-node's `sandboxToolchainBase`). */
  readonly toolchainBase?: () => readonly string[];
  /** Windows: the server's ACL broker. */
  readonly acl?: AclBroker;
  /** The ACEs a launch needs (default: fin-node's `sandboxHookGrants`). */
  readonly hookGrants?: (input: HookInput) => AclGrant[];
  /** A container's SID by name (default: fin-node's; tests). */
  readonly containerSid?: (name: string) => string | null;
  /** Whether the hook still applies, right before its process starts (false: `unavailable`). */
  readonly recheck?: () => boolean;
  /** Told how the process starts, with the profile's warning where it runs unconfined. */
  readonly onStart?: (s: { readonly mode: SandboxMode; readonly warning?: SandboxWarning }) => void;
  /** Overrides the hook's `timeoutS` (tests). */
  readonly timeoutMs?: number;
}

/** At most `n` hook processes at once (Decision 10): FIFO, a waiter gives up at its deadline. */
export class HookSlots {
  #free: number;
  readonly #waiting: Array<(release: () => void) => void> = [];

  constructor(n: number = HOOK_LIMITS.processes) {
    this.#free = n;
  }

  /** A slot's release, or `undefined` when none was free by `deadline` (ms since the epoch). */
  acquire(deadline: number): Promise<(() => void) | undefined> {
    if (this.#free > 0) {
      this.#free -= 1;
      return Promise.resolve(this.#release());
    }
    return new Promise((resolve) => {
      const take = (release: () => void) => {
        clearTimeout(timer);
        resolve(release);
      };
      const timer = setTimeout(() => {
        const i = this.#waiting.indexOf(take);
        if (i >= 0) this.#waiting.splice(i, 1);
        resolve(undefined);
      }, Math.max(0, deadline - Date.now()));
      this.#waiting.push(take);
    });
  }

  #release(): () => void {
    let done = false;
    return () => {
      if (done) return;
      done = true;
      const next = this.#waiting.shift();
      if (next === undefined) this.#free += 1;
      else next(this.#release());
    };
  }
}

const fail = (kind: HookFailureKind, detail: string, stderr?: string): HookFailure => ({ kind, detail, ...(stderr === undefined || stderr === "" ? {} : { stderr }) });
const message = (e: unknown) => (e instanceof Error ? e.message : String(e)).split("\n")[0]!;

/**
 * Runs `h` once for `req` (see the module docs): its reply, or why there is none. Never fails:
 * every error is a `HookFailure`.
 */
export function runCommandHook(h: ResolvedHook, req: HookRequest, deps: HookProcessDeps): Effect.Effect<HookReply | HookFailure> {
  return Effect.promise(() => runOnce(h, req, deps).catch((e: unknown) => fail("unavailable", message(e))));
}

async function runOnce(h: ResolvedHook, req: HookRequest, deps: HookProcessDeps): Promise<HookReply | HookFailure> {
  const platform = deps.platform ?? process.platform;
  const timeoutMs = deps.timeoutMs ?? (h.spec?.timeoutS ?? HOOK_LIMITS.timeoutS.default) * 1000;
  const deadline = Date.now() + timeoutMs;
  const status = (deps.status ?? currentSandboxStatus)();
  const mode = sandboxMode(status.hook, status.os, "hook");
  if (typeof mode !== "string") return fail("unavailable", mode.message);
  const release = await deps.slots.acquire(deadline);
  if (release === undefined) return fail("timeout", `no process slot within ${timeoutMs} ms`);
  try {
    const base = (deps.toolchainBase ?? sandboxToolchainBase)();
    const program = findProgram(h.program, base, platform);
    if (typeof program !== "string") return program;
    return await inTemp(deps.serverData, async (tmp, id) => {
      const env = hookEnv(tmp, req.event, base, platform);
      const argv = [program, ...h.run.slice(1)];
      let via: LaunchVia | undefined;
      let owner: string | undefined;
      const acl = platform === "win32" && deps.acl?.active === true ? deps.acl : undefined;
      try {
        if (mode === "launcher") {
          const input = await hookInput(h, program, req, tmp, deps.serverData, base, timeoutMs);
          const reason = sandboxHookCheck(input);
          if (reason !== null) return fail("unavailable", `the hook sandbox cannot be built: ${reason}`);
          if (acl !== undefined) {
            owner = `call:${id}`;
            await grantTable(acl, (deps.hookGrants ?? sandboxHookGrants)(input), { owner, ...(deps.containerSid === undefined ? {} : { containerSid: deps.containerSid }) });
          }
          via = { launcher: (deps.launcher ?? resolveSandboxExecPath)(), request: { profile: "hook", input } };
        }
        if (deps.recheck !== undefined && !deps.recheck()) return fail("unavailable", "the hook's configuration changed before it started");
        const warning = profileWarning("hook", status.os, status.hook);
        deps.onStart?.({ mode, ...(warning === undefined ? {} : { warning }) });
        let launcherFailed = false;
        let child: SandboxedChild;
        try {
          child = await startProcess(via, argv, tmp, env, platform, (_e, failed) => (launcherFailed = failed), "hook");
        } catch (e) {
          return fail("unavailable", message(e));
        }
        return await collect(child, req, deadline, timeoutMs, () => launcherFailed);
      } finally {
        // A revoke that fails stays in the broker's log; the next start's sweep completes it.
        if (owner !== undefined) await acl?.revoke(owner).catch(() => 0);
      }
    });
  } finally {
    release();
  }
}

/** Runs `f` in a new private temp `<server-data>/hooks/run/<ulid>/` (0700), removed after. */
async function inTemp<A>(serverData: string, f: (tmp: string, id: string) => Promise<A>): Promise<A> {
  const runs = join(serverData, "hooks", "run");
  await mkdir(runs, { recursive: true, mode: 0o700 });
  const id = ulid();
  const tmp = join(await canonical(runs), id);
  await mkdir(tmp, { mode: 0o700 });
  try {
    return await f(tmp, id);
  } finally {
    await rm(tmp, { recursive: true, force: true, maxRetries: 3 }).catch(() => 0);
  }
}

/** The folders of the toolchain base, in its order (its files are not `PATH` entries). */
function baseDirs(base: readonly string[]): string[] {
  return base.filter((p) => {
    try {
      return statSync(p).isDirectory();
    } catch {
      return false;
    }
  });
}

/** The program's path: as resolved at load, or a bare name found in the toolchain base's folders. */
function findProgram(p: HookProgram, base: readonly string[], platform: NodeJS.Platform): string | HookFailure {
  if ("path" in p) return p.path;
  for (const dir of baseDirs(base)) {
    const candidate = (platform === "win32" ? win32 : posix).join(dir, p.bare);
    try {
      if (!statSync(candidate).isFile()) continue;
      if (platform !== "win32") accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      // Not here.
    }
  }
  return fail("unavailable", `${p.bare} is not in the toolchain base (a hook's bare program name is looked up only there)`);
}

/** Decision 9's environment, and nothing else. */
function hookEnv(tmp: string, event: string, base: readonly string[], platform: NodeJS.Platform): Record<string, string> {
  const env: Record<string, string> = {
    PATH: baseDirs(base).join(platform === "win32" ? ";" : ":"),
    HOME: tmp,
    TMPDIR: tmp,
    TEMP: tmp,
    TMP: tmp,
    LANG: "C.UTF-8",
    FIN_HOOK_EVENT: event,
  };
  if (platform === "win32") env.SystemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT ?? "C:\\Windows";
  return env;
}

/** The `hook` profile's input: canonical paths, the program (and `node`'s folder) readable. */
async function hookInput(h: ResolvedHook, program: string, req: HookRequest, tmp: string, serverData: string, base: readonly string[], timeoutMs: number): Promise<HookInput> {
  const projectDir = await canonical(req.project.dir);
  const prog = await canonical(program);
  const hookFile = h.file === undefined ? prog : await canonical(h.file);
  // The program is read through `toolchains` unless it is the hook's own file or project content.
  const own = prog === hookFile || within(prog, projectDir) ? [] : [prog];
  const node = "node" in h.program ? [dirname(prog)] : [];
  return {
    projectDir,
    hookFile,
    toolchains: [...new Set([...base, ...own, ...node])],
    tmp,
    serverData: await canonical(serverData),
    home: await canonical(homedir()),
    timeoutMs,
  };
}

/** The request line: the payload omitted when it is over the cap. */
function requestLine(req: HookRequest): string {
  const payload = JSON.stringify(req.payload ?? null);
  const bytes = Buffer.byteLength(payload);
  const sent = bytes > HOOK_LIMITS.payloadBytes ? { ...req, payload: { payloadOmitted: true, bytes } } : req;
  return `${JSON.stringify(sent)}\n`;
}

/** Feeds the request, reads stdout under its cap and stderr's start, and enforces the deadline. */
async function collect(child: SandboxedChild, req: HookRequest, deadline: number, timeoutMs: number, launcherFailed: () => boolean): Promise<HookReply | HookFailure> {
  child.stdin.on("error", () => {});
  child.stdin.end(requestLine(req));
  const out: Buffer[] = [];
  let size = 0;
  let over = false;
  let timedOut = false;
  let err = "";
  child.stdout.on("data", (c: Buffer) => {
    if (over) return;
    size += c.length;
    if (size > HOOK_LIMITS.stdoutBytes) {
      over = true;
      child.kill("SIGKILL");
      return;
    }
    out.push(c);
  });
  child.stderr.on("data", (c: Buffer) => {
    if (err.length < 512) err += c.toString("utf8").slice(0, 512 - err.length);
  });
  const closed = new Promise<void>((resolve) => {
    child.stdout.once("close", () => resolve());
    child.stdout.once("error", () => resolve());
  });
  const timer = setTimeout(() => {
    timedOut = true;
    child.kill("SIGKILL");
  }, Math.max(0, deadline - Date.now()));
  let exit: ChildExit;
  try {
    exit = await child.exited;
    // What it wrote before it exited (a descendant still holding stdout is given a second).
    await Promise.race([closed, new Promise((r) => setTimeout(r, 1_000).unref())]);
  } finally {
    clearTimeout(timer);
  }
  child.stdout.destroy();
  if (timedOut) return fail("timeout", `no reply within ${timeoutMs} ms`);
  if (over) return fail("tooLarge", `more than ${HOOK_LIMITS.stdoutBytes} bytes on stdout`);
  if (exit.code === LAUNCHER_UNAVAILABLE && launcherFailed()) return fail("unavailable", "the hook sandbox could not be applied");
  if (exit.signal !== null) return fail("signal", `killed by ${exit.signal}`, err);
  if (exit.code !== 0) return fail("exit", `exit ${exit.code}`, err);
  return parseReply(Buffer.concat(out));
}

const utf8 = new TextDecoder("utf-8", { fatal: true });
/** Control characters but `\n` and `\t`. */
const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;

/** One reply line (Decision 2), or nothing for `allow`. */
export function parseReply(bytes: Uint8Array): HookReply | HookFailure {
  let text: string;
  try {
    text = utf8.decode(bytes);
  } catch {
    return fail("invalid", "stdout is not UTF-8");
  }
  const line = text.trim();
  if (line === "") return { reply: "allow" };
  if (line.includes("\n")) return fail("invalid", "more than one line on stdout");
  let json: unknown;
  try {
    json = JSON.parse(line);
  } catch {
    return fail("invalid", "stdout is not one JSON value");
  }
  const decoded = decodeHookReply(json);
  if (Exit.isFailure(decoded)) return fail("invalid", "not a reply: allow, block {reason} or context {text}");
  const r = decoded.value;
  if (r.reply === "block") {
    const reason = r.reason.replace(CONTROL, "");
    return Buffer.byteLength(reason) > HOOK_LIMITS.reasonBytes ? fail("tooLarge", `a reason over ${HOOK_LIMITS.reasonBytes} bytes`) : { reply: "block", reason };
  }
  if (r.reply === "context") {
    const t = r.text.replace(CONTROL, "");
    return Buffer.byteLength(t) > HOOK_LIMITS.textBytes ? fail("tooLarge", `a context text over ${HOOK_LIMITS.textBytes} bytes`) : { reply: "context", text: t };
  }
  return { reply: "allow" };
}
