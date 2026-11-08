// The one sandbox status source of the server (plan 7C Decision 23, spec §8.9). fin-node's
// `sandboxStatus()` answers per profile: `enforced`, `disabled {reason}` or `unavailable
// {need}`. From it come:
// - the persistent warnings of `server.status`, the agent runtime and the start-up log:
//   `SANDBOX_DISABLED {profile, os, reason, fix}` for a disabled profile (`PLUGINS_UNCONFINED`,
//   sp8's name, for plugins), `SANDBOX_UNAVAILABLE {profile, os, need}` for one whose start is
//   refused. F2 (owner decision): a disabled profile runs, unconfined and visibly so; there is
//   no setting that silences a warning or opts out of an enforced profile;
// - the `sandbox.unconfined {profile, os, reason}` journal event, once per profile per server
//   start and again when its reason changes (`SandboxReporter`).
import type { SandboxMechanism, SandboxStatus } from "@finisher/fin-node";
import { sandboxStatus } from "@finisher/fin-node";

export const SANDBOX_PROFILES = ["agent", "hook", "worker", "plugin"] as const;
export type SandboxProfile = (typeof SANDBOX_PROFILES)[number];
export type { SandboxMechanism, SandboxStatus };

export interface SandboxWarning {
  readonly code: "SANDBOX_DISABLED" | "PLUGINS_UNCONFINED" | "SANDBOX_UNAVAILABLE";
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

/** The warnings of `profiles`, in that order. */
export function sandboxWarnings(status: SandboxStatus, profiles: ReadonlyArray<SandboxProfile> = SANDBOX_PROFILES): SandboxWarning[] {
  return profiles.flatMap((p) => profileWarning(p, status.os, status[p]) ?? []);
}

interface JournalLike {
  readonly enabled: boolean;
  record(kind: string, kv: number, ctxJson: string | null, payloadJson: string, source: string): void;
}

/**
 * Records `sandbox.unconfined` for each disabled profile: once per server start (one reporter
 * per start) and again when the reason changes. A profile that becomes enforced or unavailable
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
      this.#last.set(profile, m.reason);
      if (this.#journal === undefined || !this.#journal.enabled) continue;
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
