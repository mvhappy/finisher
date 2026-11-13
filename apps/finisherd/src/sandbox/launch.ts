// Starting a sandboxed process (moved out of the session sandbox for hooks, 7F plan Task 3): the
// one place `agent/` and `hooks/` reach `child_process` for a profile's process (a gate,
// `scripts/check-agent-spawn.mjs`, keeps it out of both folders but the session sandbox and our
// own `fin`). A process starts either directly (the profile not ready on this OS) or as
// `fin-sandbox-exec --policy-fd 3 -- <argv>`, its `LaunchRequest {profile, input}` written on
// descriptor 3 (never argv or the environment). Every process starts in a process group of its
// own; when its leader exits the group goes with it. On Linux and macOS the launcher stays the
// program's parent and reaps every descendant when the program exits, is hung up or loses its
// parent, so a kill with `SIGKILL` of a launched process hangs it up (`SIGHUP`, which it never
// forwards) and falls back to `SIGKILL` on its group after `REAP_GRACE_MS`. What the launcher
// itself reports (its line on descriptor 3 before exit 78) is passed to `onExit` as
// `launcherFailed`.
import { type ChildProcess, spawn as spawnChild } from "node:child_process";
import type { Readable, Writable } from "node:stream";
import { finError } from "../errors.ts";

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

/** The profiles the launcher starts (`fin_sandbox::launch::LaunchRequest`). */
export type LaunchProfile = "agent" | "hook";

/** A start through the launcher: its path and the request it reads on descriptor 3. */
export interface LaunchVia {
  readonly launcher: string;
  readonly request: { readonly profile: LaunchProfile; readonly input: unknown };
}

/**
 * Starts one process (through the launcher when `via` is given, its request on descriptor 3)
 * in a process group of its own. `profile` names the profile in a `SANDBOX_UNAVAILABLE` of a
 * direct start (default: the request's, else `agent`).
 */
export function startProcess(
  via: LaunchVia | undefined,
  argv: readonly string[],
  cwd: string,
  env: Readonly<Record<string, string>>,
  platform: NodeJS.Platform,
  onExit: (e: ChildExit, launcherFailed: boolean) => void,
  profile: LaunchProfile = via?.request.profile ?? "agent",
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
      reject(finError("SANDBOX_UNAVAILABLE", `cannot start ${via === undefined ? program : "the sandbox launcher"}: ${e.message}`, { profile, program: via === undefined ? program : "fin-sandbox-exec" })),
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

/** Kills every child (their groups) and waits for them, at most `graceMs`. */
export async function killAll(children: Set<SandboxedChild>, graceMs: number): Promise<void> {
  const all = [...children];
  for (const c of all) c.kill("SIGKILL");
  await Promise.race([Promise.all(all.map((c) => c.exited)), new Promise((r) => setTimeout(r, graceMs).unref())]);
}

