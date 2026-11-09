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
// leader exits or the scope closes (session end, or the instance replaced). On Linux and
// macOS the launcher stays the program's parent (plan 7C Decision 3 as amended): it forwards
// signals, passes the program's exit through and, when the program exits, is hung up or loses
// its parent, kills every descendant, one that left the group (`setsid`, `setpgid`) included:
// on Linux as a subreaper (and with the session's cgroup where one is delegated), on macOS by
// the session's Seatbelt mark, which every descendant inherits. So a kill with `SIGKILL` of a
// process under either launcher hangs it up (`SIGHUP`, which it never forwards) and falls back
// to `SIGKILL` on its group after `REAP_GRACE_MS`; a `SIGKILL` would end the launcher before it
// reaps. **Residual:** without the launcher (direct mode, where the profile is not ready on
// this OS) a process that leaves its group outlives both, unconfined anyway. Windows closes it
// with the launcher's job object (Task 9).
// What the launcher itself reports, a failure to apply the policy (its line written back on
// descriptor 3 before exit 78; a program's own exit 78 writes none), goes to `onViolation`
// (`sandbox.violation`, spec §8.8). A seccomp kill (`SIGSYS`) is not attributed: without the
// filter (plan Task 4) every `SIGSYS` is one a process sent, and so could be forged.
// On Windows the launcher's AppContainer reaches only what the ACL broker granted (plan 7C
// Task 10): the agent table (`sandboxAgentGrants`: the masks follow the policy) is granted
// before the first process starts (re-granting an entry the broker already holds is a no-op),
// a process's `extraRead` granted to the session's capability under the session's owner, the
// session's own entries revoked when its last open scope closes (a replaced instance's scope
// may close after its successor opened: `SessionGrants` counts them, so the old one never
// revokes the new one's grants), and the project's container SID is allowed on the pipe
// (`allowPipe`).
import { type ChildProcess, spawn as spawnChild } from "node:child_process";
import { mkdir, realpath } from "node:fs/promises";
import { homedir } from "node:os";
import { isAbsolute, join, relative, sep } from "node:path";
import type { Readable, Writable } from "node:stream";
import type { FinError, Harness, Session } from "@finisher/contracts";
import type { AgentInput, FinEndpoint } from "@finisher/core-types";
import { sandboxAgentCheck, sandboxAgentGrants, sandboxContainerSid, sandboxProjectContainer, sandboxToolchainBase } from "@finisher/fin-node";
import { Effect, type Scope } from "effect";
import { canonicalEndpoint } from "../auth/crypto.ts";
import { finError, toFinError } from "../errors.ts";
import { type AclBroker, grantTable, sessionOwner } from "../sandbox/broker.ts";
import { resolveSandboxExecPath } from "../sandbox/exec.ts";
import { currentSandboxStatus, type SandboxMechanism, type SandboxStatus } from "../sandbox/status.ts";
import { sessionEnv } from "./sandbox-env.ts";
import { ensureSessionDirs, type LinkOptions, linkSignInStore } from "./session-dirs.ts";

export { assertSessionEnv, sessionEnv } from "./sandbox-env.ts";

/** How long a hung-up Linux launcher may take to reap its session before its group is killed. */
export const REAP_GRACE_MS = 2_000;

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
  /**
   * Inside the session's workspace or the shared workspace, both granted (spec §8.5): on
   * Windows a container's process needs its working directory granted, and Node reads the
   * nearest `package.json` above it (S9 point 8), so it never starts elsewhere.
   */
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
  readonly op: "apply";
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
  /** Windows: the server's ACL broker (plan 7C Task 10). */
  readonly acl?: AclBroker;
  /** Windows: admits a container SID on the server's pipe (`PipeServer.allow`). */
  readonly allowPipe?: (sid: string) => void;
  /** The ACEs a launch needs (default: fin-node's `sandboxAgentGrants`). */
  readonly agentGrants?: typeof sandboxAgentGrants;
  /** A container's SID by name (default: fin-node's `sandboxContainerSid`; tests). */
  readonly containerSid?: (name: string) => string | null;
  /** The sign-in link call (tests: a Windows host without the symlink privilege). */
  readonly symlink?: LinkOptions["symlink"];
  /** The open scopes per session owner (default: one per broker). */
  readonly sessionGrants?: SessionGrants;
}

/**
 * The session sandboxes open per session (Windows, Task 10 review 2): a session's grants are
 * revoked when its last open scope closes, and a scope that opens while that revoke runs waits
 * for it before granting (a grant the broker still held would otherwise be skipped as covered,
 * then removed by the revoke).
 */
export class SessionGrants {
  readonly #open = new Map<string, number>();
  readonly #revoking = new Map<string, Promise<unknown>>();

  /** A scope of `owner` opens: waits for a revoke of `owner` still running. */
  async enter(owner: string): Promise<void> {
    this.#open.set(owner, (this.#open.get(owner) ?? 0) + 1);
    await this.#revoking.get(owner);
  }

  /** A scope of `owner` closes: `revoke` runs when it was the last. */
  async leave(owner: string, revoke: () => Promise<unknown>): Promise<void> {
    const n = (this.#open.get(owner) ?? 1) - 1;
    if (n > 0) {
      this.#open.set(owner, n);
      return;
    }
    this.#open.delete(owner);
    const p = (this.#revoking.get(owner) ?? Promise.resolve()).then(revoke).catch(() => 0);
    this.#revoking.set(owner, p);
    await p;
    if (this.#revoking.get(owner) === p) this.#revoking.delete(owner);
  }
}

const GRANTS = new WeakMap<AclBroker, SessionGrants>();

function grantsOf(acl: AclBroker): SessionGrants {
  let g = GRANTS.get(acl);
  if (g === undefined) {
    g = new SessionGrants();
    GRANTS.set(acl, g);
  }
  return g;
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

/** `p` is `dir` or inside it (both canonical). */
function within(p: string, dir: string): boolean {
  const r = relative(dir, p);
  return r === "" || (r !== ".." && !r.startsWith(`..${sep}`) && !isAbsolute(r));
}

/**
 * Opens `s`'s sandbox in the current scope (spec §8.5, §8.6): its directories, the sign-in
 * store link, the environment and the profile input, checked by `fin-sandbox` now so a bad
 * input fails here rather than at each process. Closing the scope kills every process it
 * started (their whole groups).
 */
export function openSessionSandbox(s: Session, harness: Harness, deps: SessionSandboxDeps): Effect.Effect<SessionSandbox, FinError, Scope.Scope> {
  const platform = deps.platform ?? process.platform;
  const children = new Set<SandboxedChild>();
  // Set once the scope closes: a spawn still in flight is killed as it arrives.
  const closed = { v: false };
  // Windows: the broker's grants are made under this owner (revoked at scope close).
  const acl = platform === "win32" && deps.acl?.active === true ? deps.acl : undefined;
  const owner = sessionOwner(s.id);
  const scopes = acl === undefined ? undefined : (deps.sessionGrants ?? grantsOf(acl));
  // Set once this scope counts in `scopes` (it then leaves at close, or when the open fails).
  let entered = false;
  const leave = async () => {
    if (!entered || acl === undefined || scopes === undefined) return;
    entered = false;
    // A revoke that fails stays in the broker's log; the next start's sweep completes it.
    await scopes.leave(owner, () => acl.revoke(owner));
  };
  const grant = async (input: AgentInput, extraRead?: readonly string[]) => {
    if (acl === undefined) return;
    const table = (deps.agentGrants ?? sandboxAgentGrants)(input, extraRead);
    await grantTable(acl, table, { owner, ...(deps.containerSid === undefined ? {} : { containerSid: deps.containerSid }) });
  };
  const open = Effect.tryPromise({
    try: async (): Promise<SessionSandbox> => {
      const status = (deps.status ?? currentSandboxStatus)();
      const mode = sandboxMode(status.agent, status.os);
      if (typeof mode !== "string") throw mode;
      const dir = deps.projectDirOf(s.projectId);
      if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "the session's project is not open on this server", { projectId: s.projectId });
      const projectDir = await canonical(dir);
      await mkdir(join(projectDir, "workspace", s.id), { recursive: true, mode: 0o700 });
      await mkdir(join(projectDir, "workspace", "shared"), { recursive: true, mode: 0o700 });
      // Links resolved (a `workspace/` the user put on another disk), as `spawn` resolves a cwd.
      const workspace = await canonical(join(projectDir, "workspace", s.id));
      const shared = await canonical(join(projectDir, "workspace", "shared"));
      const dirs = await ensureSessionDirs(join(deps.serverData, "agents"), s.id, { platform });
      const store = await linkSignInStore(harness, deps.serverData, dirs, { platform, ...(deps.symlink === undefined ? {} : { symlink: deps.symlink }) });
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
      if (mode === "launcher") {
        check(base, status.os);
        // The project's container reaches `fin` over the pipe (spec §8.11): its SID is allowed
        // there, whatever the grant table names.
        if (acl !== undefined && deps.allowPipe !== undefined) {
          const sid = (deps.containerSid ?? sandboxContainerSid)(sandboxProjectContainer(projectDir));
          if (sid === null) throw finError("SANDBOX_UNAVAILABLE", "no SID for the project's container", { profile: "agent", os: status.os, need: "the project's container SID" });
          deps.allowPipe(sid);
        }
        // Before any process: what the container may reach (a failure, here or later in the
        // open, revokes what was granted unless another open scope of the session uses it:
        // `leave` below).
        if (scopes !== undefined) {
          entered = true;
          await scopes.enter(owner);
        }
        await grant(base);
      }
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
              const cwd = await canonical(opts.cwd);
              if (!within(cwd, workspace) && !within(cwd, shared)) {
                throw finError("USAGE", "a session's process starts in its workspace", { sessionId: s.id, reason: "cwdOutsideWorkspace" });
              }
              let input = base;
              if (opts.extraRead !== undefined && opts.extraRead.length > 0) {
                const extra = await Promise.all(opts.extraRead.map(canonical));
                input = { ...base, toolchains: [...base.toolchains!, ...extra] };
                if (mode === "launcher") {
                  check(input, status.os);
                  // The session's own (revoked with it), never static toolchain grants.
                  await grant(input, extra);
                }
              }
              if (closed.v) throw finError("SESSION_INTERRUPTED", "the session's sandbox is closed", { sessionId: s.id });
              const child = await start(mode === "launcher" ? { launcher, request: { profile: "agent", input } } : undefined, argv, cwd, env, platform, (exit, launcherFailed) => {
                if (launcherFailed && exit.code === LAUNCHER_UNAVAILABLE) deps.onViolation?.({ sessionId: s.id, op: "apply", profile: "agent", rootKind: "none" });
              });
              children.add(child);
              void child.exited.then(() => children.delete(child));
              if (closed.v) {
                child.kill("SIGKILL");
                throw finError("SESSION_INTERRUPTED", "the session's sandbox closed while the process started", { sessionId: s.id });
              }
              return child;
            },
            catch: toFinError,
          }),
      };
    },
    catch: toFinError,
  });
  const opened = open.pipe(Effect.tapError(() => Effect.promise(leave)));
  return Effect.acquireRelease(opened, () =>
    Effect.promise(async () => {
      closed.v = true;
      await killAll(children, deps.killGraceMs ?? 5_000);
      await leave();
    }),
  );
}

/** `FIN_SERVER` and the policy's `fin` endpoint under the launcher, per OS. */
async function launcherServer(deps: SessionSandboxDeps, platform: NodeJS.Platform): Promise<{ env: string; fin: FinEndpoint | undefined }> {
  if (platform === "win32") return { env: deps.localEndpoint, fin: { kind: "pipe" } };
  if (platform === "darwin") {
    // Resolved, as the handshake binds it: a sandboxed `fin` may not resolve a link on the way
    // (`/tmp`, `/var`), and the endpoint it compares would then be the link's (`endpointMismatch`).
    let socket = deps.localEndpoint;
    try {
      socket = canonicalEndpoint(socket, platform);
    } catch {
      // Not there (yet): `fin` meets the same unresolved path.
    }
    return { env: socket, fin: { kind: "unixSocket", path: socket } };
  }
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
  onExit: (e: ChildExit, launcherFailed: boolean) => void,
): Promise<SandboxedChild> {
  // The Linux and macOS launchers stay the parent and reap the session (see the header).
  const reaps = via !== undefined && (platform === "linux" || platform === "darwin");
  const [program, args] = via === undefined ? [argv[0]!, argv.slice(1)] : [via.launcher, ["--policy-fd", "3", "--", ...argv]];
  const child: ChildProcess = spawnChild(program, args, {
    cwd,
    env,
    stdio: via === undefined ? ["pipe", "pipe", "pipe"] : ["pipe", "pipe", "pipe", "pipe"],
    detached: platform !== "win32",
    windowsHide: true,
  });
  // The launcher's own failure line, written back on descriptor 3 (end of file alone: the exec happened).
  let report = "";
  const reported = new Promise<void>((resolve) => {
    const r = via === undefined ? undefined : (child.stdio[3] as (Writable & Readable) | null);
    if (r === undefined || r === null || typeof r.on !== "function") return resolve();
    r.on("data", (c: Buffer) => {
      if (report.length < 4096) report += c.toString("utf8");
    });
    r.on("error", () => resolve());
    r.on("close", () => resolve());
    r.on("end", () => resolve());
  });
  let gone = false;
  const exited = new Promise<ChildExit>((resolve) => {
    child.once("exit", (code, signal) => {
      gone = true;
      const e = { code, signal };
      // What the leader left running in its group goes with it, while the group's id cannot
      // yet name another process's (under the Linux launcher the group is the launcher alone,
      // which has reaped the session).
      if (platform !== "win32" && child.pid !== undefined) {
        try {
          process.kill(-child.pid, "SIGKILL");
        } catch {
          // The group is gone.
        }
      }
      void reported.then(() => {
        onExit(e, /"code":"SANDBOX_UNAVAILABLE"/.test(report));
        resolve(e);
      });
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
          // After the exit the group's id may name another process's group.
          if (gone) return;
          try {
            if (platform === "win32") child.kill(signal);
            else if (reaps && signal === "SIGKILL") {
              // The reaper's session end; its group alone if it does not end in time.
              process.kill(-pid, "SIGHUP");
              setTimeout(() => {
                if (gone) return;
                try {
                  process.kill(-pid, "SIGKILL");
                } catch {
                  // Gone meanwhile.
                }
              }, REAP_GRACE_MS).unref();
            } else process.kill(-pid, signal);
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
