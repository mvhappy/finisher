// The hook's `fin` credential (7F plan Task 5; Decision 11, Owner question 6; spec §9.2): a hook
// that declares `fin: true` gets `FIN_CREDENTIAL` and `FIN_SERVER` (and its project as
// `FIN_PROJECT_DIR`): an `agent` credential of its own with `plan`'s scopes, its event's project
// and `main`, minted on first use, one per (hook, project), revoked through 5A's one revoke path
// when the hook's configuration changes or disappears, at stop, and at start for any left over
// (label prefix `hook `). A hook without `fin` gets none. Calls made with a hook credential fire
// no hooks (the recursion guard). Linux, under the real launcher (`FIN_TEST_SANDBOX_READY=hook`):
// the hook reaches the agent listener's port and no other. The hooks run `fin` (the release
// build `agents.finPath` resolves to) through `fake-hook.mjs --fin`.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { DatabaseSync } from "node:sqlite";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { HookEvent, Principal } from "@finisher/contracts";
import type { AclGrant, HookInput } from "@finisher/core-types";
import { sandboxProjectContainer, sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { controlDbPath } from "../src/control/db.ts";
import { HookConfig } from "../src/hooks/config.ts";
import { type HookProcessDeps, HookSlots, isCommandHook, runCommandHook } from "../src/hooks/process.ts";
import { ulid } from "../src/control/ulid.ts";
import { LOCAL } from "../src/principal.ts";
import { probeLauncher } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import type { NdjsonClient } from "./support/ndjson-client.ts";
import { ok } from "./support/ndjson-client.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const WIN = process.platform === "win32";
const LINUX = process.platform === "linux";
const require = process.env.FIN_REQUIRE_LANDLOCK === "1" || process.env.FIN_REQUIRE_SANDBOX === "1";
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });
const saved = process.env.FIN_TEST_SANDBOX_READY;

beforeAll(() => {
  // The `hook` profile is not in `READY` until 7C Task 16: the test-hooks build adds it.
  process.env.FIN_TEST_SANDBOX_READY = "hook";
});
afterAll(() => {
  if (saved === undefined) delete process.env.FIN_TEST_SANDBOX_READY;
  else process.env.FIN_TEST_SANDBOX_READY = saved;
});

let tmp: string;
const servers: AgentServer[] = [];
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-cred-")));
});
afterEach(async () => {
  for (const s of servers.splice(0)) await s.stop();
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

/** An agent's call (not a hook's). */
const AGENT: Principal = { kind: "agent", id: "01JAGENTAGENTAGENTAGENTAGE", scopes: [], projects: "*", branches: "*", human: false };

/** The fake hook in the user layer: `run: ["node", "fake-hook.mjs", <args…>], file: "fake-hook.mjs"`. */
const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });
const fin = (...args: string[]) => ["--fin", JSON.stringify(args)];

interface Running {
  readonly server: AgentServer;
  readonly serverData: string;
  readonly local: NdjsonClient;
  readonly projectId: string;
  readonly dir: string;
}

const userHooks = (serverData: string, hooks: readonly unknown[]) => writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks }));

/** A server whose user layer holds `hooks`, with project `P` open. */
async function started(hooks: readonly unknown[], status: () => ReturnType<typeof sandboxStatus> = DIRECT, serverData = join(tmp, "server")): Promise<Running> {
  mkdirSync(serverData, { recursive: true });
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  userHooks(serverData, hooks);
  const server = new AgentServer(serverData, { hooks: { status } });
  servers.push(server);
  await server.start();
  const local = await server.local();
  const dir = join(tmp, "P");
  const projectId = existsSync(dir)
    ? ok<{ projectId: string }>(await local.call("project.open", { dir })).projectId
    : ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
  return { server, serverData, local, projectId, dir };
}

const fire = (r: Running, event: HookEvent = "UserPrompt", projectId = r.projectId, principal: Principal = AGENT) =>
  Effect.runPromise(r.server.agents.hookWiring.engine.fire(event, { projectId, principal }, {}));

/** What the fake hook observed (its context). */
async function seen(r: Running, event: HookEvent = "UserPrompt", projectId = r.projectId): Promise<Record<string, any>> {
  const out = await fire(r, event, projectId);
  if (out.context === undefined) throw new Error(`no observation: ${JSON.stringify(out)}`);
  return JSON.parse(out.context);
}

/** Every hook credential the server holds, live or ended, newest first. */
const hookCreds = (r: Running) => r.server.core.credentials.base.credentials.list(true).filter((c) => c.label.startsWith("hook "));

function auditCount(serverData: string, kind: string): number {
  const db = new DatabaseSync(controlDbPath(serverData));
  try {
    return (db.prepare("SELECT COUNT(*) AS n FROM audit WHERE kind = ?").get(kind) as { n: number }).n;
  } finally {
    db.close();
  }
}

const credFiles = (serverData: string) => (existsSync(join(serverData, "hooks", "creds")) ? readdirSync(join(serverData, "hooks", "creds")) : []);

describe("the credential's rights (Decision 11)", () => {
  test("fin_hook_reads_with_plan_scopes: fin view against the event's project", { timeout: 60_000 }, async () => {
    const r = await started([hook("reader", "UserPrompt", ["--report", "env", ...fin("view")], { fin: true })]);
    const s = await seen(r);
    expect(s.fin).toEqual({ code: 0 });
    expect(Object.keys(s.env).sort()).toEqual(["FIN_CREDENTIAL", "FIN_HOOK_EVENT", "FIN_PROJECT_DIR", "FIN_SERVER", "HOME", "LANG", "PATH", "TEMP", "TMP", "TMPDIR"]);
    const [cred] = hookCreds(r);
    expect(cred).toMatchObject({ kind: "agent", label: "hook user:reader", projects: [r.projectId], branches: ["main"], idleExpiryS: 60, status: "live" });
    expect(cred!.absoluteExpiryAt).toBeUndefined();
    expect([...cred!.scopes].sort()).toEqual(["agent.read", "diagnostics.read", "diagnostics.record", "jobs.read", "library.read", "project.read", "server.read"]);
    // Its file is private, in server data.
    expect(s.env.FIN_CREDENTIAL).toMatch(new RegExp(`^${realpathSync(r.serverData).replace(/[\\^$.*+?()[\]{}|]/g, "\\$&")}[\\\\/]hooks[\\\\/]creds[\\\\/][0-9a-f]+\\.json$`));
  });

  test("fin_hook_cannot_write: a write is SCOPE_DENIED", { timeout: 60_000 }, async () => {
    const r = await started([hook("writer", "UserPrompt", fin("track", "add", "--kind", "video"), { fin: true })]);
    const s = await seen(r);
    expect(s.fin).toMatchObject({ code: 7, error: "SCOPE_DENIED" });
  });

  test("fin_hook_reaches_only_event_project: another project is PROJECT_NOT_FOUND", { timeout: 60_000 }, async () => {
    const r = await started([hook("other", "UserPrompt", fin("view", "--project", join(tmp, "Q")), { fin: true })]);
    ok(await r.local.call("project.create", { dir: join(tmp, "Q"), name: "Q", rate: "30" }));
    expect((await seen(r)).fin).toMatchObject({ error: "PROJECT_NOT_FOUND" });
  });

  test("hook_without_fin_has_no_credential", { timeout: 60_000 }, async () => {
    const r = await started([hook("plain", "UserPrompt", ["--report", "env"])]);
    const s = await seen(r);
    expect(s.env.FIN_CREDENTIAL).toBeUndefined();
    expect(s.env.FIN_SERVER).toBeUndefined();
    expect(s.env.FIN_PROJECT_DIR).toBeUndefined();
    expect(hookCreds(r)).toEqual([]);
    expect(credFiles(r.serverData)).toEqual([]);
  });
});

describe("the credential's lifetime (Owner question 6)", () => {
  test("one_credential_per_hook_and_project: two runs, one mint; another project its own", { timeout: 60_000 }, async () => {
    const r = await started([hook("reader", "UserPrompt", fin("view"), { fin: true })]);
    const before = auditCount(r.serverData, "credential.mint");
    expect((await seen(r)).fin).toEqual({ code: 0 });
    expect((await seen(r)).fin).toEqual({ code: 0 });
    expect(auditCount(r.serverData, "credential.mint") - before).toBe(1);
    expect(hookCreds(r)).toHaveLength(1);
    const q = ok<{ projectId: string }>(await r.local.call("project.create", { dir: join(tmp, "Q"), name: "Q", rate: "30" })).projectId;
    await fire(r, "UserPrompt", q);
    expect(auditCount(r.serverData, "credential.mint") - before).toBe(2);
    expect(hookCreds(r).map((c) => c.projects)).toEqual([[q], [r.projectId]]);
    expect(credFiles(r.serverData)).toHaveLength(2);
  });

  test("credential_revoked_when_hook_changes: and when it disappears", { timeout: 60_000 }, async () => {
    const r = await started([hook("reader", "UserPrompt", fin("view"), { fin: true })]);
    await fire(r);
    const [first] = hookCreds(r);
    // Changed: the next dispatch ends the old one; the run mints a new one.
    userHooks(r.serverData, [hook("reader", "UserPrompt", fin("view"), { fin: true, timeoutS: 20 })]);
    expect((await seen(r)).fin).toEqual({ code: 0 });
    const [second, old] = hookCreds(r);
    expect(old!.id).toBe(first!.id);
    expect(old!.status).toBe("revoked");
    expect(second!.status).toBe("live");
    expect(auditCount(r.serverData, "credential.revoke")).toBeGreaterThanOrEqual(1);
    // Gone (no hook for any event any more): the next dispatch ends it, with its file.
    userHooks(r.serverData, []);
    await fire(r, "PreCommand");
    expect(hookCreds(r).every((c) => c.status === "revoked")).toBe(true);
    expect(credFiles(r.serverData)).toEqual([]);
  });

  test("credential_revoked_at_stop", { timeout: 60_000 }, async () => {
    const r = await started([hook("reader", "UserPrompt", fin("view"), { fin: true })]);
    await fire(r);
    const credentials = r.server.core.credentials.base.credentials;
    const [cred] = hookCreds(r);
    await r.server.stop();
    expect(credentials.get(cred!.id)?.status).toBe("revoked");
    expect(credFiles(r.serverData)).toEqual([]);
    const db = new DatabaseSync(controlDbPath(r.serverData));
    try {
      expect((db.prepare("SELECT revokedAt FROM credentials WHERE id = ?").get(cred!.id) as { revokedAt: number | null }).revokedAt).not.toBeNull();
    } finally {
      db.close();
    }
  });

  test("leftover_hook_credentials_revoked_at_start: by the label prefix, with their files", { timeout: 60_000 }, async () => {
    const serverData = join(tmp, "server");
    const r = await started([], DIRECT, serverData);
    // What a crash leaves: a hook credential the server did not end, and its file.
    const credentials = r.server.core.credentials.base.credentials;
    const left = credentials.mintWithId(LOCAL, ulid(), { kind: "agent", label: "hook user:left", scopes: ["project.read"], projects: [r.projectId], branches: ["main"] }).credential;
    const other = credentials.mintWithId(LOCAL, ulid(), { kind: "agent", label: "hooked on fin", scopes: ["project.read"], projects: [r.projectId], branches: ["main"] }).credential;
    mkdirSync(join(serverData, "hooks", "creds"), { recursive: true });
    writeFileSync(join(serverData, "hooks", "creds", "0123abcd.json"), "{}");
    await r.server.stop();
    const again = await started([], DIRECT, serverData);
    const after = again.server.core.credentials.base.credentials;
    expect(after.get(left.id)?.status).toBe("revoked");
    expect(after.get(other.id)?.status).toBe("live");
    expect(credFiles(serverData)).toEqual([]);
  });
});

describe("the recursion guard (Review Focus 4)", () => {
  test("hook_fin_calls_fire_no_hooks: an event whose principal is a live hook credential runs no hook", { timeout: 60_000 }, async () => {
    const r = await started([hook("guard", "PreCommand", fin("view"), { fin: true, principals: "all" })]);
    // An agent's command runs it (and mints its credential).
    expect(JSON.parse((await fire(r, "PreCommand")).context!).fin).toEqual({ code: 0 });
    const [cred] = hookCreds(r);
    const asHook: Principal = { kind: "agent", id: cred!.id, scopes: [], projects: [r.projectId], branches: ["main"], human: false };
    // The hook's own `fin` call (as the chain will fire it, Task 7): nothing runs.
    expect(await fire(r, "PreCommand", r.projectId, asHook)).toEqual({ ran: 0 });
    expect(await fire(r, "UserPrompt", r.projectId, asHook)).toEqual({ ran: 0 });
    // The engine's counters saw one run.
    expect(r.server.agents.hookWiring.engine.stats().events.PreCommand.runs).toBe(1);
  });
});

describe("under the real launcher (Linux)", () => {
  test.skipIf(!LINUX)("fin_hook_reaches_only_listener_port: the listener answers, another loopback port is refused", { timeout: 60_000 }, async () => {
    const m = sandboxStatus().hook;
    if (m.kind !== "enforced") {
      expect(require, `FIN_REQUIRE_LANDLOCK=1 but the hook profile is ${JSON.stringify(m)}`).toBe(false);
      return;
    }
    const other = createServer((s) => s.end());
    await new Promise<void>((res) => other.listen(0, "127.0.0.1", res));
    try {
      const port = String((other.address() as { port: number }).port);
      const r = await started([hook("net", "UserPrompt", ["--report", "env", ...fin("view"), "--connects", port, "--report", "net"], { fin: true })], sandboxStatus);
      const s = await seen(r);
      expect(s.env.FIN_SERVER).toMatch(/^tcp:127\.0\.0\.1:\d+$/);
      // `fin view` reached the agent listener with the hook's credential.
      expect(s.fin).toEqual({ code: 0 });
      expect(s.connects[port]).not.toBe("ok");
      // No address at all where the launcher has its namespaces; elsewhere (no user or network
      // namespace: an AppArmor host) Landlock's TCP rules alone refuse other ports, and the
      // host warns `SANDBOX_SOCKETS_EXPOSED` (Task 13 runs it on a host with namespaces).
      if (probeLauncher().kind === "available") expect(s.net).toEqual([]);
    } finally {
      other.close();
    }
  });
});

describe("Windows (Decision 12)", () => {
  test.skipIf(WIN)("windows_fin_hook_container_allowed_on_the_pipe: only for a fin hook, before its grants", async () => {
    const serverData = join(tmp, "sd");
    const dir = join(tmp, "P");
    mkdirSync(serverData, { recursive: true });
    mkdirSync(dir, { recursive: true });
    copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
    const config = new HookConfig({ serverData, trustOf: () => undefined, dirOf: () => dir });
    // A launcher that runs the program after reading the request (the AppContainer is the real
    // launcher's on Windows, Task 13; the pipe's admission and the input are what this proves).
    const launcher = join(tmp, "pass-launcher.sh");
    writeFileSync(launcher, `#!/bin/sh\ncat <&3 >/dev/null\nshift 3\nexec "$@"\n`, { mode: 0o755 });
    const credential = join(serverData, "cred.json");
    writeFileSync(credential, "{}");
    const log: string[] = [];
    const inputs: HookInput[] = [];
    const deps: HookProcessDeps = {
      serverData,
      slots: new HookSlots(8),
      platform: "win32",
      status: () => ({ ...sandboxStatus(), os: "windows", hook: { kind: "enforced", detail: "stub" } }),
      launcher: () => launcher,
      acl: { active: true, grant: async (g) => void log.push(`grant ${g.owner}`), revoke: async (owner) => (log.push(`revoke ${owner}`), 0), sweep: async () => 0 },
      containerSid: (name) => `sid-of-${name}`,
      allowPipe: (sid) => void log.push(`allow ${sid}`),
      hookGrants: (input) => (inputs.push(input), [{ path: input.tmp, trustee: { kind: "container", value: "c" }, mask: 0x1301bf, scope: "owner" }] as AclGrant[]),
      credential: () => credential,
      finServer: async (launched) => ({ env: "\\\\.\\pipe\\finisher-test", ...(launched ? { fin: { kind: "pipe" as const } } : {}) }),
      finPath: process.execPath,
    };
    const runIt = async (spec: Record<string, unknown>) => {
      writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: [hook("h", "UserPrompt", ["--reply", "allow"], spec)] }));
      const h = config.hooks("p1").find((x) => x.id === "h")!;
      if (!isCommandHook(h)) throw new Error("not a command hook");
      return Effect.runPromise(runCommandHook(h, { event: "UserPrompt", v: 1, project: { id: "p1", dir }, payload: {} }, deps));
    };
    expect(await runIt({ fin: true })).toEqual({ reply: "allow" });
    expect(log[0]).toBe(`allow sid-of-${sandboxProjectContainer(dir)}`);
    expect(log[1]).toMatch(/^grant call:/);
    expect(inputs[0]!.fin).toEqual({ credential, endpoint: { kind: "pipe" } });
    expect(inputs[0]!.toolchains).toContain(process.execPath);
    log.length = 0;
    expect(await runIt({})).toEqual({ reply: "allow" });
    expect(log.some((l) => l.startsWith("allow"))).toBe(false);
    expect(inputs[1]!.fin).toBeUndefined();
  });
});
