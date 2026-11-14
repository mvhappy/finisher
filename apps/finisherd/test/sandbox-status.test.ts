// The sandbox status source and its warnings (plan 7C Task 3, F2): a disabled profile runs with
// persistent warnings, `sandbox.unconfined` is recorded once per profile per start and again on
// change, a worker whose sandbox cannot apply fails `SANDBOX_UNAVAILABLE`, and nothing opts out.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { agentWarnings } from "../src/agent/warnings.ts";
import { warningLine } from "../src/console/commands/status.ts";
import type { Audit } from "../src/control/audit.ts";
import { exitVerdict } from "../src/jobs/host.ts";
import {
  LANDLOCK_FIX,
  NOT_READY_FIX,
  type SandboxMechanism,
  SandboxReporter,
  type SandboxNamespaces,
  type SandboxStatus,
  SOCKETS_FIX,
  probeLauncher,
  sandboxWarnings,
  setNamespacesSource,
  setSandboxStatusSource,
} from "../src/sandbox/status.ts";
import { serverStatus } from "../src/server-admin.ts";

const all = (m: SandboxMechanism, os = "linux"): SandboxStatus => ({ os, agent: m, hook: m, worker: m, plugin: m }) as SandboxStatus;
const DISABLED: SandboxMechanism = { kind: "disabled", reason: "landlockDisabled" };
const NOT_READY: SandboxMechanism = { kind: "disabled", reason: "profileNotReady" };
const ENFORCED: SandboxMechanism = { kind: "enforced", detail: "landlock abi 8" };

const caps = { killTree: "processGroup", watchdog: true, cgroupMemory: false, leafLimits: false, sandbox: "none", sandboxed: false, profiles: { worker: "disabled", plugin: "disabled" }, perJobGrants: false } as const;
const deps = (status: SandboxStatus, reporter?: SandboxReporter) => ({
  control: { envId: "env", listeners: () => [], stop: () => {} },
  audit: { failures: () => 0 } as unknown as Audit,
  connections: { count: () => ({}) } as never,
  journalEnabled: () => false,
  jobCaps: () => caps as never,
  sandbox: () => status,
  ...(reporter === undefined ? {} : { sandboxReporter: reporter }),
});

const STUB = "FIN_TEST_SANDBOX_STATUS";
const NS_STUB = "FIN_TEST_SANDBOX_NAMESPACES";
const AVAILABLE: SandboxNamespaces = { kind: "available" };
const NO_NS: SandboxNamespaces = { kind: "unavailable", reason: "mount --make-rprivate /: Permission denied (os error 13)" };
// Every case answers "available" unless it sets its own: the host's real answer varies.
const nsPrev = setNamespacesSource(() => AVAILABLE);
afterEach(() => {
  delete process.env[STUB];
  delete process.env[NS_STUB];
  setNamespacesSource(() => AVAILABLE);
});

describe("sandbox status", () => {
  test("the_native_status_follows_the_test_stub", () => {
    process.env[STUB] = "disabled";
    const s = sandboxStatus();
    for (const p of ["agent", "hook", "worker", "plugin"] as const) expect(s[p]).toEqual(DISABLED);
    process.env[STUB] = "unavailable";
    // `unavailable` refuses only a ready profile, and none is yet (`READY` is empty): every
    // profile, `worker` included, runs as before 7C (plan 7C Task 7: status and spawner agree).
    const u = sandboxStatus();
    expect(u.worker).toEqual(NOT_READY);
    expect(u.agent).toEqual(NOT_READY);
    delete process.env[STUB];
    expect(["linux", "macos", "windows"]).toContain(sandboxStatus().os);
  });

  test("landlock_disabled_warns_for_every_profile_and_refuses_nothing", async () => {
    const s = await Effect.runPromise(serverStatus(deps(all(DISABLED))));
    expect(s.warnings[0]).toMatchObject({
      code: "SANDBOX_DISABLED",
      sandbox: "none",
      details: { profile: "worker", os: "linux", reason: "landlockDisabled" },
      fix: LANDLOCK_FIX,
    });
    expect(LANDLOCK_FIX).toContain("lsm=");
    const byProfile = Object.fromEntries(s.warnings.map((w) => [(w.details as { profile: string }).profile, w.code]));
    expect(byProfile).toEqual({ worker: "SANDBOX_DISABLED", plugin: "PLUGINS_UNCONFINED", hook: "SANDBOX_DISABLED", agent: "SANDBOX_DISABLED" });
    expect(s.warnings.map((w) => w.code)).not.toContain("SANDBOX_UNAVAILABLE");
    // The agent runtime answers the same warning.
    expect(agentWarnings(all(DISABLED))).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", details: { profile: "agent", os: "linux", reason: "landlockDisabled" } })]);
  });

  test("a_profile_not_ready_keeps_its_warning_and_an_enforced_one_has_none", async () => {
    expect(sandboxWarnings(all(NOT_READY)).map((w) => w.fix)).toEqual([NOT_READY_FIX, NOT_READY_FIX, NOT_READY_FIX, NOT_READY_FIX]);
    expect(sandboxWarnings(all(ENFORCED))).toEqual([]);
    // Enforced, but the job host's spawner applies nothing yet: workers still warn.
    const s = await Effect.runPromise(serverStatus(deps(all(ENFORCED))));
    expect(s.warnings).toEqual([expect.objectContaining({ code: "SANDBOX_DISABLED", sandbox: "none", details: expect.objectContaining({ profile: "worker", reason: "spawnerUnconfined" }) })]);
    const unavailable = sandboxWarnings(all({ kind: "unavailable", need: "kernel ≥ 6.2" }), ["worker"]);
    expect(unavailable).toEqual([expect.objectContaining({ code: "SANDBOX_UNAVAILABLE", details: { profile: "worker", os: "linux", need: "kernel ≥ 6.2" } })]);
  });

  test("sandbox_unconfined_is_recorded_once_per_profile_per_start_and_on_change", async () => {
    const records: Array<{ kind: string; payload: unknown }> = [];
    const journal = { enabled: true, record: (kind: string, _kv: number, _ctx: string | null, payload: string) => void records.push({ kind, payload: JSON.parse(payload) }) };
    const reporter = new SandboxReporter(journal);
    await Effect.runPromise(serverStatus(deps(all(DISABLED), reporter)));
    await Effect.runPromise(serverStatus(deps(all(DISABLED), reporter)));
    expect(records).toEqual(
      ["agent", "hook", "worker", "plugin"].map((profile) => ({ kind: "sandbox.unconfined", payload: { profile, os: "linux", reason: "landlockDisabled" } })),
    );
    records.length = 0;
    reporter.report({ ...all(DISABLED), worker: NOT_READY });
    expect(records).toEqual([{ kind: "sandbox.unconfined", payload: { profile: "worker", os: "linux", reason: "profileNotReady" } }]);
    records.length = 0;
    // Enforced, then disabled again: a change, recorded again.
    reporter.report({ ...all(DISABLED), worker: ENFORCED });
    reporter.report(all(DISABLED));
    expect(records).toEqual([{ kind: "sandbox.unconfined", payload: { profile: "worker", os: "linux", reason: "landlockDisabled" } }]);
    // A new start records again; a disabled journal records nothing.
    records.length = 0;
    new SandboxReporter(journal).report(all(DISABLED));
    expect(records).toHaveLength(4);
    records.length = 0;
    const off = { ...journal, enabled: false };
    const later = new SandboxReporter(off);
    later.report(all(DISABLED));
    expect(records).toEqual([]);
    // Diagnostics turned on later in the same start: that start's events are still recorded.
    off.enabled = true;
    later.report(all(DISABLED));
    expect(records).toHaveLength(4);
  });

  test("a_worker_whose_sandbox_cannot_apply_fails_sandbox_unavailable", () => {
    const v = exitVerdict(
      { reason: "spawnFailed", what: "selfSandbox", code: 78, stderrTail: 'fin-jobs: self-sandbox "worker" failed: the sandbox needs kernel ≥ 6.2\n', lifetimeMs: 3 } as never,
      false,
    );
    expect(v.error).toMatchObject({
      code: "SANDBOX_UNAVAILABLE",
      details: { profile: "worker", os: sandboxStatus().os, need: "kernel ≥ 6.2" },
    });
    // A failure that is not a missing mechanism names no path.
    const bad = exitVerdict({ reason: "spawnFailed", what: "selfSandbox", stderrTail: 'fin-jobs: self-sandbox "worker" failed: unreadable worker input: /home/x\n', lifetimeMs: 1 } as never, false);
    expect(bad.error).toMatchObject({ code: "SANDBOX_UNAVAILABLE", details: { need: "a sandbox policy this host can apply" } });
    // Any other spawn failure is still `WORKER_UNAVAILABLE`.
    expect(exitVerdict({ reason: "spawnFailed", what: "thread", stderrTail: "", lifetimeMs: 1 } as never, false).error.code).toBe("WORKER_UNAVAILABLE");
  });

  test("finisherd_status_prints_the_reason_and_the_fix", () => {
    const [w] = sandboxWarnings(all(DISABLED), ["worker"]);
    const line = warningLine(w as never);
    expect(line).toContain("SANDBOX_DISABLED (worker, landlockDisabled)");
    expect(line).toContain("lsm=");
  });

  test("no_setting_opts_out_of_the_sandbox", () => {
    // Spec §16.2: there is no `agents.unsandboxed` (or any other) opt-out to read.
    const src = join(import.meta.dirname, "../src");
    const files = (dir: string): string[] =>
      readdirSync(dir).flatMap((n) => (statSync(join(dir, n)).isDirectory() ? files(join(dir, n)) : n.endsWith(".ts") ? [join(dir, n)] : []));
    const hits = files(src).filter((f) => /unsandboxed["']?\s*[:\]]|agents\.unsandboxed/.test(readFileSync(f, "utf8")));
    expect(hits).toEqual([]);
  });
});

describe("sockets exposed (plan 7C Decision 26)", () => {
  test("an_enforced_agent_or_hook_without_namespaces_warns_sockets_exposed", async () => {
    const codes = (ns: SandboxNamespaces, m: SandboxMechanism = ENFORCED) =>
      sandboxWarnings(all(m), undefined, ns).map((w) => `${w.code}:${String(w.details.profile)}`);
    expect(codes(NO_NS)).toEqual(["SANDBOX_SOCKETS_EXPOSED:agent", "SANDBOX_SOCKETS_EXPOSED:hook"]);
    expect(codes(AVAILABLE)).toEqual([]);
    expect(codes({ kind: "notApplicable" })).toEqual([]);
    // A disabled or not-ready profile already warns that it runs unconfined: nothing more.
    expect(codes(NO_NS, DISABLED)).not.toContain("SANDBOX_SOCKETS_EXPOSED:agent");
    expect(codes(NO_NS, NOT_READY).filter((c) => c.startsWith("SANDBOX_SOCKETS"))).toEqual([]);
    const [w] = sandboxWarnings(all(ENFORCED), ["agent"], NO_NS);
    expect(w).toEqual({
      code: "SANDBOX_SOCKETS_EXPOSED",
      message: expect.stringContaining("ssh-agent"),
      details: { profile: "agent", os: "linux", reason: NO_NS.reason },
      fix: SOCKETS_FIX,
    });
    expect(SOCKETS_FIX).toContain("apparmor");
  });

  test("server_status_the_agent_runtime_and_finisherd_status_show_it", async () => {
    setNamespacesSource(() => NO_NS);
    const s = await Effect.runPromise(serverStatus(deps(all(ENFORCED))));
    expect(s.warnings.filter((w) => w.code === "SANDBOX_SOCKETS_EXPOSED").map((w) => (w.details as { profile: string }).profile).sort()).toEqual(["agent", "hook"]);
    expect(agentWarnings(all(ENFORCED))).toEqual([expect.objectContaining({ code: "SANDBOX_SOCKETS_EXPOSED", details: expect.objectContaining({ profile: "agent" }) })]);
    const line = warningLine(agentWarnings(all(ENFORCED))[0] as never);
    expect(line).toContain("SANDBOX_SOCKETS_EXPOSED (agent");
    expect(line).toContain("apparmor");
    setNamespacesSource(() => AVAILABLE);
    expect(agentWarnings(all(ENFORCED))).toEqual([]);
  });

  test.runIf(process.platform === "linux")("the_launcher_probe_answers_and_follows_its_test_stub", () => {
    process.env[NS_STUB] = "unavailable";
    expect(probeLauncher()).toEqual({ kind: "unavailable", reason: "stubbed" });
    delete process.env[NS_STUB];
    expect(["available", "unavailable"]).toContain(probeLauncher().kind);
    expect(nsPrev).toBe(probeLauncher);
  });
});

describe("statuses in use", () => {
  test("the_default_source_is_fin_node", () => {
    const prev = setSandboxStatusSource(() => all(DISABLED));
    try {
      expect(agentWarnings()).toHaveLength(1);
    } finally {
      setSandboxStatusSource(prev);
    }
    expect(prev).toBe(sandboxStatus);
  });
});

describe("plugins unconfined (sp8b Decision 17)", () => {
  const pluginWarnings = async (status: SandboxStatus, enabled: number, profiles: Record<string, string> = caps.profiles) => {
    const s = await Effect.runPromise(serverStatus({ ...deps(status), jobCaps: () => ({ ...caps, profiles }) as never, enabledCodePlugins: () => enabled }));
    return s.warnings.filter((w) => (w.details as { profile?: string }).profile === "plugin");
  };

  test("plugins_unconfined_counts_enabled_code_plugins_and_goes_with_the_last", async () => {
    expect(await pluginWarnings(all(NOT_READY), 2)).toEqual([
      expect.objectContaining({ code: "PLUGINS_UNCONFINED", details: { profile: "plugin", os: "linux", reason: "profileNotReady", enabled: 2 }, fix: NOT_READY_FIX }),
    ]);
    expect(await pluginWarnings(all(DISABLED), 1)).toEqual([expect.objectContaining({ code: "PLUGINS_UNCONFINED", details: expect.objectContaining({ reason: "landlockDisabled", enabled: 1 }) })]);
    // No enabled code plugin: nothing runs unconfined, so nothing warns.
    expect(await pluginWarnings(all(NOT_READY), 0)).toEqual([]);
    // The profile enforced and applied by the spawner: no warning whatever the count.
    expect(await pluginWarnings(all(ENFORCED), 3, { ...caps.profiles, plugin: "enforced" })).toEqual([]);
    // Enforceable here, but the job host's spawner does not apply it: still unconfined.
    expect(await pluginWarnings(all(ENFORCED), 1)).toEqual([expect.objectContaining({ code: "PLUGINS_UNCONFINED", details: expect.objectContaining({ reason: "spawnerUnconfined", enabled: 1 }) })]);
  });
});
