// Claude Code's account-change detection and the sign-in store spy (plan 7D Task 11, Decision 19,
// spec §8.6): the account the CLI reports is kept as a keyed hash only; a different account,
// unconfirmed, stops new sessions with `PROVIDER_ACCOUNT_CHANGED` (exit 5) until a person at the
// console confirms it (y/N, audited); `provider signin --confirm` lets the next change through;
// and finisherd makes no file-system call under `<server-data>/providers/`. Everything runs
// `fake-claude` (its `account` sidecar knob is the label), never the real CLI.
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { type Principal, type ProviderInfo, ruleOf } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";
import { type AuthStatus, accountCheck, accountKey, ACCOUNT_FILE, ClaudeAccounts, instanceAccount, labelOf, labelOfOutput } from "../src/agent/claude/account.ts";
import { CLAUDE_CAPABILITIES, claudeProviderInfo, type ClaudeDetectDeps, invalidateClaudeDetection } from "../src/agent/claude/detect.ts";
import { probe } from "../src/agent/claude/probe.ts";
import { BUNDLED_CATALOGUE, Catalogue } from "../src/agent/catalogue/catalogue.ts";
import { sessionPlanner } from "../src/agent/planner.ts";
import { BUNDLED_ROLES } from "../src/agent/roles.ts";
import type { SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { claudeWiring } from "../src/agent/wiring.ts";
import { exitCodeOf } from "../src/console/exit-codes.ts";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";
import { AgentServer } from "./support/agent-server.ts";
import { chainHarness, codeOf, errorOf } from "./support/chain-harness.ts";
import { ok } from "./support/ndjson-client.ts";
import { person } from "./support/person.ts";
import { PEOPLE } from "./support/runtime-doubles.ts";

// Every path finisherd's own code hands `node:fs` (first two arguments) and every `cwd` it
// spawns in, for the store spy (`finisherd_never_opens_sign_in_store`).
const spy = vi.hoisted(() => {
  const touched: string[] = [];
  const record = (args: unknown[]) => {
    for (const a of args.slice(0, 2)) if (typeof a === "string" || a instanceof URL) touched.push(String(a));
  };
  const wrap = <T extends object>(m: T): T => {
    const out: Record<string, unknown> = { ...(m as Record<string, unknown>) };
    for (const [k, v] of Object.entries(m)) {
      if (k === "promises" && v !== null && typeof v === "object") out[k] = wrap(v as object);
      if (typeof v !== "function" || /^[A-Z]/.test(k)) continue;
      const f = v as (...a: unknown[]) => unknown;
      out[k] = Object.assign((...args: unknown[]) => (record(args), f(...args)), f);
    }
    return out as T;
  };
  const wrapSpawns = <T extends object>(m: T): T => {
    const out: Record<string, unknown> = { ...(m as Record<string, unknown>) };
    for (const k of ["spawn", "spawnSync", "execFile", "execFileSync", "exec", "execSync", "fork"]) {
      const f = (m as Record<string, unknown>)[k];
      if (typeof f !== "function") continue;
      out[k] = Object.assign((...args: unknown[]) => {
        for (const a of args) {
          const cwd = a !== null && typeof a === "object" && !Array.isArray(a) ? (a as { cwd?: unknown }).cwd : undefined;
          if (typeof cwd === "string" || cwd instanceof URL) touched.push(String(cwd));
        }
        return (f as (...a: unknown[]) => unknown)(...args);
      }, f);
    }
    return out as T;
  };
  return { touched, wrap, wrapSpawns };
});
vi.mock("node:fs", async (orig) => {
  const m = await orig<typeof import("node:fs")>();
  return { ...spy.wrap(m), default: spy.wrap(m) };
});
vi.mock("node:fs/promises", async (orig) => {
  const m = await orig<typeof import("node:fs/promises")>();
  return { ...spy.wrap(m), default: spy.wrap(m) };
});
vi.mock("node:child_process", async (orig) => {
  const m = await orig<typeof import("node:child_process")>();
  return { ...spy.wrapSpawns(m), default: spy.wrapSpawns(m) };
});

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
/** CI (`FIN_REQUIRE_SERVER=1`): a missing fake fails instead of skipping. */
const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(!existsSync(FAKE) && !REQUIRE_FAKE);

const LABEL_A = "person-a.label-7d11@example.invalid";
const LABEL_B = "person-b.label-7d11@example.invalid";
const LABEL_C = "person-c.label-7d11@example.invalid";

const hooks = process.env.FIN_TEST_HOOKS;
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-claude-account-")));
});
afterEach(() => {
  invalidateClaudeDetection();
  rmSync(tmp, { recursive: true, force: true });
});

/** `fake-claude` installed as `<dir>/claude`, signed in to `account` (its sidecar knob). */
function installFake(name: string, o: { account?: string; script?: unknown } = {}): { dir: string; bin: string } {
  const dir = join(tmp, name);
  mkdirSync(dir, { recursive: true });
  const bin = join(dir, WIN ? "claude.exe" : "claude");
  copyFileSync(FAKE, bin);
  chmodSync(bin, 0o755);
  writeFileSync(join(dir, "script.json"), JSON.stringify(o.script ?? {}));
  setAccount(dir, o.account);
  return { dir, bin };
}

/** The person signs the CLI in to another account (the sidecar's label); detection asks again. */
function setAccount(dir: string, account: string | undefined): void {
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", ...(account === undefined ? {} : { account }) }));
  invalidateClaudeDetection();
}

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8" };

function sandboxDeps(data: string): SessionSandboxDeps {
  mkdirSync(data, { recursive: true, mode: 0o700 });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data, projectDirOf: () => undefined, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts") };
}

const detectDeps = (data: string, bin: string): ClaudeDetectDeps => ({ sandbox: sandboxDeps(data), settings: () => ({ enabled: true, path: bin }), pathEnv: "" });
const line = (d: ClaudeDetectDeps): Promise<ProviderInfo> => Effect.runPromise(claudeProviderInfo(d));

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

/** Every file under `root` (not `skip`), recursively. */
function filesUnder(root: string, skip: string): string[] {
  const out: string[] = [];
  const walk = (d: string) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (p === skip) continue;
      if (e.isDirectory()) walk(p);
      else if (e.isFile()) out.push(p);
    }
  };
  walk(root);
  return out;
}

const SIGNED_IN = (label?: string): AuthStatus => ({ signedIn: true, authMethod: "claude.ai", ...(label === undefined ? {} : { label }) });

// ---- the hash (unit) ----

describe("the account hash", () => {
  test("account_hash_keyed_and_stored_only_as_hash: keyed, short, never the label", () => {
    const k1 = Buffer.alloc(32, 1);
    const k2 = Buffer.alloc(32, 2);
    const a = accountCheck(SIGNED_IN(LABEL_A), k1, null);
    expect(a.hash).toMatch(/^[0-9a-f]{8}$/);
    expect(a.changed).toBe(false);
    // The same account under the same key is the same hash; another key gives another.
    expect(accountCheck(SIGNED_IN(LABEL_A), k1, a.hash)).toEqual({ hash: a.hash, changed: false });
    expect(accountCheck(SIGNED_IN(LABEL_A), k2, null).hash).not.toBe(a.hash);
    // Another account is a change against the last recorded one.
    const b = accountCheck(SIGNED_IN(LABEL_B), k1, a.hash);
    expect(b.changed).toBe(true);
    expect(b.hash).not.toBe(a.hash);
    // No label (an API key that names none): the method alone, never equal to a labelled account.
    expect(accountCheck(SIGNED_IN(), k1, null).hash).not.toBe(a.hash);
    // Signed out: no account to hash.
    expect(() => accountCheck({ signedIn: false }, k1, null)).toThrow(expect.objectContaining({ code: "PROVIDER_AUTH" }));
    expect(labelOf({ loggedIn: true, email: LABEL_A })).toBe(LABEL_A);
    expect(labelOf({ loggedIn: true })).toBeUndefined();
    expect(labelOfOutput("not json")).toBeUndefined();
  });

  test("account_hash_keyed_and_stored_only_as_hash: the key is made once, 0600, beside the approval key", () => {
    const data = join(tmp, "data");
    mkdirSync(data, { recursive: true, mode: 0o700 });
    const k = accountKey(data);
    expect(k.length).toBe(32);
    expect(accountKey(data).equals(k)).toBe(true);
    const path = join(data, "auth", "account.key");
    if (!WIN) expect(statSync(path).mode & 0o777).toBe(0o600);
    // The state file holds hashes and one-word methods only.
    const accounts = new ClaudeAccounts(data);
    accounts.observe(SIGNED_IN(LABEL_A), k);
    accounts.observe(SIGNED_IN(LABEL_B), k);
    const raw = readFileSync(join(data, "agents", ACCOUNT_FILE), "utf8");
    expect(raw).not.toContain("label-7d11");
    expect(Object.keys(JSON.parse(raw)).sort()).toEqual(["current", "pending", "v"]);
    if (!WIN) expect(statSync(join(data, "agents", ACCOUNT_FILE)).mode & 0o777).toBe(0o600);
  });

  withFake("account_hash_keyed_and_stored_only_as_hash: agent.providers reports the hash; no file of the server's, nor its log, holds the label", async () => {
    const { bin } = installFake("cli", { account: LABEL_A });
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: bin } } }));
    const logged: string[] = [];
    const write = process.stderr.write.bind(process.stderr);
    const capture = vi.spyOn(process.stderr, "write").mockImplementation(((chunk: unknown, ...rest: unknown[]) => {
      logged.push(String(chunk));
      return (write as (...a: unknown[]) => boolean)(chunk, ...rest);
    }) as typeof process.stderr.write);
    const server = new AgentServer(serverData);
    await server.start();
    try {
      const local = await server.local();
      const answer = ok<{ providers: ProviderInfo[] }>(await local.call("agent.providers", {}));
      const claude = answer.providers.find((p) => p.harness === "claudeCode")!;
      expect(claude).toMatchObject({ detected: true, signedIn: true, authMethod: "claude.ai" });
      expect(claude.accountHash).toMatch(/^[0-9a-f]{8}$/);
      expect(claude.account).toBeUndefined();
      expect(JSON.stringify(answer)).not.toContain("label-7d11");
      // The hash is the keyed one of the fake's label.
      expect(claude.accountHash).toBe(accountCheck(SIGNED_IN(LABEL_A), accountKey(serverData), null).hash);
      // Another account: the line says so, the label still nowhere.
      setAccount(join(tmp, "cli"), LABEL_B);
      const changed = ok<{ providers: ProviderInfo[] }>(await local.call("agent.providers", {})).providers.find((p) => p.harness === "claudeCode")!;
      expect(changed.problem?.code).toBe("PROVIDER_ACCOUNT_CHANGED");
      expect(changed.accountHash).not.toBe(claude.accountHash);
      expect(JSON.stringify(changed)).not.toContain("label-7d11");
    } finally {
      await server.stop();
      capture.mockRestore();
    }
    // control.db (and its WAL), the journal, the audit, the account state, every other file.
    const files = filesUnder(serverData, join(serverData, "providers"));
    expect(files.some((f) => f.endsWith("control.db"))).toBe(true);
    expect(files.some((f) => f.endsWith(ACCOUNT_FILE))).toBe(true);
    for (const f of files) expect(readFileSync(f).includes(Buffer.from("label-7d11")), f).toBe(false);
    expect(logged.join("")).not.toContain("label-7d11");
  });
});

// ---- a change stops new sessions ----

const startReq = { projectId: "01JP0000000000000000000001", harness: "claudeCode" as const, model: "default" };

describe("an account change", () => {
  withFake("account_change_stops_new_sessions: PROVIDER_ACCOUNT_CHANGED, exit 5, until a person confirms", async () => {
    const { dir, bin } = installFake("cli", { account: LABEL_A });
    const data = join(tmp, "data");
    const d = detectDeps(data, bin);
    const w = claudeWiring(() => line(d), data);
    const planner = sessionPlanner({
      roles: BUNDLED_ROLES,
      catalogue: Catalogue.load(BUNDLED_CATALOGUE),
      harnesses: () => new Set(["builtin", "claudeCode"] as never[]),
      hasKey: () => false,
      local: () => undefined,
      localModels: async () => [],
      claude: w.planning,
    });
    // The first account seen is the account: sessions plan.
    expect(await planner.plan(startReq, PEOPLE)).toMatchObject({ harness: "claudeCode", provider: "anthropic" });
    const first = (await line(d)).accountHash!;
    // The same account again (a fresh `auth status`): still fine.
    invalidateClaudeDetection();
    expect(await planner.plan(startReq, PEOPLE)).toMatchObject({ harness: "claudeCode" });
    // Signed in to another account: new sessions are refused.
    setAccount(dir, LABEL_B);
    const refused = await planner.plan(startReq, PEOPLE).then(
      () => undefined,
      (e: unknown) => e as { code: string; details: Record<string, unknown>; fix?: string },
    );
    expect(refused).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { harness: "claudeCode" } });
    expect(refused!.fix).toContain("finisherd provider account confirm claudeCode");
    expect(exitCodeOf("PROVIDER_ACCOUNT_CHANGED")).toBe(5);
    const changed = await line(d);
    expect(changed.problem?.code).toBe("PROVIDER_ACCOUNT_CHANGED");
    expect(changed.accountHash).not.toBe(first);
    // Other harnesses are not affected.
    expect(await sessionPlanner({ roles: BUNDLED_ROLES, catalogue: Catalogue.load(BUNDLED_CATALOGUE), harnesses: () => new Set(["fake"] as never[]), hasKey: () => false, local: () => undefined, localModels: async () => [], claude: w.planning }).plan({ ...startReq, harness: "fake" as never }, PEOPLE)).toMatchObject({ harness: "fake" });
    // Back on the recorded account: nothing waits any more.
    setAccount(dir, LABEL_A);
    expect(await planner.plan(startReq, PEOPLE)).toMatchObject({ harness: "claudeCode" });
    expect(new ClaudeAccounts(data).pending()).toBeUndefined();
    // Away again, then a person confirms: sessions plan on the new account.
    setAccount(dir, LABEL_B);
    await expect(planner.plan(startReq, PEOPLE)).rejects.toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED" });
    expect(new ClaudeAccounts(data).confirm()).toEqual({ confirmed: true, accountHash: changed.accountHash, previous: first });
    expect(await planner.plan(startReq, PEOPLE)).toMatchObject({ harness: "claudeCode" });
    expect((await line(d)).problem).toBeUndefined();
    // The model cache is keyed by the same hash (Decision 11).
    w.cache.recordInstanceModels(changed.version!, changed.accountHash!, [{ id: "opus", displayName: "Opus", isDefault: false }]);
    expect(await planner.plan({ ...startReq, model: "opus" }, PEOPLE)).toMatchObject({ model: "opus" });
  });

  test("account_change_stops_new_sessions: the instance seam refuses a new session only; a running one continues", () => {
    const data = join(tmp, "data");
    mkdirSync(data, { recursive: true, mode: 0o700 });
    const a = instanceAccount(data, SIGNED_IN(LABEL_A), { newSession: true });
    expect(a.refused).toBeNull();
    expect(a.hash).toMatch(/^[0-9a-f]{8}$/);
    // Another account at a running session's instance (a respawn, a resume): it keeps running,
    // and the change is recorded, so the next new session is refused.
    const running = instanceAccount(data, SIGNED_IN(LABEL_B), { newSession: false });
    expect(running.refused).toBeNull();
    expect(running.hash).not.toBe(a.hash);
    expect(new ClaudeAccounts(data).pending()).toBe(running.hash);
    expect(instanceAccount(data, SIGNED_IN(LABEL_B), { newSession: true }).refused).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { harness: "claudeCode", accountHash: running.hash } });
    // Signed out: no hash, no refusal (the CLI's own error is PROVIDER_AUTH).
    expect(instanceAccount(data, { signedIn: false }, { newSession: true })).toEqual({ refused: null });
  });

  // TODO(7D Task 7): once the adapter is registered, `agent.session.start {harness: claudeCode}`
  // over RPC answers PROVIDER_ACCOUNT_CHANGED (exit 5 from `finisherd agent start`) while a
  // session started before the change keeps taking prompts; the adapter calls `instanceAccount`
  // after each instance's `auth status`.
  test.todo("account_change_stops_new_sessions over agent.session.start, with a running Claude Code session continuing (Task 7)");
});

// ---- confirming ----

describe("confirming a change", () => {
  let server: AgentServer;
  let cli: { dir: string; bin: string };

  async function boot(): Promise<void> {
    cli = installFake("cli", { account: LABEL_A });
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: cli.bin } } }));
    server = new AgentServer(serverData);
    await server.start();
  }
  afterEach(async () => {
    await server?.stop();
  });

  async function run(argv: string[], prompt: Prompt = NOT_A_TERMINAL): Promise<{ code: number; out: string; err: string }> {
    let out = "";
    let err = "";
    const code = await runConsole([...argv, "--server-data", server.serverData], {
      prompt,
      output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
    });
    return { code, out, err };
  }

  const claudeLine = async () => {
    const local = await server.local();
    return ok<{ providers: ProviderInfo[] }>(await local.call("agent.providers", {})).providers.find((p) => p.harness === "claudeCode")!;
  };

  withFake("confirm_is_human_only_and_audited: y/N at a terminal, audited, nothing to confirm says so", async () => {
    await boot();
    const first = (await claudeLine()).accountHash!;
    // Nothing waits: said so, nothing asked.
    const idle = person();
    const none = await run(["provider", "account", "confirm", "claudeCode"], idle);
    expect(none.code, none.err).toBe(0);
    expect(none.out).toContain("No Claude Code account change waits");
    expect(idle.asked).toEqual([]);
    setAccount(cli.dir, LABEL_B);
    const changed = await claudeLine();
    expect(changed.problem?.code).toBe("PROVIDER_ACCOUNT_CHANGED");
    // Not at a terminal: refused before anything connects.
    expect((await run(["provider", "account", "confirm", "claudeCode"])).code).toBe(7);
    expect((await run(["provider", "account", "confirm", "codex"], person())).code).toBe(2);
    // A person answers no: nothing changes.
    const no = person("n");
    const declined = await run(["provider", "account", "confirm", "claudeCode"], no);
    expect(declined.code).not.toBe(0);
    expect(no.shown.join("\n")).toContain(changed.accountHash!);
    expect((await claudeLine()).problem?.code).toBe("PROVIDER_ACCOUNT_CHANGED");
    // Yes: the new account is the account, audited.
    const yes = await run(["provider", "account", "confirm", "claudeCode"], person());
    expect(yes.code, yes.err).toBe(0);
    expect(yes.out).toContain(`now run on account ${changed.accountHash} (was ${first})`);
    expect((await claudeLine()).problem).toBeUndefined();
    const audit = ok<{ entries: Array<{ kind: string; principalKind: string; outcome: string; details: Record<string, unknown> }> }>(await (await server.console()).call("auth.audit.list", { kind: "settings.write" }));
    const rows = audit.entries.filter((e) => e.details.rpc === "agent.account.confirm");
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ principalKind: "console", outcome: "ok", details: { value: "claudeCode" } });
    expect(JSON.stringify(audit)).not.toContain("label-7d11");
  });

  withFake("confirm_is_human_only_and_audited: `fin`'s own principal is refused over RPC", async () => {
    await boot();
    const local = await server.local();
    expect(codeOf2(await local.call("agent.account.confirm", { harness: "claudeCode" }))).toBe("SCOPE_DENIED");
  });

  test("confirm_is_human_only_and_audited: an agent or a device is refused whatever it holds", async () => {
    const wide = ["agent.admin", "agent.run", "agent.read", "server.read", "project.read", "project.write"];
    const principals: Record<number, Principal> = {
      1: { kind: "console", id: "console", scopes: wide, projects: "*", branches: "*", human: true } as Principal,
      2: { kind: "device", id: "01device00000000000000000a", scopes: wide, projects: "*", branches: "*", human: true } as Principal,
      3: { kind: "agent", id: "01agent000000000000000000a", scopes: wide, projects: "*", branches: "*", human: false } as Principal,
    };
    const reached: unknown[] = [];
    const h = chainHarness(principals, { projects: { isOpen: () => true, dirOf: () => "/p", resolveBranch: () => undefined } as never }, () => ({
      "agent.account.confirm": (p: unknown) => Effect.sync(() => (reached.push(p), { confirmed: true })),
    }));
    expect(ruleOf("agent.account.confirm")).toMatchObject({ scopes: ["agent.admin"], kinds: ["console"] });
    for (const client of [2, 3]) {
      const e = errorOf(await h.call(client, "agent.account.confirm", { harness: "claudeCode", challenge: h.challenge(client) }));
      expect(e, String(client)).toMatchObject({ code: "SCOPE_DENIED", details: { reason: "principalKind" } });
    }
    expect(h.audited.filter((a) => a.kind === "auth.denied")).toHaveLength(2);
    // The console without the typed code is refused; with it, the call goes through.
    expect(codeOf(await h.call(1, "agent.account.confirm", { harness: "claudeCode" }))).toBe("CONFIRMATION_REQUIRED");
    expect(Exit.isSuccess(await h.call(1, "agent.account.confirm", { harness: "claudeCode", challenge: h.challenge(1) }))).toBe(true);
    expect(reached).toHaveLength(1);
  });

  withFake("signin_then_change_needs_no_confirm: `provider signin --confirm` lets the next change through, once", async () => {
    await boot();
    const first = (await claudeLine()).accountHash!;
    // Not at a terminal: refused before anything is printed or sent.
    const notty = await run(["provider", "signin", "claudeCode", "--confirm"]);
    expect(notty.code).toBe(7);
    expect(notty.out).toBe("");
    const p = person();
    const signin = await run(["provider", "signin", "claudeCode", "--confirm"], p);
    expect(signin.code, signin.err).toBe(0);
    expect(signin.out).toContain("auth login");
    expect(signin.out).toContain("The next Claude Code account is taken as expected.");
    expect(p.asked).toEqual(["Proceed? [y/N] "]);
    // The person signs in to another account: no confirmation needed.
    setAccount(cli.dir, LABEL_B);
    const next = await claudeLine();
    expect(next.problem).toBeUndefined();
    expect(next.accountHash).not.toBe(first);
    // The expectation is spent: a later change waits for a person again.
    setAccount(cli.dir, LABEL_C);
    expect((await claudeLine()).problem?.code).toBe("PROVIDER_ACCOUNT_CHANGED");
    const audit = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await (await server.console()).call("auth.audit.list", { kind: "settings.write" }));
    expect(audit.entries.filter((e) => e.details.rpc === "agent.account.confirm").map((e) => e.details.value)).toEqual(["claudeCode.next"]);
  });
});

const codeOf2 = (e: { ok: boolean; error?: { code: string } }) => (e.ok ? undefined : e.error?.code);

// ---- the store spy ----

describe("the sign-in store", () => {
  /** Paths under `<data>/providers/` among what finisherd touched (both spellings of server data). */
  const underProviders = (data: string, seen: readonly string[]) => {
    const roots = [...new Set([data, realpathSync(data)])].map((d) => join(d, "providers"));
    return seen.filter((p) => roots.some((r) => p === r || p.startsWith(r + sep) || p.startsWith(`file://${r}`)));
  };

  withFake("finisherd_never_opens_sign_in_store: detection, a probe, agent.providers, sign-in and confirm commands", async () => {
    const { dir, bin } = installFake("cli", { account: LABEL_A });
    const unsigned = installFake("cli-unsigned", {
      script: { auth: { loggedIn: false }, initialize: { account: { tokenSource: "none", apiProvider: "firstParty" } }, steps: [{ expect: "user", notLoggedIn: true }] },
    });
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: bin } } }));
    // A person signed the CLI in: the home and its store exist, with a credential in it.
    const store = join(serverData, "providers", "claudeCode", "home", "store");
    mkdirSync(store, { recursive: true, mode: 0o700 });
    writeFileSync(join(store, "credentials"), "not for finisherd");
    writeFileSync(join(serverData, "providers", "claudeCode", "home", ".claude.json"), JSON.stringify({ email: LABEL_A }));
    spy.touched.length = 0;

    // Detection (in this process, as the server's would) and the capability probe.
    const d = detectDeps(serverData, bin);
    expect((await line(d)).accountHash).toMatch(/^[0-9a-f]{8}$/);
    const probed = await Effect.runPromiseExit(
      probe({ installed: true, version: "2.1.321", supported: null, signedIn: false, capabilities: CLAUDE_CAPABILITIES }, "spy", { sandbox: d.sandbox, binary: unsigned.bin, timeoutMs: 10_000 }),
    );
    expect(Exit.isSuccess(probed), JSON.stringify(probed)).toBe(true);

    // The server: agent.providers, the console's sign-in and confirm commands.
    const server = new AgentServer(serverData);
    await server.start();
    try {
      let out = "";
      const run = (argv: string[], prompt: Prompt = NOT_A_TERMINAL) =>
        runConsole([...argv, "--server-data", serverData], { prompt, output: { out: (t) => void (out += `${t}\n`), err: () => undefined } });
      const local = await server.local();
      ok(await local.call("agent.providers", {}));
      expect(await run(["provider", "signin", "claudeCode"])).toBe(0);
      expect(await run(["provider", "signout", "claudeCode"])).toBe(0);
      expect(await run(["provider", "signin", "claudeCode", "--confirm"], person())).toBe(0);
      setAccount(dir, LABEL_B);
      setAccount(dir, LABEL_C);
      ok(await local.call("agent.providers", {}));
      expect(await run(["provider", "account", "confirm", "claudeCode"], person())).toBe(0);
      expect(await run(["agent", "providers", "--json"])).toBe(0);
      // Only the printed commands and the child's environment name the store.
      expect(out).toContain(store);
    } finally {
      await server.stop();
    }
    const seen = [...spy.touched];
    expect(seen.length, "the spy records finisherd's calls").toBeGreaterThan(100);
    expect(seen.some((p) => p.startsWith(join(serverData, "agents")))).toBe(true);
    expect(underProviders(serverData, seen)).toEqual([]);
    // The store is as the person's sign-in left it.
    expect(readFileSync(join(store, "credentials"), "utf8")).toBe("not for finisherd");
  });

  // TODO(7D Tasks 7–10): extend the spy over a full scripted Claude Code session — a permission
  // prompt answered through `permissions.ts`, an interrupt and `--resume` (Task 10), and the end —
  // with the same `underProviders(...) === []` check. It needs the adapter and its registration
  // (`claude/session.ts`, `adapter.ts`, `wiring.ts`), which are not on this branch.
  test.todo("finisherd_never_opens_sign_in_store: a scripted session with a permission prompt, resume and end (Tasks 7–10)");
});
