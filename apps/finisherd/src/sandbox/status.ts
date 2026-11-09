// The one sandbox status source of the server (plan 7C Decision 23, spec §8.9). fin-node's
// `sandboxStatus()` answers per profile: `enforced`, `disabled {reason}` or `unavailable
// {need}`. From it come:
// - the persistent warnings of `server.status`, the agent runtime and the start-up log:
//   `SANDBOX_DISABLED {profile, os, reason, fix}` for a disabled profile (`PLUGINS_UNCONFINED`,
//   sp8's name, for plugins), `SANDBOX_UNAVAILABLE {profile, os, need}` for one whose start is
//   refused. F2 (owner decision): a disabled profile runs, unconfined and visibly so; there is
//   no setting that silences a warning or opts out of an enforced profile;
// - `SANDBOX_SOCKETS_EXPOSED {profile, os, reason, fix}` for an enforced `agent` or `hook`
//   profile where the launcher cannot hide Unix sockets in a namespace (plan 7C Decision 26):
//   those processes then reach the user's sockets (D-Bus, ssh-agent, docker.sock, fin.sock).
//   Only the launcher binary can tell (an AppArmor profile attaches to it by path), so its
//   `--probe` answer is read once per server process;
// - the `sandbox.unconfined {profile, os, reason}` journal event, once per profile per server
//   start and again when its reason changes (`SandboxReporter`).
import { execFileSync } from "node:child_process";
import type { SandboxLauncherProbe, SandboxNamespaces, SandboxProfile } from "@finisher/core-types";
import type { SandboxMechanism, SandboxStatus } from "@finisher/fin-node";
import { sandboxStatus } from "@finisher/fin-node";
import { resolveSandboxExecPath } from "./exec.ts";

export const SANDBOX_PROFILES: ReadonlyArray<SandboxProfile> = ["agent", "hook", "worker", "plugin"];
export type { SandboxMechanism, SandboxNamespaces, SandboxProfile, SandboxStatus };

export interface SandboxWarning {
  readonly code: "SANDBOX_DISABLED" | "PLUGINS_UNCONFINED" | "SANDBOX_UNAVAILABLE" | "SANDBOX_SOCKETS_EXPOSED";
  readonly message: string;
  readonly details: Readonly<Record<string, unknown>>;
  readonly fix: string;
}

/** The `fix` of `reason: landlockDisabled`: the kernel's `lsm=` boot parameter. */
export const LANDLOCK_FIX =
  "Turn Landlock on: add landlock to the kernel's lsm= boot parameter (for example lsm=landlock,lockdown,yama,integrity,apparmor), then reboot.";
/** The `fix` of `reason: profileNotReady`. */
export const NOT_READY_FIX =
  "This sandbox is not yet validated on this operating system; the release notes say when it is. Until then nothing turns this warning off.";

/** The `fix` of `SANDBOX_SOCKETS_EXPOSED`. */
export const SOCKETS_FIX =
  "Let fin-sandbox-exec create user namespaces: on Ubuntu 24.04 and later install Finisher's AppArmor profile (resources/linux/apparmor/finisher-sandbox-exec, copied to /etc/apparmor.d/ and loaded with apparmor_parser -r; the installer does this); elsewhere enable unprivileged user namespaces.";

const SOCKETS_EXPOSED: Partial<Record<SandboxProfile, string>> = {
  agent: "Agent tools can connect to the Unix sockets your user can open (the D-Bus session bus, ssh-agent, docker.sock, the server's socket): this host does not let the sandbox hide them.",
  hook: "Hooks can connect to the Unix sockets your user can open (the D-Bus session bus, ssh-agent, docker.sock) and send UDP: this host does not let the sandbox hide them.",
};

/** The warning for an enforced `agent` or `hook` profile whose sockets the launcher cannot hide. */
export function socketsWarning(profile: SandboxProfile, os: string, m: SandboxMechanism, ns: SandboxNamespaces): SandboxWarning | undefined {
  const message = SOCKETS_EXPOSED[profile];
  if (message === undefined || m.kind !== "enforced" || ns.kind !== "unavailable") return undefined;
  return { code: "SANDBOX_SOCKETS_EXPOSED", message, details: { profile, os, reason: ns.reason }, fix: SOCKETS_FIX };
}

const UNCONFINED: Record<SandboxProfile, string> = {
  agent: "Agent tools run as your user, without a sandbox: a tool can bypass the session's scopes (for example by using local.key). Run agents only on projects and prompts you trust.",
  hook: "Hooks run as your user, without a sandbox. Use only hooks you trust.",
  worker: "Job workers run without a sandbox: only the cleared environment, private job folders and the kill tree confine them.",
  plugin: "Plugin processes run without a sandbox: only the cleared environment, private job folders and the kill tree confine them. Run only plugins you trust.",
};

/** The warning for one profile's status, or none when it is enforced. */
export function profileWarning(profile: SandboxProfile, os: string, m: SandboxMechanism): SandboxWarning | undefined {
  switch (m.kind) {
    case "enforced":
      return undefined;
    case "disabled":
      return {
        code: profile === "plugin" ? "PLUGINS_UNCONFINED" : "SANDBOX_DISABLED",
        message: UNCONFINED[profile],
        details: { profile, os, reason: m.reason },
        fix: m.reason === "landlockDisabled" ? LANDLOCK_FIX : NOT_READY_FIX,
      };
    case "unavailable":
      return {
        code: "SANDBOX_UNAVAILABLE",
        message: `the ${profile} sandbox cannot be applied on this host, so ${profile} processes do not start`,
        details: { profile, os, need: m.need },
        fix: `This host needs ${m.need}.`,
      };
  }
}

let source: () => SandboxStatus = sandboxStatus;

/** The status now (one Landlock version query on Linux). */
export function currentSandboxStatus(): SandboxStatus {
  return source();
}

/** Replaces the status source (tests); returns the previous one. */
export function setSandboxStatusSource(next: () => SandboxStatus): () => SandboxStatus {
  const prev = source;
  source = next;
  return prev;
}

/** What `fin-sandbox-exec --probe` answers here: run once, on first need. */
export function probeLauncher(): SandboxNamespaces {
  if (process.platform !== "linux") return { kind: "notApplicable" };
  const stub = process.env.FIN_TEST_SANDBOX_NAMESPACES;
  try {
    const out = execFileSync(resolveSandboxExecPath(), ["--probe"], {
      encoding: "utf8",
      timeout: 10_000,
      env: stub === undefined ? {} : { FIN_TEST_SANDBOX_NAMESPACES: stub },
      stdio: ["ignore", "pipe", "pipe"],
    });
    return (JSON.parse(out) as SandboxLauncherProbe).namespaces;
  } catch (e) {
    return { kind: "unavailable", reason: `the launcher's probe failed: ${(e as Error).message.split("\n")[0]}` };
  }
}

let namespaces: SandboxNamespaces | undefined;
let namespacesSource: () => SandboxNamespaces = probeLauncher;

/** Whether the launcher hides Unix sockets on this host (cached for the process). */
export function currentNamespaces(): SandboxNamespaces {
  namespaces ??= namespacesSource();
  return namespaces;
}

/** Replaces the namespace answer's source (tests) and forgets the cached one; returns the previous source. */
export function setNamespacesSource(next: () => SandboxNamespaces): () => SandboxNamespaces {
  const prev = namespacesSource;
  namespacesSource = next;
  namespaces = undefined;
  return prev;
}

/**
 * The warnings of `profiles`, in that order: each profile's status warning, then for an
 * enforced `agent` or `hook` `SANDBOX_SOCKETS_EXPOSED` where the namespace is unavailable
 * (`ns` is read only when one is enforced).
 */
export function sandboxWarnings(status: SandboxStatus, profiles: ReadonlyArray<SandboxProfile> = SANDBOX_PROFILES, ns?: SandboxNamespaces): SandboxWarning[] {
  return profiles.flatMap((p) => {
    const m = status[p];
    const own = profileWarning(p, status.os, m);
    if (m.kind !== "enforced" || SOCKETS_EXPOSED[p] === undefined) return own === undefined ? [] : [own];
    const sockets = socketsWarning(p, status.os, m, ns ?? currentNamespaces());
    return [own, sockets].filter((w): w is SandboxWarning => w !== undefined);
  });
}

interface JournalLike {
  readonly enabled: boolean;
  record(kind: string, kv: number, ctxJson: string | null, payloadJson: string, source: string): void;
}

/**
 * Records `sandbox.unconfined` for each disabled profile: once per server start (one reporter
 * per start) and again when the reason changes. A change is noticed when the status is read
 * again (`server.status`), not by polling. A profile that becomes enforced or unavailable
 * is forgotten, so a later disable is recorded again. Never throws.
 */
export class SandboxReporter {
  readonly #last = new Map<SandboxProfile, string>();
  readonly #journal: JournalLike | undefined;
  readonly #log: (message: string) => void;

  constructor(journal: JournalLike | undefined, log: (message: string) => void = () => {}) {
    this.#journal = journal;
    this.#log = log;
  }

  report(status: SandboxStatus): void {
    for (const profile of SANDBOX_PROFILES) {
      const m = status[profile];
      if (m.kind !== "disabled") {
        this.#last.delete(profile);
        continue;
      }
      if (this.#last.get(profile) === m.reason) continue;
      // Remembered only once written, so diagnostics turned on later still get this start's event.
      if (this.#journal === undefined || !this.#journal.enabled) continue;
      this.#last.set(profile, m.reason);
      try {
        this.#journal.record("sandbox.unconfined", 1, null, JSON.stringify({ profile, os: status.os, reason: m.reason }), "finisherd");
      } catch (e) {
        this.#log(`sandbox: the journal refused a sandbox.unconfined: ${(e as Error).message}`);
      }
    }
  }
}

const reporters = new WeakMap<object, SandboxReporter>();

/**
 * The reporter of the server start that opened `journal` (one journal per start, so one
 * reporter per start): `main.ts` reports at start, `server.status` on each call.
 */
export function reporterFor(journal: JournalLike & object, log?: (message: string) => void): SandboxReporter {
  let r = reporters.get(journal);
  if (r === undefined) {
    r = new SandboxReporter(journal, log);
    reporters.set(journal, r);
  }
  return r;
}
