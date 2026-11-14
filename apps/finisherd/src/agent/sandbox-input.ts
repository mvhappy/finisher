// The session sandbox's types and the parts of opening one that start nothing (plan 7C Task 11,
// 7D Task 3b): the mode from the `agent` profile's status, the launcher's endpoint per OS, the
// profile input's check and the Windows grant counting. `sandbox.ts` starts the processes
// (through `sandbox/launch.ts`).
import { realpath } from "node:fs/promises";
import { isAbsolute, relative, sep } from "node:path";
import type { FinError } from "@finisher/contracts";
import type { AgentInput, FinEndpoint } from "@finisher/core-types";
import { sandboxAgentCheck, type sandboxAgentGrants } from "@finisher/fin-node";
import type { Effect } from "effect";
import { canonicalEndpoint } from "../auth/crypto.ts";
import { finError } from "../errors.ts";
import type { AclBroker } from "../sandbox/broker.ts";
import type { SandboxedChild } from "../sandbox/launch.ts";
import type { SandboxMechanism, SandboxStatus, SandboxWarning } from "../sandbox/status.ts";
import type { LinkOptions } from "./session-dirs.ts";

export { type ChildExit, LAUNCHER_UNAVAILABLE, REAP_GRACE_MS, type SandboxedChild } from "../sandbox/launch.ts";

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
  /**
   * Names this process adds over the sandbox's `env` (plan 7D Task 3b): only the harness's
   * (`harnessExtra`: Claude Code's switches, `FIN_CALL`), never a session name or a secret;
   * checked (`assertSpawnEnv`, then `assertSessionEnv` on the merge) before anything starts.
   */
  readonly env?: Readonly<Record<string, string>>;
}

export interface SessionSandbox {
  /**
   * The session environment (spec §8.5), `FIN_PROJECT_DIR` included; a host sandbox's has no
   * `FIN_*` connection, project, credential or sign-in store variable (`hostEnv`).
   */
  readonly env: Readonly<Record<string, string>>;
  /** The `agent` profile's status when the sandbox opened. */
  readonly status: SandboxMechanism;
  readonly mode: SandboxMode;
  /** The `agent` profile's persistent warning (`SANDBOX_DISABLED` where it runs unconfined). */
  readonly warning?: SandboxWarning;
  /** `workspace/<sessionId>/`, the session's working directory (a host sandbox's own). */
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
  /**
   * Where the server's one host project folder (`fin-host-<hash>`, `ensureHostProject`) is made
   * (default: the OS temporary directory); each host sandbox uses `workspace/host-<ulid>/` in it.
   */
  readonly hostRoot?: string;
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

export function grantsOf(acl: AclBroker): SessionGrants {
  let g = GRANTS.get(acl);
  if (g === undefined) {
    g = new SessionGrants();
    GRANTS.set(acl, g);
  }
  return g;
}

/** How a profile's status (`agent` by default, or `hook`) starts processes, or the refusal. */
export function sandboxMode(m: SandboxMechanism, os: string, profile: "agent" | "hook" = "agent"): SandboxMode | FinError {
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
        `the ${profile} sandbox cannot be applied on this host, so ${profile} processes do not start`,
        { profile, os, need: m.need },
        `This host needs ${m.need}.`,
      );
  }
}

export const canonical = (p: string) => realpath(p).catch(() => p);

/** `p` is `dir` or inside it (both canonical). */
export function within(p: string, dir: string): boolean {
  const r = relative(dir, p);
  return r === "" || (r !== ".." && !r.startsWith(`..${sep}`) && !isAbsolute(r));
}

/** `FIN_SERVER` and the policy's `fin` endpoint under the launcher, per OS. */
export async function launcherServer(deps: Pick<SessionSandboxDeps, "localEndpoint" | "agentListener">, platform: NodeJS.Platform): Promise<{ env: string; fin: FinEndpoint | undefined }> {
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

export function checkInput(input: AgentInput, os: string): void {
  const reason = sandboxAgentCheck(input);
  if (reason !== null) throw finError("SANDBOX_UNAVAILABLE", `the agent sandbox cannot be built: ${reason}`, { profile: "agent", os, need: reason });
}
