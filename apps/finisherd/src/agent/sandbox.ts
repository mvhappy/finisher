// The session sandbox (spec §8; plan 7C Task 11): the only way the agent runtime starts a
// provider CLI or anything else for a session (a gate, `scripts/check-agent-spawn.mjs`, keeps
// `child_process` out of `agent/` but here and our own `fin`). Opening it makes the session's
// directories (`session-dirs.ts`), links the sign-in store in place, builds the session
// environment of §8.5 (`sandbox-env.ts`) and decides how processes start, from the one status
// source (`sandbox/status.ts`, Decision 23):
// - **launcher**, where the `agent` profile is enforced, or Landlock is disabled (F2: the
//   launcher then runs the program unconfined but still strips the socket variables): each
//   process starts as `fin-sandbox-exec --policy-fd 3 -- <argv>`, its typed `AgentInput` on
//   descriptor 3 (never argv or the environment), and `fin` inside reaches the server over the
//   agent listener on Linux (`tcp:127.0.0.1:<port>`, Decision 13), `fin.sock` on macOS, the
//   pipe on Windows;
// - **direct**, where the profile is not ready on this OS (Decision 5): the process starts
//   with the session environment and no sandbox, and the persistent warning says so;
// - **refused** where the mechanism is unavailable: `SANDBOX_UNAVAILABLE`, nothing starts.
// Every process starts in a process group of its own and the whole group is killed when its
// leader exits or the scope closes (session end, or the instance replaced). What the launcher observes and
// reports, a failure to apply (exit 78) or a seccomp kill (`SIGSYS`), goes to `onViolation`
// (`sandbox.violation`, spec §8.8).
import { type ChildProcess, spawn as spawnChild } from "node:child_process";
import { mkdir, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import type { Readable, Writable } from "node:stream";
import type { FinError, Harness, Session } from "@finisher/contracts";
import type { AgentInput, FinEndpoint } from "@finisher/core-types";
import { sandboxAgentCheck, sandboxToolchainBase } from "@finisher/fin-node";
import { Effect, type Scope } from "effect";
import { finError, toFinError } from "../errors.ts";
import { resolveSandboxExecPath } from "../sandbox/exec.ts";
import { currentSandboxStatus, type SandboxMechanism, type SandboxStatus } from "../sandbox/status.ts";
import { sessionEnv } from "./sandbox-env.ts";
import { ensureSessionDirs, linkSignInStore } from "./session-dirs.ts";

export { assertSessionEnv, sessionEnv } from "./sandbox-env.ts";

/** The launcher's own exit code for `SANDBOX_UNAVAILABLE` (`launch.rs` `EXIT_UNAVAILABLE`). */
export const LAUNCHER_UNAVAILABLE = 78;

export interface ChildExit {
  readonly code: number | null;
  readonly signal: NodeJS.Signals | null;
}

export interface SandboxedChild {
  readonly pid: number;
  readonly stdin: Writable;
  readonly stdout: Readable;
  readonly stderr: Readable;
  /** Settles when the process has exited. */
  readonly exited: Promise<ChildExit>;
  /** Signals the child's whole process group. */
  kill(signal?: NodeJS.Signals): void;
}

export type SandboxMode = "launcher" | "direct";

export interface SpawnOptions {
  readonly cwd: string;
  readonly stdio: "pipe";
  /** Read roots this process needs beyond the session's (a harness's own files). */
  readonly extraRead?: readonly string[];
}

export interface SessionSandbox {
  /** The session environment (spec §8.5), `FIN_PROJECT_DIR` included. */
  readonly env: Readonly<Record<string, string>>;
  /** The `agent` profile's status when the sandbox opened. */
  readonly status: SandboxMechanism;
  readonly mode: SandboxMode;
  /** `workspace/<sessionId>/`, the session's working directory. */
  readonly workspace: string;
  spawn(argv: readonly string[], opts: SpawnOptions): Effect.Effect<SandboxedChild, FinError>;
}

/** What the launcher observed (spec §8.8): no path, only the root kind. */
export interface SandboxViolation {
  readonly sessionId: string;
  readonly op: "apply" | "seccompKill";
  readonly profile: "agent";
  readonly rootKind: "none";
}

export interface SessionSandboxDeps {
  /** Server data, links resolved. */
  readonly serverData: string;
  readonly projectDirOf: (projectId: string) => string | undefined;
  /** Our own `fin`, readable inside the sandbox. */
  readonly finPath: string;
  /** The local socket (or the pipe on Windows): `FIN_SERVER` where no listener is used. */
  readonly localEndpoint: string;
  /** The agent listener's endpoint, started on first need (Linux). */
  readonly agentListener?: () => Promise<string>;
  /** The roots of the libraries the project references. */
  readonly libraryRoots?: (projectId: string) => Promise<readonly string[]>;
  readonly status?: () => SandboxStatus;
  readonly launcher?: () => string;
  readonly onViolation?: (v: SandboxViolation) => void;
  readonly platform?: NodeJS.Platform;
  readonly parent?: NodeJS.ProcessEnv;
  /** How long a killed process group may take to go at scope close (default 5 s). */
  readonly killGraceMs?: number;
}

/** How the `agent` profile's status starts processes, or the refusal. */
export function sandboxMode(m: SandboxMechanism, os: string): SandboxMode | FinError {
  switch (m.kind) {
    case "enforced":
      return "launcher";
    case "disabled":
      // F2: the launcher runs the program unconfined where Landlock is off; any other reason
      // (a profile not ready on this OS) is refused by the launcher, so it is not used.
      return m.reason === "landlockDisabled" ? "launcher" : "direct";
    case "unavailable":
      return finError(
        "SANDBOX_UNAVAILABLE",
        "the agent sandbox cannot be applied on this host, so agent processes do not start",
        { profile: "agent", os, need: m.need },
        `This host needs ${m.need}.`,
      );
  }
}

const canonical = (p: string) => realpath(p).catch(() => p);

/**
 * Opens `s`'s sandbox in the current scope (spec §8.5, §8.6): its directories, the sign-in
 * store link, the environment and the profile input, checked by `fin-sandbox` now so a bad
 * input fails here rather than at each process. Closing the scope kills every process it
 * started (their whole groups).
 */
export function openSessionSandbox(s: Session, harness: Harness, deps: SessionSandboxDeps): Effect.Effect<SessionSandbox, FinError, Scope.Scope> {
  const platform = deps.platform ?? process.platform;
  const children = new Set<SandboxedChild>();
  const open = Effect.tryPromise({
    try: async (): Promise<SessionSandbox> => {
      const status = (deps.status ?? currentSandboxStatus)();
      const mode = sandboxMode(status.agent, status.os);
      if (typeof mode !== "string") throw mode;
      const dir = deps.projectDirOf(s.projectId);
      if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "the session's project is not open on this server", { projectId: s.projectId });
      const projectDir = await canonical(dir);
      const workspace = join(projectDir, "workspace", s.id);
      const shared = join(projectDir, "workspace", "shared");
      await mkdir(workspace, { recursive: true, mode: 0o700 });
      await mkdir(shared, { recursive: true, mode: 0o700 });
      const dirs = await ensureSessionDirs(join(deps.serverData, "agents"), s.id, { platform });
      const store = await linkSignInStore(harness, deps.serverData, dirs);
      const server = mode === "launcher" ? await launcherServer(deps, platform) : { env: deps.localEndpoint, fin: undefined };
      const env = sessionEnv({ session: { id: s.id, harness }, server: server.env, projectDir, dirs, store, platform, ...(deps.parent === undefined ? {} : { parent: deps.parent }) });
      const libraries = await Promise.all((await (deps.libraryRoots?.(s.projectId) ?? Promise.resolve([]))).map(canonical));
      const base: AgentInput = {
        sessionId: s.id,
        mode: s.mode,
        harness,
        projectDir,
        workspaceDir: workspace,
        sharedDir: shared,
        libraryRoots: libraries,
        sessionDir: dirs.dir,
        ...(store === undefined ? {} : { signInStore: store.grant }),
        toolchains: [...sandboxToolchainBase(), await canonical(deps.finPath)],
        serverData: deps.serverData,
        home: await canonical(homedir()),
        ...(server.fin === undefined ? {} : { fin: server.fin }),
      };
      if (mode === "launcher") check(base, status.os);
      const launcher = mode === "launcher" ? (deps.launcher ?? resolveSandboxExecPath)() : "";
      return {
        env,
        status: status.agent,
        mode,
        workspace,
        spawn: (argv, opts) =>
          Effect.tryPromise({
            try: async () => {
              if (argv.length === 0) throw finError("USAGE", "nothing to start", { sessionId: s.id });
              let input = base;
              if (opts.extraRead !== undefined && opts.extraRead.length > 0) {
                input = { ...base, toolchains: [...base.toolchains!, ...(await Promise.all(opts.extraRead.map(canonical)))] };
                if (mode === "launcher") check(input, status.os);
              }
              const child = await start(mode === "launcher" ? { launcher, request: { profile: "agent", input } } : undefined, argv, opts.cwd, env, platform, (exit) => {
                if (mode !== "launcher") return;
                const op = exit.signal === "SIGSYS" ? "seccompKill" : exit.code === LAUNCHER_UNAVAILABLE ? "apply" : undefined;
                if (op !== undefined) deps.onViolation?.({ sessionId: s.id, op, profile: "agent", rootKind: "none" });
              });
              children.add(child);
              void child.exited.then(() => children.delete(child));
              return child;
            },
            catch: toFinError,
          }),
      };
    },
    catch: toFinError,
  });
  return Effect.acquireRelease(open, () => Effect.promise(() => killAll(children, deps.killGraceMs ?? 5_000)));
}

/** `FIN_SERVER` and the policy's `fin` endpoint under the launcher, per OS. */
async function launcherServer(deps: SessionSandboxDeps, platform: NodeJS.Platform): Promise<{ env: string; fin: FinEndpoint | undefined }> {
  if (platform === "win32") return { env: deps.localEndpoint, fin: { kind: "pipe" } };
  if (platform === "darwin") return { env: deps.localEndpoint, fin: { kind: "unixSocket", path: deps.localEndpoint } };
  if (deps.agentListener === undefined) throw finError("SANDBOX_UNAVAILABLE", "the agent listener is not available", { profile: "agent", os: "linux", need: "the agent listener" });
  const endpoint = await deps.agentListener();
  const port = Number(/^tcp:127\.0\.0\.1:(\d+)$/.exec(endpoint)?.[1]);
  if (!Number.isInteger(port) || port <= 0) throw finError("INTERNAL", "the agent listener's endpoint is not loopback TCP", { endpoint });
  return { env: endpoint, fin: { kind: "tcp", port } };
}

function check(input: AgentInput, os: string): void {
  const reason = sandboxAgentCheck(input);
  if (reason !== null) throw finError("SANDBOX_UNAVAILABLE", `the agent sandbox cannot be built: ${reason}`, { profile: "agent", os, need: reason });
}

/** Starts one process (through the launcher when `via` is given) in a process group of its own. */
function start(
  via: { launcher: string; request: unknown } | undefined,
  argv: readonly string[],
  cwd: string,
  env: Readonly<Record<string, string>>,
  platform: NodeJS.Platform,
  onExit: (e: ChildExit) => void,
): Promise<SandboxedChild> {
  const [program, args] = via === undefined ? [argv[0]!, argv.slice(1)] : [via.launcher, ["--policy-fd", "3", "--", ...argv]];
  const child: ChildProcess = spawnChild(program, args, {
    cwd,
    env,
    stdio: via === undefined ? ["pipe", "pipe", "pipe"] : ["pipe", "pipe", "pipe", "pipe"],
    detached: platform !== "win32",
    windowsHide: true,
  });
  const exited = new Promise<ChildExit>((resolve) => {
    child.once("exit", (code, signal) => {
      const e = { code, signal };
      // What the leader left running in its group goes with it, while the group's id cannot
      // yet name another process's (a process that left the group is a residual: no seccomp).
      if (platform !== "win32" && child.pid !== undefined) {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {
          // The group is gone.
        }
      }
      onExit(e);
      resolve(e);
    });
  });
  return new Promise((resolve, reject) => {
    child.once("error", (e) =>
      reject(finError("SANDBOX_UNAVAILABLE", `cannot start ${via === undefined ? program : "the sandbox launcher"}: ${e.message}`, { profile: "agent", program: via === undefined ? program : "fin-sandbox-exec" })),
    );
    child.once("spawn", () => {
      if (via !== undefined) {
        // The launcher reads descriptor 3 to its end: our end is closed once written.
        const req = child.stdio[3] as Writable;
        req.on("error", () => {});
        req.end(JSON.stringify(via.request));
      }
      const pid = child.pid!;
      resolve({
        pid,
        stdin: child.stdin!,
        stdout: child.stdout!,
        stderr: child.stderr!,
        exited,
        kill: (signal = "SIGTERM") => {
          try {
            if (platform === "win32") child.kill(signal);
            else process.kill(-pid, signal);
          } catch {
            // Already gone.
          }
        },
      });
    });
  });
}

async function killAll(children: Set<SandboxedChild>, graceMs: number): Promise<void> {
  const all = [...children];
  for (const c of all) c.kill("SIGKILL");
  await Promise.race([Promise.all(all.map((c) => c.exited)), new Promise((r) => setTimeout(r, graceMs).unref())]);
}

/** The runtime's view of session sandboxes (one per server; `wiring.ts` builds it). */
export interface SessionSandboxes {
  /** Refuses a CLI harness's start where the `agent` profile is unavailable. */
  check(harness: Harness): void;
  /** The sandbox to open for `s`, or `undefined` for the built-in agent (spec §8.9). */
  open(s: Session, onViolation: (v: SandboxViolation) => void): Effect.Effect<SessionSandbox, FinError, Scope.Scope> | undefined;
}

/** Session sandboxes over `deps` (each violation is also given to `deps.onViolation`: the audit). */
export function sessionSandboxes(deps: SessionSandboxDeps): SessionSandboxes {
  return {
    check(harness) {
      if (harness === "builtin") return;
      const status = (deps.status ?? currentSandboxStatus)();
      const mode = sandboxMode(status.agent, status.os);
      if (typeof mode !== "string") throw mode;
    },
    open(s, onViolation) {
      if (s.harness === "builtin") return undefined;
      return openSessionSandbox(s, s.harness, {
        ...deps,
        onViolation: (v) => {
          deps.onViolation?.(v);
          onViolation(v);
        },
      });
    },
  };
}
