// One command hook's process (7F plan Task 3; Decisions 2, 6, 9, 10, 12; spec §8.10, §9.2).
//
// - **Mode** from the `hook` profile's status (`sandboxMode`, as the session sandbox does for
//   `agent`): `launcher` where it is enforced (or Landlock is disabled, F2), `direct` where it is
//   not ready on this OS (unconfined, with the persistent `SANDBOX_DISABLED` warning handed to
//   `onStart`), and where it is unavailable the hook **fails** (`unavailable`), nothing starts.
//   Under the launcher the process gets `LaunchRequest {profile: "hook", input: HookInput}`
//   (`sandbox/launch.ts`, the one place `hooks/` reaches `child_process`): the project
//   read-only, its own file, the toolchain base plus its program (and `node`'s install
//   directory), its private temp the only write root, no network, and `timeoutMs` as
//   `Limits.wall_ms`. On Windows the launch's table (`sandboxHookGrants`) is granted through
//   the ACL broker under owner `call:<ulid>` before the start and revoked at the exit and on a
//   failed start.
// - **`fin: true`** (Task 5, Decision 11): after the re-check the hook's credential file
//   (`credential`, `hooks/credential.ts`: minted on first use) and `FIN_SERVER` (`finServer`:
//   under the launcher the agent listener on Linux, `fin.sock` on macOS, the pipe on Windows, as
//   `launcherServer` answers them; else the local endpoint) are added: `HookInput.fin
//   {credential, endpoint}` (the listener's port is then its only network), `FIN_CREDENTIAL`,
//   `FIN_SERVER`, and `FIN_PROJECT_DIR` (the event's project: under an agent credential `fin`
//   trusts no other folder); `fin` itself (`finPath`) is readable through `toolchains` and its
//   folder ends `PATH`. On Windows under the launcher the project's container is allowed on
//   the pipe (`allowPipe`). A hook without `fin` gets none of these.
// - **The process:** argv `[program, ...run.slice(1)]` without a shell (boundaries kept); the
//   environment cleared to `PATH` (the toolchain base's folders), `HOME` and
//   `TMPDIR`/`TEMP`/`TMP` (the private temp), `LANG=C.UTF-8`, `FIN_HOOK_EVENT` (Windows adds
//   `SystemRoot`, which Node needs to start); cwd the private temp
//   `<server-data>/hooks/run/<ulid>/` (0700, removed after the call); stdin one `HookRequest`
//   line (a payload over 1 MiB as `{payloadOmitted: true, bytes}`), then EOF.
// - **The deadline** (Decision 10) is finisherd's: the hook's `timeoutS` from the call, slot
//   wait included (at most `HOOK_LIMITS.processes` at once, `HookSlots`); at it the process is
//   killed with its tree (`SIGKILL` of its group, or the launcher hung up and then its group).
//   The server's `signal` (it stops) does the same at once, and nothing starts after it.
// - **The reply** (Decision 2): exit 0 with empty stdout is `allow`; else exactly one JSON line
//   that `decodeHookReply` accepts, `reason` ≤ 1 KiB and `text` ≤ 16 KiB after control
//   characters (but `\n`, `\t`) are stripped. Anything else is a `HookFailure`: `timeout`,
//   `exit`, `signal`, `invalid`, `tooLarge` (stdout over 64 KiB: killed at once; a reason or
//   text too long) or `unavailable` (the profile, the program, the input or the start).
// - **The file that runs** (review of Task 3): an argument in `run[1..]` exactly equal to the
//   spec's `file` (as written) is replaced by the path of the file that runs, so the plan's
//   `run: ["node", "fake-hook.mjs"], file: "fake-hook.mjs"` works from the private temp.
// - **Nothing unverified runs from the project:** after `recheck` (the engine's:
//   `HookConfig.applies`, every layer re-read), a project hook's bound files (`bound`: its
//   program in the project, its `file`) are read once, hashed, compared with the hashes its
//   `hooksHash` binds, and the bytes read are written to `<temp>/.hook/<n>/<name>` (0700); the
//   copy runs (it is `run[0]` and the `file` argument) and `HookInput.hookFile` names it (a
//   read root, so Linux executes it although the temp is not an execute root). A failed
//   re-check or a hash that differs is `HookNotRun {notRun: changed}`: the hook does not
//   apply, which is not a failure. The race left is the other layers' (server data, written
//   by a person): from `recheck` to the program's own reads.
import { accessSync, constants, statSync } from "node:fs";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { basename, dirname, join, posix, win32 } from "node:path";
import { decodeHookReply, HOOK_LIMITS, type HookReply } from "@finisher/contracts";
import type { AclGrant, FinEndpoint, HookInput, HookRequest } from "@finisher/core-types";
import { sandboxContainerSid, sandboxHookCheck, sandboxHookGrants, sandboxProjectContainer, sandboxToolchainBase } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { canonical, type SandboxMode, sandboxMode, within } from "../agent/sandbox-input.ts";
import { hashOf } from "../agent/file-cache.ts";
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
  /** Its process had started (`HookStats.runs` counts it). */
  readonly started?: true;
}

/** A hook that did not run because it no longer applies (its configuration or a bound file changed). */
export interface HookNotRun {
  readonly notRun: "changed";
  readonly detail: string;
}

/** What one run of a command hook answers. */
export type HookRun = HookReply | HookFailure | HookNotRun;

export const isNotRun = (r: HookRun): r is HookNotRun => "notRun" in r;

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
  /** Whether the hook still applies, right before its process starts (false: `HookNotRun`). */
  readonly recheck?: () => boolean;
  /** Tests: called after the re-check (`checked`) and after the verified copies (`copied`). */
  readonly onStage?: (stage: "checked" | "copied") => void | Promise<void>;
  /** Told how the process starts, with the profile's warning where it runs unconfined. */
  readonly onStart?: (s: { readonly mode: SandboxMode; readonly warning?: SandboxWarning }) => void;
  /** Overrides the hook's `timeoutS` (tests). */
  readonly timeoutMs?: number;
  /** The server stopping: nothing starts after it, a running process is killed (`unavailable`). */
  readonly signal?: AbortSignal;
  /** A `fin` hook's credential file (the engine's, per hook and project; minted on first use). */
  readonly credential?: () => string;
  /** `FIN_SERVER` and, under the launcher, the policy's endpoint. */
  readonly finServer?: (launcher: boolean) => Promise<{ readonly env: string; readonly fin?: FinEndpoint | undefined }>;
  /** The `fin` a `fin` hook runs. */
  readonly finPath?: string;
  /** Windows: admits a container SID on the server's pipe (`PipeServer.allow`). */
  readonly allowPipe?: (sid: string) => void;
}

/** What a `fin` hook's run adds (Decision 11). */
interface FinGrant {
  readonly credential: string;
  readonly server: { readonly env: string; readonly fin?: FinEndpoint | undefined };
  readonly finPath: string;
}

/** At most `n` hook processes at once (Decision 10): FIFO, a waiter gives up at its deadline. */
export class HookSlots {
  #free: number;
  readonly #waiting: Array<(release: () => void) => void> = [];

  constructor(n: number = HOOK_LIMITS.processes) {
    this.#free = n;
  }

  /** A slot's release, or `undefined` when none was free by `deadline` (ms since the epoch). */
  acquire(deadline: number, signal?: AbortSignal): Promise<(() => void) | undefined> {
    if (signal?.aborted === true) return Promise.resolve(undefined);
    if (this.#free > 0) {
      this.#free -= 1;
      return Promise.resolve(this.#release());
    }
    return new Promise((resolve) => {
      const take = (release: () => void) => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", give);
        resolve(release);
      };
      const give = () => {
        clearTimeout(timer);
        const i = this.#waiting.indexOf(take);
        if (i >= 0) this.#waiting.splice(i, 1);
        resolve(undefined);
      };
      const timer = setTimeout(give, Math.max(0, deadline - Date.now()));
      signal?.addEventListener("abort", give, { once: true });
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
export function runCommandHook(h: ResolvedHook, req: HookRequest, deps: HookProcessDeps): Effect.Effect<HookRun> {
  return Effect.promise(() => runOnce(h, req, deps).catch((e: unknown) => fail("unavailable", message(e))));
}

async function runOnce(h: ResolvedHook, req: HookRequest, deps: HookProcessDeps): Promise<HookRun> {
  const platform = deps.platform ?? process.platform;
  const timeoutMs = deps.timeoutMs ?? (h.spec?.timeoutS ?? HOOK_LIMITS.timeoutS.default) * 1000;
  const deadline = Date.now() + timeoutMs;
  const status = (deps.status ?? currentSandboxStatus)();
  const mode = sandboxMode(status.hook, status.os, "hook");
  if (typeof mode !== "string") return fail("unavailable", mode.message);
  const release = await deps.slots.acquire(deadline, deps.signal);
  if (deps.signal?.aborted === true) {
    release?.();
    return fail("unavailable", STOPPING);
  }
  if (release === undefined) return fail("timeout", `no process slot within ${timeoutMs} ms`);
  try {
    const base = (deps.toolchainBase ?? sandboxToolchainBase)();
    const found = findProgram(h.program, base, platform);
    if (typeof found !== "string") return found;
    if (deps.recheck !== undefined && !deps.recheck()) return changed("the hook's configuration changed before it started");
    await deps.onStage?.("checked");
    return await inTemp(deps.serverData, async (tmp, id) => {
      const copies = await verifiedCopies(h, tmp);
      if (!(copies instanceof Map)) return copies;
      await deps.onStage?.("copied");
      const granted = h.spec?.fin === true ? await finGrant(deps, mode === "launcher") : undefined;
      if (granted !== undefined && "kind" in granted) return granted;
      const program = copies.get(found) ?? found;
      const file = h.file === undefined ? undefined : (copies.get(h.file) ?? h.file);
      const named = h.spec?.file;
      const argv = [program, ...h.run.slice(1).map((a) => (named !== undefined && file !== undefined && a === named ? file : a))];
      const env = hookEnv(tmp, req.event, base, platform);
      if (granted !== undefined) {
        env.PATH = [env.PATH, dirname(granted.finPath)].filter((p) => p !== "").join(platform === "win32" ? ";" : ":");
        Object.assign(env, { FIN_CREDENTIAL: granted.credential, FIN_SERVER: granted.server.env, FIN_PROJECT_DIR: req.project.dir });
      }
      let via: LaunchVia | undefined;
      let owner: string | undefined;
      const acl = platform === "win32" && deps.acl?.active === true ? deps.acl : undefined;
      try {
        if (mode === "launcher") {
          const input = await hookInput(h, program, file ?? program, req, tmp, deps.serverData, base, Math.max(1, deadline - Date.now()), granted);
          const reason = sandboxHookCheck(input);
          if (reason !== null) return fail("unavailable", `the hook sandbox cannot be built: ${reason}`);
          if (acl !== undefined && granted !== undefined) {
            // The project's container reaches `fin` over the pipe (spec §8.11): its SID is allowed there.
            const sid = (deps.containerSid ?? sandboxContainerSid)(sandboxProjectContainer(input.projectDir));
            if (sid === null || deps.allowPipe === undefined) return fail("unavailable", "the project's container cannot be admitted on the server's pipe");
            deps.allowPipe(sid);
          }
          if (acl !== undefined) {
            owner = `call:${id}`;
            await grantTable(acl, (deps.hookGrants ?? sandboxHookGrants)(input), { owner, ...(deps.containerSid === undefined ? {} : { containerSid: deps.containerSid }) });
          }
          via = { launcher: (deps.launcher ?? resolveSandboxExecPath)(), request: { profile: "hook", input } };
        }
        const warning = profileWarning("hook", status.os, status.hook);
        deps.onStart?.({ mode, ...(warning === undefined ? {} : { warning }) });
        if (deps.signal?.aborted === true) return fail("unavailable", STOPPING);
        let launcherFailed = false;
        let child: SandboxedChild;
        try {
          child = await startProcess(via, argv, tmp, env, platform, (_e, failed) => (launcherFailed = failed), "hook");
        } catch (e) {
          return fail("unavailable", message(e));
        }
        const r = await collect(child, req, deadline, timeoutMs, () => launcherFailed, deps.signal);
        return "kind" in r ? { ...r, started: true } : r;
      } finally {
        // A revoke that fails stays in the broker's log; the next start's sweep completes it.
        if (owner !== undefined) await acl?.revoke(owner).catch(() => 0);
      }
    });
  } finally {
    release();
  }
}

const changed = (detail: string): HookNotRun => ({ notRun: "changed", detail });

/** A `fin` hook's credential and endpoint, or why it gets none (`unavailable`). */
async function finGrant(deps: HookProcessDeps, launcher: boolean): Promise<FinGrant | HookFailure> {
  if (deps.credential === undefined || deps.finServer === undefined || deps.finPath === undefined) return fail("unavailable", "fin is not available to hooks on this server");
  const credential = deps.credential();
  return { credential, server: await deps.finServer(launcher), finPath: deps.finPath };
}
/** A run refused or ended because the server stops. */
const STOPPING = "the server is stopping";

/**
 * A project hook's bound files, each read once, checked against its bound hash and written to
 * `<tmp>/.hook/<n>/<name>` (0700): resolved path → the copy. A file missing or changed is
 * `HookNotRun`.
 */
async function verifiedCopies(h: ResolvedHook, tmp: string): Promise<Map<string, string> | HookNotRun> {
  const copies = new Map<string, string>();
  let n = 0;
  for (const [path, hash] of Object.entries(h.bound ?? {})) {
    let bytes: Buffer;
    try {
      bytes = await readFile(path);
    } catch {
      return changed(`${basename(path)} cannot be read since it was trusted`);
    }
    if (hashOf(bytes) !== hash) return changed(`${basename(path)} changed since it was trusted`);
    const dir = join(tmp, ".hook", String(n++));
    await mkdir(dir, { recursive: true, mode: 0o700 });
    const copy = join(dir, basename(path));
    await writeFile(copy, bytes, { mode: 0o700, flag: "wx" });
    copies.set(path, copy);
  }
  return copies;
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
async function hookInput(h: ResolvedHook, program: string, file: string, req: HookRequest, tmp: string, serverData: string, base: readonly string[], timeoutMs: number, granted?: FinGrant): Promise<HookInput> {
  const projectDir = await canonical(req.project.dir);
  const prog = await canonical(program);
  const hookFile = await canonical(file);
  // The program is read through `toolchains` unless it is the hook's own file or project content.
  const own = prog === hookFile || within(prog, projectDir) ? [] : [prog];
  const node = "node" in h.program ? [dirname(prog)] : [];
  const fin = granted === undefined ? [] : [await canonical(granted.finPath)];
  const endpoint = granted?.server.fin;
  return {
    projectDir,
    hookFile,
    toolchains: [...new Set([...base, ...own, ...node, ...fin])],
    tmp,
    serverData: await canonical(serverData),
    home: await canonical(homedir()),
    timeoutMs,
    ...(granted === undefined || endpoint === undefined ? {} : { fin: { credential: await canonical(granted.credential), endpoint } }),
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
async function collect(child: SandboxedChild, req: HookRequest, deadline: number, timeoutMs: number, launcherFailed: () => boolean, signal?: AbortSignal): Promise<HookReply | HookFailure> {
  child.stdin.on("error", () => {});
  child.stdin.end(requestLine(req));
  const out: Buffer[] = [];
  let size = 0;
  let over = false;
  let timedOut = false;
  const err: Buffer[] = [];
  let errBytes = 0;
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
    if (errBytes >= STDERR_BYTES) return;
    const part = c.subarray(0, STDERR_BYTES - errBytes);
    err.push(part);
    errBytes += part.length;
  });
  const closed = new Promise<void>((resolve) => {
    child.stdout.once("close", () => resolve());
    child.stdout.once("error", () => resolve());
  });
  const timer = setTimeout(() => {
    timedOut = true;
    child.kill("SIGKILL");
  }, Math.max(0, deadline - Date.now()));
  let stopped = false;
  const stop = () => {
    stopped = true;
    child.kill("SIGKILL");
  };
  if (signal?.aborted === true) stop();
  else signal?.addEventListener("abort", stop, { once: true });
  let exit: ChildExit;
  try {
    exit = await child.exited;
  } finally {
    // An exit in time is never a timeout, however long its stdout takes to close.
    clearTimeout(timer);
    signal?.removeEventListener("abort", stop);
  }
  // What it wrote before it exited (a descendant still holding stdout is given a second).
  await Promise.race([closed, new Promise((r) => setTimeout(r, 1_000).unref())]);
  child.stdout.destroy();
  // At most `STDERR_BYTES`, a character cut at the end dropped.
  const stderr = Buffer.concat(err).toString("utf8").replace(/\uFFFD$/, "");
  if (stopped) return fail("unavailable", STOPPING);
  if (timedOut) return fail("timeout", `no reply within ${timeoutMs} ms`);
  if (over) return fail("tooLarge", `more than ${HOOK_LIMITS.stdoutBytes} bytes on stdout`);
  if (exit.code === LAUNCHER_UNAVAILABLE && launcherFailed()) return fail("unavailable", "the hook sandbox could not be applied");
  if (exit.signal !== null) return fail("signal", `killed by ${exit.signal}`, stderr);
  if (exit.code !== 0) return fail("exit", `exit ${exit.code}`, stderr);
  return parseReply(Buffer.concat(out));
}

/** Bytes of stderr kept for a failure's `stderr`. */
const STDERR_BYTES = 512;
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
  if (text === "") return { reply: "allow" };
  // Exactly one line: at most one trailing `\n` or `\r\n`, nothing blank.
  const line = text.endsWith("\r\n") ? text.slice(0, -2) : text.endsWith("\n") ? text.slice(0, -1) : text;
  if (line.trim() === "") return fail("invalid", "a blank line on stdout");
  if (/[\r\n]/.test(line)) return fail("invalid", "more than one line on stdout");
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
