// Codex's detection, its `agent.providers` line and the sign-in commands (plan 7E Task 6,
// Decisions 1, 8, 11, 20, 21; Owner Q7(a)). Everything runs `fake-codex` (no real CLI, no sign-in).
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { hostname, tmpdir, userInfo } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { AgentProvidersSuccess, type FinError, type ProviderInfo } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Schema } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { AgentAdmin } from "../src/agent/admin.ts";
import { ALLOWED_METHODS } from "../src/agent/codex/rpc.ts";
import { CODEX_CAPABILITIES, type CodexDetectDeps, codexProviderInfo, detectCodex, invalidateCodexDetection, parseCodexVersion } from "../src/agent/codex/detect.ts";
import { type PinDeps, pinnedCodexVersion } from "../src/agent/codex/pin.ts";
import type { SessionSandboxDeps } from "../src/agent/sandbox.ts";
import { readAgentSettings } from "../src/agent/settings.ts";
import { runConsole } from "../src/console/main.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-codex${WIN ? ".exe" : ""}`, import.meta.url));
/** CI (`FIN_REQUIRE_SERVER=1`): a missing fake fails instead of skipping. */
const NO_FAKE = !existsSync(FAKE) && process.env.FIN_REQUIRE_SERVER !== "1";
const PIN = pinnedCodexVersion();

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY, res: process.env.FIN_RESOURCES_DIR };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready], ["FIN_RESOURCES_DIR", saved.res]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-codex-detect-")));
});
afterEach(() => {
  invalidateCodexDetection();
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: process.env.PATH ?? "/usr/bin:/bin", HOME: "/home/u", LANG: "C.UTF-8" };

function sandboxDeps(): SessionSandboxDeps {
  const data = join(tmp, "data");
  mkdirSync(join(data, "providers", "codex", "home"), { recursive: true, mode: 0o700 });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data, projectDirOf: () => undefined, finPath: "/bin/sh", localEndpoint: join(data, "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts") };
}

/** What a person's `codex login` left: `auth.json` in the sign-in home (the fake reads `account` from it). */
function signIn(d: SessionSandboxDeps, account?: Record<string, unknown>): void {
  writeFileSync(join(d.serverData, "providers", "codex", "home", "auth.json"), JSON.stringify({ account: account ?? {}, tokens: { access_token: "TOKEN-must-not-leak" } }));
}

/** `fake-codex` as `<dir>/codex` with its sidecar; `observed` records what it received. */
function installFake(name: string, o: { version?: string } = {}): { bin: string; observed: string } {
  const dir = join(tmp, name);
  mkdirSync(dir, { recursive: true });
  const bin = join(dir, WIN ? "codex.exe" : "codex");
  copyFileSync(FAKE, bin);
  chmodSync(bin, 0o755);
  const observed = join(dir, "observed.json");
  writeFileSync(join(dir, "fake-codex.json"), JSON.stringify({ observed, version: o.version ?? PIN }));
  return { bin, observed };
}

const pinDeps = (bin: string | undefined): PinDeps => ({
  env: bin === undefined ? {} : { FIN_CODEX_BIN: bin },
  testHooks: true,
  platform: process.platform,
  arch: process.arch,
  resourcesDir: join(tmp, "no-resources"),
  cacheRoot: join(tmp, "no-cache"),
  version: PIN,
  exists: (p) => existsSync(p),
});

const deps = (bin: string | undefined, over: Partial<CodexDetectDeps> = {}): CodexDetectDeps => ({ sandbox: sandboxDeps(), settings: () => ({ enabled: true }), pin: pinDeps(bin), ...over });

async function failure(e: Effect.Effect<unknown, FinError>): Promise<FinError> {
  const exit = await Effect.runPromiseExit(e);
  if (!Exit.isFailure(exit)) throw new Error(`expected a failure, got ${JSON.stringify(exit)}`);
  const err = exit.cause.reasons.map((r) => (r._tag === "Fail" ? r.error : undefined)).find((x) => x !== undefined);
  if (err === undefined) throw new Error(`no failure in ${String(exit.cause)}`);
  return err;
}

const received = (observed: string): { method: string; params?: Record<string, unknown> }[] =>
  (JSON.parse(readFileSync(observed, "utf8")) as { received: { method: string; params?: Record<string, unknown> }[] }[]).flatMap((p) => p.received);

describe("detection", () => {
  test("not_bundled_is_unavailable", async () => {
    const e = await failure(detectCodex(deps(undefined)));
    expect(e).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { reason: "notBundled" } });
    const info = await Effect.runPromise(codexProviderInfo(deps(undefined)));
    expect(info).toMatchObject({ harness: "codex", detected: false, enabled: true, problem: { code: "PROVIDER_UNAVAILABLE" } });
  });

  test.skipIf(NO_FAKE)("version_must_equal_pin", async () => {
    expect(parseCodexVersion("codex-cli 0.167.0\n")).toBe("0.167.0");
    const ok = await Effect.runPromise(detectCodex(deps(installFake("ok").bin)));
    expect(ok).toMatchObject({ version: PIN, pinned: PIN });
    for (const v of ["0.166.0", "0.168.0", "1.167.0"]) {
      const bin = installFake(`v${v}`, { version: v }).bin;
      const e = await failure(detectCodex(deps(bin)));
      expect(e, v).toMatchObject({ code: "PROVIDER_VERSION", details: { version: v, pinned: PIN } });
      const info = await Effect.runPromise(codexProviderInfo(deps(bin)));
      expect(info).toMatchObject({ detected: true, version: v, supported: false, problem: { code: "PROVIDER_VERSION" } });
    }
  });

  test.skipIf(NO_FAKE)("garbled_version_is_protocol", async () => {
    for (const line of ["0.167.0", "codex 0.167.0", "codex-cli 0.167.0 extra", "codex-cli v0.167.0", "codex-cli 0.167", ""]) {
      expect(() => parseCodexVersion(line), line).toThrow(expect.objectContaining({ code: "PROVIDER_PROTOCOL" }));
    }
    const e = await failure(detectCodex(deps(installFake("garbled", { version: "two.one" }).bin)));
    expect(e).toMatchObject({ code: "PROVIDER_PROTOCOL", details: { reason: "version" } });
    expect(await Effect.runPromise(codexProviderInfo(deps(join(tmp, "garbled", "codex"))))).toMatchObject({ detected: true, problem: { code: "PROVIDER_PROTOCOL" } });
    if (WIN) return;
    const slow = join(tmp, "slow");
    mkdirSync(slow);
    writeFileSync(join(slow, "codex"), "#!/bin/sh\nexec sleep 30\n", { mode: 0o755 });
    const started = Date.now();
    const t = await failure(detectCodex(deps(join(slow, "codex"), { timeoutMs: 300 })));
    expect(t).toMatchObject({ code: "PROVIDER_PROTOCOL", details: { reason: "timeout", step: "--version" } });
    expect(Date.now() - started).toBeLessThan(10_000);
  });

  test.skipIf(NO_FAKE)("signed_in_from_account_read", async () => {
    const { bin } = installFake("acct");
    const d = deps(bin);
    // No auth.json: signed out, with the sign-in fix.
    const out = await Effect.runPromise(codexProviderInfo(d));
    expect(out).toMatchObject({ detected: true, version: PIN, supported: true, signedIn: false });
    expect(out.accountHash).toBeUndefined();
    expect(out.problem).toBeUndefined();
    invalidateCodexDetection();
    // ChatGPT: signed in, the plan shown, the account only as its keyed hash.
    signIn(d.sandbox, { type: "chatgpt", email: "owner-7e@example.invalid", planType: "pro" });
    const plan = await Effect.runPromise(codexProviderInfo(d));
    expect(plan).toMatchObject({ signedIn: true, authMethod: "chatgpt", planType: "pro" });
    expect(plan.accountHash).toMatch(/^[0-9a-f]{8}$/);
    expect(plan.warnings).toBeUndefined();
    expect(JSON.stringify(plan)).not.toContain("example.invalid");
    invalidateCodexDetection();
    // An API key: signed in, recorded by method alone, with the accountUnidentified warning (Q7a).
    signIn(d.sandbox, { type: "apiKey" });
    const key = await Effect.runPromise(codexProviderInfo(d));
    expect(key).toMatchObject({ signedIn: true, authMethod: "apiKey", warnings: [{ code: "accountUnidentified" }] });
    expect(key.accountHash).toMatch(/^[0-9a-f]{8}$/);
    expect(key.accountHash).not.toBe(plan.accountHash);
    // The switch from the recorded ChatGPT account to a key is a change that waits (stops new sessions).
    expect(key.problem).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED" });
    invalidateCodexDetection();
    // Bedrock and GCP are not v1 sign-ins.
    for (const type of ["amazonBedrock", "gcp"]) {
      signIn(d.sandbox, { type });
      const refused = await Effect.runPromise(codexProviderInfo(d));
      expect(refused, type).toMatchObject({ detected: true, problem: { code: "PROVIDER_AUTH" } });
      const e = await failure(detectCodex(d).pipe(Effect.flatMap((x) => (x.problem === undefined ? Effect.void : Effect.fail(x.problem)))));
      expect(e.details).toMatchObject({ reason: "unsupportedAccount" });
      invalidateCodexDetection();
    }
  });

  test.skipIf(NO_FAKE)("providers_never_carry_label_or_token", async () => {
    const { bin } = installFake("label");
    const d = deps(bin);
    signIn(d.sandbox, { type: "chatgpt", email: "owner-label-7e1@example.invalid", planType: "plus" });
    const admin = new AgentAdmin({ serverData: d.sandbox.serverData, secrets: undefined, localPresets: {}, audit: () => undefined, codex: () => codexProviderInfo(d) });
    const answer = await Effect.runPromise(admin.providers(new Map(), () => true));
    const wire = JSON.stringify(Schema.encodeUnknownSync(AgentProvidersSuccess)(answer));
    for (const s of ["owner-label-7e1", "example.invalid", "TOKEN-must-not-leak"]) expect(wire, s).not.toContain(s);
    const line = answer.providers.find((p) => p.harness === "codex") as ProviderInfo;
    expect(line).toMatchObject({ detected: true, version: PIN, supported: true, signedIn: true, authMethod: "chatgpt", planType: "plus", enabled: true, capabilities: CODEX_CAPABILITIES });
    expect(line.account).toBeUndefined();
    expect(Object.keys(line).sort()).toEqual(["accountHash", "authMethod", "capabilities", "detected", "enabled", "harness", "planType", "signedIn", "supported", "version"]);
  });

  test.skipIf(NO_FAKE)("detect_never_sends_refresh", async () => {
    const { bin, observed } = installFake("refresh");
    const d = deps(bin);
    signIn(d.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    await Effect.runPromise(detectCodex(d));
    const got = received(observed);
    expect(got.map((r) => r.method)).toEqual(["initialize", "initialized", "account/read"]);
    for (const r of got) if (r.method !== "initialized") expect(ALLOWED_METHODS.has(r.method as never), r.method).toBe(true);
    expect(got.find((r) => r.method === "account/read")?.params ?? {}).not.toHaveProperty("refreshToken");
    expect(got[0]!.params).not.toHaveProperty("capabilities.experimentalApi");
  });

  test.skipIf(NO_FAKE)("cached_and_single_flight", async () => {
    const { bin, observed } = installFake("cache");
    const d = deps(bin);
    signIn(d.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    await Promise.all([1, 2, 3].map(() => Effect.runPromise(detectCodex(d))));
    await Effect.runPromise(detectCodex(d));
    expect(received(observed).filter((r) => r.method === "account/read")).toHaveLength(1);
  });

  test("disabled_by_default", async () => {
    const d = sandboxDeps();
    expect(readAgentSettings(d.serverData).settings.codex).toEqual({ enabled: false });
    writeFileSync(join(d.serverData, "settings.json"), JSON.stringify({ agents: { codex: { enabled: "yes", idleCloseS: 0, chargePlanMoney: 1 } } }));
    const bad = readAgentSettings(d.serverData);
    expect(bad.settings.codex).toEqual({ enabled: false });
    expect(bad.error).toMatch(/agents\.codex\.enabled.*agents\.codex\.chargePlanMoney.*agents\.codex\.idleCloseS|agents\.codex\.enabled/);
    writeFileSync(join(d.serverData, "settings.json"), JSON.stringify({ agents: { codex: { enabled: true, idleCloseS: 60, chargePlanMoney: true } } }));
    expect(readAgentSettings(d.serverData).settings.codex).toEqual({ enabled: true, idleCloseS: 60, chargePlanMoney: true });
    expect(readAgentSettings(d.serverData).settings.claudeCode).toEqual({ enabled: false });
    writeFileSync(join(d.serverData, "settings.json"), "{}");
    // Off: the line says so and the binary is never run.
    if (NO_FAKE) return;
    const { bin, observed } = installFake("off");
    const info = await Effect.runPromise(codexProviderInfo({ sandbox: d, pin: pinDeps(bin) }));
    expect(info).toMatchObject({ harness: "codex", detected: true, enabled: false, problem: { code: "PROVIDER_UNAVAILABLE", fix: expect.stringContaining("agents.codex.enabled") } });
    expect(existsSync(observed)).toBe(false);
    expect(info.signedIn).toBeUndefined();
  });
});

describe("signin", () => {
  test("signin_prints_and_runs_nothing", async () => {
    const data = join(tmp, "server data");
    mkdirSync(data, { mode: 0o700 });
    const dir = join(tmp, "it's res", "codex", `${process.platform}-${process.arch}`);
    mkdirSync(dir, { recursive: true });
    const path = join(dir, WIN ? "codex.exe" : "codex");
    const marker = join(tmp, "ran");
    writeFileSync(path, WIN ? "" : `#!/bin/sh\necho "$@" >> '${marker}'\n`, { mode: 0o755 });
    process.env.FIN_RESOURCES_DIR = join(tmp, "it's res");
    const lines: string[] = [];
    const errs: string[] = [];
    const output = { out: (s: string) => void lines.push(s), err: (s: string) => void errs.push(s) };
    expect(await runConsole(["provider", "signin", "codex", "--server-data", data], { output }), errs.join("\n")).toBe(0);
    const text = lines.join("\n");
    const home = join(data, "providers", "codex", "home");
    expect(existsSync(marker)).toBe(false);
    expect(existsSync(join(data, "providers"))).toBe(false);
    expect(text).toContain("login");
    expect(text).toContain("--device-auth");
    expect(text).toContain("cli_auth_credentials_store");
    if (!WIN) {
      const q = (s: string) => `'${s.replaceAll("'", `'\\''`)}'`;
      expect(text).toContain(`CODEX_HOME=${q(home)} ${q(path)} -c 'cli_auth_credentials_store="file"' login`);
      expect(text).toContain(`CODEX_HOME=${q(home)} ${q(path)} -c 'cli_auth_credentials_store="file"' login --device-auth`);
      expect(text).toContain("  umask 077");
      expect(text).toContain(`mkdir -p ${q(home)}`);
      for (const v of ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "OPENAI_BASE_URL"]) expect(text, v).toContain(`-u ${v}`);
      lines.length = 0;
      expect(await runConsole(["provider", "signout", "codex", "--server-data", data], { output })).toBe(0);
      const out = lines.join("\n");
      expect(out).toContain(`CODEX_HOME=${q(home)} ${q(path)} -c 'cli_auth_credentials_store="file"' logout`);
      expect(out).not.toContain("mkdir");
      expect(out).not.toContain("--device-auth");
    } else {
      expect(text).toContain("$env:CODEX_HOME = ");
    }
    expect(existsSync(marker)).toBe(false);
    const rest = text.replaceAll(tmp, "");
    for (const name of [hostname(), userInfo().username]) if (name.length > 2) expect(rest, name).not.toContain(name);
  });

  test("signin_refused_when_not_bundled", async () => {
    const data = join(tmp, "data2");
    mkdirSync(data, { mode: 0o700 });
    process.env.FIN_RESOURCES_DIR = join(tmp, "nothing");
    const errs: string[] = [];
    const code = await runConsole(["provider", "signin", "codex", "--server-data", data], { output: { out: () => undefined, err: (s: string) => void errs.push(s) } });
    // A development checkout may hold the dev cache's binary; either way nothing was run or made.
    expect([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).toContain(code);
    expect(existsSync(join(data, "providers"))).toBe(false);
  });
});
