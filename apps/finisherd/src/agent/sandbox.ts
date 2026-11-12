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
// A **host sandbox** (`openHostSandbox`, plan 7D Task 3b) starts a CLI without a session, for
// detection and the capability probe, under the same mode rules: a throwaway directory, no
// project, no credential, no sign-in store. Non-spawning parts live in `sandbox-input.ts`.
import { type ChildProcess, spawn as spawnChild } from "node:child_process";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { Readable, Writable } from "node:stream";
import type { FinError, Harness, Session } from "@finisher/contracts";
import type { AgentInput } from "@finisher/core-types";
import { sandboxAgentGrants, sandboxContainerSid, sandboxProjectContainer, sandboxToolchainBase } from "@finisher/fin-node";
import { Effect, type Scope } from "effect";
import { finError, toFinError } from "../errors.ts";
import { grantTable, sessionOwner } from "../sandbox/broker.ts";
import { resolveSandboxExecPath } from "../sandbox/exec.ts";
import { currentSandboxStatus, profileWarning } from "../sandbox/status.ts";
import { assertSessionEnv, assertSpawnEnv, harnessExtra, hostEnv, sessionEnv } from "./sandbox-env.ts";
import {
  canonical,
  type ChildExit,
  checkInput,
  grantsOf,
  LAUNCHER_UNAVAILABLE,
  launcherServer,
  REAP_GRACE_MS,
  type SandboxedChild,
  type SandboxMode,
  sandboxMode,
  type SessionSandbox,
  type SessionSandboxDeps,
  type SandboxViolation,
  within,
} from "./sandbox-input.ts";
import { ensureHostDirs, ensureSessionDirs, linkSignInStore, removeHostDirs, type SessionDirs } from "./session-dirs.ts";

export { assertSessionEnv, sessionEnv } from "./sandbox-env.ts";
export {
  type ChildExit,
  LAUNCHER_UNAVAILABLE,
  REAP_GRACE_MS,
  type SandboxedChild,
  type SandboxMode,
  sandboxMode,
  type SandboxViolation,
  SessionGrants,
  type SessionSandbox,
  type SessionSandboxDeps,
  type SpawnOptions,
} from "./sandbox-input.ts";

/** What opening a sandbox prepared: its environment, working roots and profile input. */
interface Prepared {
  readonly env: Record<string, string>;
  readonly workspace: string;
  /** Where a process may start (at any depth). */
  readonly roots: readonly string[];
  readonly input: AgentInput;
  /** Under the launcher, after the input's check and before the grants (Windows: the pipe). */
  readonly beforeGrant?: () => void;
}

/** One sandbox: `prepare` makes its directories and input; `cleanup` runs when it is gone. */
interface BoxSpec {
  readonly id: string;
  readonly harness: Harness;
  /** A host sandbox (no session): its environment is checked as `hostEnv`'s. */
  readonly host: boolean;
  readonly prepare: (mode: SandboxMode, os: string) => Promise<Prepared>;
  readonly cleanup?: () => Promise<void>;
}

/**
 * Opens `s`'s sandbox in the current scope (spec §8.5, §8.6): its directories, the sign-in
 * store link, the environment and the profile input, checked by `fin-sandbox` now so a bad
 * input fails here rather than at each process. Closing the scope kills every process it
 * started (their whole groups).
 */
export function openSessionSandbox(s: Session, harness: Harness, deps: SessionSandboxDeps): Effect.Effect<SessionSandbox, FinError, Scope.Scope> {
  const platform = deps.platform ?? process.platform;
  return openBox(
    {
      id: s.id,
      harness,
      host: false,
      prepare: async (mode, os) => {
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
        const input: AgentInput = {
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
        // The project's container reaches `fin` over the pipe (spec §8.11): its SID is allowed
        // there, whatever the grant table names.
        const beforeGrant = () => {
          if (platform !== "win32" || deps.acl?.active !== true || deps.allowPipe === undefined) return;
          const sid = (deps.containerSid ?? sandboxContainerSid)(sandboxProjectContainer(projectDir));
          if (sid === null) throw finError("SANDBOX_UNAVAILABLE", "no SID for the project's container", { profile: "agent", os, need: "the project's container SID" });
          deps.allowPipe(sid);
        };
        return { env, workspace, roots: [workspace, shared], input, beforeGrant };
      },
    },
    deps,
  );
}

/**
 * Opens a host sandbox in the current scope (plan 7D Task 3b, Decisions 8 and 22): a CLI started
 * without a session, for `detect` (its version, its sign-in status) or `probe` (its flags and
 * first lines). Its directory `agents/host-<ulid>/` is the CLI's provider configuration
 * directory and temporary directory; its workspace is an empty throwaway folder outside server
 * data (`fin-sandbox`'s `agent` input needs a project folder, and that one holds nothing). No
 * project, no credential, no `fin` endpoint, no sign-in store variable and no store grant: an
 * adapter that needs the store (`auth status`) passes nothing of it here. The same mode rules
 * as a session's; closing the scope kills every process it started, then removes both folders.
 */
export function openHostSandbox(purpose: "detect" | "probe", harness: Harness, deps: SessionSandboxDeps): Effect.Effect<SessionSandbox, FinError, Scope.Scope> {
  const platform = deps.platform ?? process.platform;
  const made: { dirs?: SessionDirs; project?: string } = {};
  // The id is known once the directory is made; `openBox` reads it lazily for its errors.
  const spec: { -readonly [K in keyof BoxSpec]: BoxSpec[K] } = {
    id: `host-${purpose}`,
    harness,
    host: true,
    prepare: async () => {
      const { id, dirs } = await ensureHostDirs(join(deps.serverData, "agents"), { platform });
      made.dirs = dirs;
      spec.id = id;
      const project = await canonical(await mkdtemp(join(deps.hostRoot ?? tmpdir(), "fin-host-")));
      made.project = project;
      const workspace = join(project, "workspace", id);
      await mkdir(workspace, { recursive: true, mode: 0o700 });
      const env = hostEnv({ harness, dirs, platform, ...(deps.parent === undefined ? {} : { parent: deps.parent }) });
      assertSessionEnv(env, { id, harness }, { host: true, expect: env, platform });
      const input: AgentInput = {
        sessionId: id,
        mode: "plan",
        harness,
        projectDir: project,
        workspaceDir: workspace,
        libraryRoots: [],
        sessionDir: dirs.dir,
        toolchains: sandboxToolchainBase(),
        serverData: deps.serverData,
        home: await canonical(homedir()),
      };
      return { env, workspace, roots: [workspace], input };
    },
    cleanup: async () => {
      if (made.project !== undefined) await rm(made.project, { recursive: true, force: true, maxRetries: 3 }).catch(() => 0);
      if (made.dirs !== undefined) await removeHostDirs(made.dirs).catch(() => 0);
    },
  };
  return openBox(spec, deps);
}

/** The sandbox of `b` in the current scope: the mode, the grants, `spawn` and the kill at close. */
function openBox(b: BoxSpec, deps: SessionSandboxDeps): Effect.Effect<SessionSandbox, FinError, Scope.Scope> {
  const platform = deps.platform ?? process.platform;
  const children = new Set<SandboxedChild>();
  // Set once the scope closes: a spawn still in flight is killed as it arrives.
  const closed = { v: false };
  // Windows: the broker's grants are made under the sandbox's owner (revoked at scope close).
  const acl = platform === "win32" && deps.acl?.active === true ? deps.acl : undefined;
  const scopes = acl === undefined ? undefined : (deps.sessionGrants ?? grantsOf(acl));
  // Set once this scope counts in `scopes` (it then leaves at close, or when the open fails).
  let entered: string | undefined;
  const leave = async () => {
    const owner = entered;
    if (owner === undefined || acl === undefined || scopes === undefined) return;
    entered = undefined;
    // A revoke that fails stays in the broker's log; the next start's sweep completes it.
    await scopes.leave(owner, () => acl.revoke(owner));
  };
  const grant = async (input: AgentInput, extraRead?: readonly string[]) => {
    if (acl === undefined) return;
    const table = (deps.agentGrants ?? sandboxAgentGrants)(input, extraRead);
    await grantTable(acl, table, { owner: sessionOwner(b.id), ...(deps.containerSid === undefined ? {} : { containerSid: deps.containerSid }) });
  };
  const gone = async () => {
    await leave();
    await b.cleanup?.();
  };
  const open = Effect.tryPromise({
    try: async (): Promise<SessionSandbox> => {
      const status = (deps.status ?? currentSandboxStatus)();
      const mode = sandboxMode(status.agent, status.os);
      if (typeof mode !== "string") throw mode;
      const p = await b.prepare(mode, status.os);
      const { env, workspace, input: base } = p;
      if (mode === "launcher") {
        checkInput(base, status.os);
        p.beforeGrant?.();
        // Before any process: what the container may reach (a failure, here or later in the
        // open, revokes what was granted unless another open scope of the session uses it:
        // `leave` below).
        if (scopes !== undefined) {
          entered = sessionOwner(b.id);
          await scopes.enter(entered);
        }
        await grant(base);
      }
      const launcher = mode === "launcher" ? (deps.launcher ?? resolveSandboxExecPath)() : "";
      const warning = profileWarning("agent", status.os, status.agent);
      return {
        env,
        status: status.agent,
        mode,
        ...(warning === undefined ? {} : { warning }),
        workspace,
        spawn: (argv, opts) =>
          Effect.tryPromise({
            try: async () => {
              if (argv.length === 0) throw finError("USAGE", "nothing to start", { sessionId: b.id });
              const cwd = await canonical(opts.cwd);
              if (!p.roots.some((r) => within(cwd, r))) {
                throw finError("USAGE", "a session's process starts in its workspace", { sessionId: b.id, reason: "cwdOutsideWorkspace" });
              }
              // The process's own names, over the sandbox's and held to its contract, before anything starts.
              let childEnv = env;
              if (opts.env !== undefined) {
                const s = { id: b.id, harness: b.harness };
                assertSpawnEnv(opts.env, s, platform);
                childEnv = { ...env, ...opts.env };
                assertSessionEnv(childEnv, s, { expect: env, extra: harnessExtra(b.harness), host: b.host, platform });
              }
              let input = base;
              if (opts.extraRead !== undefined && opts.extraRead.length > 0) {
                const extra = await Promise.all(opts.extraRead.map(canonical));
                input = { ...base, toolchains: [...base.toolchains!, ...extra] };
                if (mode === "launcher") {
                  checkInput(input, status.os);
                  // The session's own (revoked with it), never static toolchain grants.
                  await grant(input, extra);
                }
              }
              if (closed.v) throw finError("SESSION_INTERRUPTED", "the session's sandbox is closed", { sessionId: b.id });
              const child = await start(mode === "launcher" ? { launcher, request: { profile: "agent", input } } : undefined, argv, cwd, childEnv, platform, (exit, launcherFailed) => {
                if (launcherFailed && exit.code === LAUNCHER_UNAVAILABLE) deps.onViolation?.({ sessionId: b.id, op: "apply", profile: "agent", rootKind: "none" });
              });
              children.add(child);
              void child.exited.then(() => children.delete(child));
              if (closed.v) {
                child.kill("SIGKILL");
                throw finError("SESSION_INTERRUPTED", "the session's sandbox closed while the process started", { sessionId: b.id });
              }
              return child;
            },
            catch: toFinError,
          }),
      };
    },
    catch: toFinError,
  });
  const opened = open.pipe(Effect.tapError(() => Effect.promise(gone)));
  return Effect.acquireRelease(opened, () =>
    Effect.promise(async () => {
      closed.v = true;
      await killAll(children, deps.killGraceMs ?? 5_000);
      await gone();
    }),
  );
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
