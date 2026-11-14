// Codex's detection, its `agent.providers` line and the sign-in commands (plan 7E Task 6,
// Decisions 1, 8, 11, 20, 21; Owner Q7(a)). Everything runs `fake-codex` (no real CLI, no sign-in).
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { hostname, tmpdir, userInfo } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { AgentProvidersSuccess, type FinError, type ProviderInfo } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Schema } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { AgentAdmin } from "../src/agent/admin.ts";
import { ALLOWED_METHODS } from "../src/agent/codex/rpc.ts";
import { accountKey, accountsOf } from "../src/agent/cli/account.ts";
import { AUTH_TTL_MS, accountAnswer, CODEX_CAPABILITIES, type CodexDetectDeps, codexProviderInfo, codexSigninCommands, codexSigninLines, detectCodex, invalidateCodexDetection, parseCodexVersion } from "../src/agent/codex/detect.ts";
import { providerLine } from "../src/agent/console.ts";
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
function installFake(name: string, o: { version?: string; script?: Record<string, unknown> } = {}): { bin: string; observed: string } {
  const dir = join(tmp, name);
  mkdirSync(dir, { recursive: true });
  const bin = join(dir, WIN ? "codex.exe" : "codex");
  copyFileSync(FAKE, bin);
  chmodSync(bin, 0o755);
  const observed = join(dir, "observed.json");
  if (o.script !== undefined) writeFileSync(join(dir, "script.json"), JSON.stringify(o.script));
  writeFileSync(join(dir, "fake-codex.json"), JSON.stringify({ observed, version: o.version ?? PIN, ...(o.script === undefined ? {} : { script: "script.json" }) }));
  return { bin, observed };
}

const pinDeps = (bin: string | undefined): PinDeps => ({
  env: bin === undefined ? {} : { FIN_CODEX_BIN: bin },
  testHooks: true,
  devCheckout: false,
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
    // No auth.json: signed out, and no problem (`signedIn: false` is the whole answer).
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
      expect(e.details).toMatchObject({ reason: "unsupportedAccount", authMethod: type });
      invalidateCodexDetection();
    }
    // A type no pin names is "other", in the line and in the refusal's details alike.
    const odd = accountAnswer(d.sandbox.serverData, { type: "evil type<x>" } as never);
    expect(odd).toMatchObject({ signedIn: true, authMethod: "other", problem: { code: "PROVIDER_AUTH", details: { reason: "unsupportedAccount", authMethod: "other" } } });
    expect(JSON.stringify(odd)).not.toContain("evil");
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

  test.skipIf(NO_FAKE)("chatgpt_without_label_fails_closed", async () => {
    // A ChatGPT account whose `email` is null or empty is never identified: no hash, nothing recorded.
    for (const email of [null, ""]) {
      const { bin } = installFake(`nolabel-${email === null ? "null" : "empty"}`);
      const d = deps(bin);
      signIn(d.sandbox, { type: "chatgpt", email, planType: "plus" });
      const found = await Effect.runPromise(detectCodex(d));
      expect(found, String(email)).toMatchObject({ signedIn: true, authMethod: "chatgpt", problem: { code: "PROVIDER_ACCOUNT_CHANGED", details: { reason: "noAccountLabel" } } });
      expect(found.accountHash).toBeUndefined();
      const info = await Effect.runPromise(codexProviderInfo(d));
      expect(info).toMatchObject({ signedIn: true, problem: { code: "PROVIDER_ACCOUNT_CHANGED" } });
      expect(info.accountHash).toBeUndefined();
      expect(existsSync(join(d.sandbox.serverData, "agents", "codex-account.json"))).toBe(false);
      invalidateCodexDetection();
    }
  });

  test.skipIf(NO_FAKE)("failed_account_read_is_not_signed_in", async () => {
    // The fake's `accountRead: "crash"` exits after `initialize` without answering `account/read`.
    const crash = installFake("crash", { script: { accountRead: "crash" } });
    const d = deps(crash.bin);
    signIn(d.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    const crashed = await Effect.runPromise(detectCodex(d));
    expect(crashed).toMatchObject({ version: PIN, signedIn: null, problem: { code: "PROVIDER_EXITED" } });
    expect(crashed.accountHash).toBeUndefined();
    const info = await Effect.runPromise(codexProviderInfo(d));
    expect(info).toMatchObject({ detected: true, signedIn: null, problem: { code: "PROVIDER_EXITED" } });
    expect(existsSync(join(d.sandbox.serverData, "agents", "codex-account.json"))).toBe(false);
    invalidateCodexDetection();
    // `accountRead: "hang"` never answers: the timeout kills the process.
    const hang = installFake("hang", { script: { accountRead: "hang" } });
    const h = deps(hang.bin, { timeoutMs: 500 });
    signIn(h.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    const started = Date.now();
    const hung = await Effect.runPromise(detectCodex(h));
    expect(Date.now() - started).toBeLessThan(5_000);
    expect(hung).toMatchObject({ signedIn: null, problem: { code: "PROVIDER_PROTOCOL", details: { reason: "timeout", step: "account/read" } } });
    const runs = JSON.parse(readFileSync(hang.observed, "utf8")) as { pid: number; argv: string[]; exit?: unknown }[];
    const server = runs.find((r) => r.argv.includes("app-server"))!;
    // Killed, not exited: the fake records no exit of its own, and the process is gone.
    expect(server.exit).toBeUndefined();
    expect(() => process.kill(server.pid, 0)).toThrow();
  });

  test.skipIf(NO_FAKE)("detect_uses_isolating_flags", async () => {
    // `accountRead: "hang"` holds the app-server open, so its CODEX_HOME can be looked at while it runs.
    const { bin, observed } = installFake("iso", { script: { accountRead: "hang" } });
    const d = deps(bin, { timeoutMs: 4_000 });
    signIn(d.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    const running = Effect.runPromise(detectCodex(d));
    const reading = () => existsSync(observed) && received(observed).some((r) => r.method === "account/read");
    for (let i = 0; i < 400 && !reading(); i++) await new Promise((r) => setTimeout(r, 20));
    expect(reading()).toBe(true);
    // The app-server's CODEX_HOME is the host sandbox's `provider/`, holding the `auth.json` link.
    const homes: string[] = [];
    const walk = (dir: string) => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        if (e.name === "auth.json") homes.push(dir);
        else if (e.isDirectory()) walk(join(dir, e.name));
      }
    };
    walk(join(d.sandbox.serverData, "agents"));
    expect(homes).toHaveLength(1);
    expect(readdirSync(homes[0]!)).not.toContain("skills");
    await running;
    const runs = JSON.parse(readFileSync(observed, "utf8")) as { argv: string[]; flags: { strictConfig: boolean; config: Record<string, string>; disable: string[] } }[];
    const server = runs.find((r) => r.argv.includes("app-server"))!;
    expect(server.flags.strictConfig).toBe(true);
    expect(server.flags.config).toMatchObject({
      "skills.bundled.enabled": "false",
      check_for_update_on_startup: "false",
      "analytics.enabled": "false",
      "history.persistence": '"none"',
      cli_auth_credentials_store: '"file"',
    });
  });

  test.skipIf(NO_FAKE)("stale_cache_never_clears_pending_change", async () => {
    const { bin, observed } = installFake("stale");
    let now = 1_000_000;
    const d = deps(bin, { now: () => now });
    signIn(d.sandbox, { type: "chatgpt", email: "a@example.invalid", planType: "plus" });
    const first = await Effect.runPromise(codexProviderInfo(d));
    expect(first.problem).toBeUndefined();
    const data = d.sandbox.serverData;
    const accounts = accountsOf(data, "codex");
    const b = accounts.hashOf({ signedIn: true, authMethod: "chatgpt", label: "b@example.invalid" }, accountKey(data));
    // An instance's own `account/read` saw another account: the change waits.
    expect(accounts.record(b, "chatgpt").changed).toBe(true);
    // Detection's cached answer (the earlier account, within AUTH_TTL_MS) leaves it waiting.
    const cached = await Effect.runPromise(codexProviderInfo(d));
    expect(received(observed).filter((r) => r.method === "account/read")).toHaveLength(1);
    expect(cached.problem).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED" });
    expect(cached.accountHash).toBe(b);
    expect(accounts.pending()).toBe(b);
    // A fresh read, begun after the change was seen, that names the recorded account: nothing waits.
    await new Promise((r) => setTimeout(r, 5));
    now += AUTH_TTL_MS + 1;
    const fresh = await Effect.runPromise(codexProviderInfo(d));
    expect(received(observed).filter((r) => r.method === "account/read")).toHaveLength(2);
    expect(fresh.problem).toBeUndefined();
    expect(fresh.accountHash).toBe(first.accountHash);
    expect(accounts.pending()).toBeUndefined();
  });

  test.skipIf(NO_FAKE)("providers_text_shows_plan_and_warnings", async () => {
    const { bin } = installFake("text");
    const d = deps(bin);
    signIn(d.sandbox, { type: "apiKey" });
    const key = providerLine(await Effect.runPromise(codexProviderInfo(d)));
    expect(key[0]).toContain("signed in (apiKey)");
    expect(key.slice(1)).toEqual([expect.stringMatching(/^ {2}warning: accountUnidentified: Codex is signed in with an API key/), expect.stringMatching(/^ {2}fix: sign in with a ChatGPT account/)]);
    invalidateCodexDetection();
    signIn(d.sandbox, { type: "chatgpt", email: "owner-text-7e@example.invalid", planType: "pro" });
    const plan = providerLine(await Effect.runPromise(codexProviderInfo(d)));
    expect(plan[0]).toContain("signed in (chatgpt, plan pro)");
    expect(plan.join("\n")).not.toContain("example.invalid");
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

  test("signin_refused_when_not_bundled", () => {
    const data = join(tmp, "data2");
    mkdirSync(data, { mode: 0o700 });
    // A pin that points at nothing: no test override, no resources, no dev cache.
    let e: FinError | undefined;
    try {
      codexSigninLines(data, "login", { pin: pinDeps(undefined), settings: { enabled: true } });
    } catch (x) {
      e = x as FinError;
    }
    expect(e).toMatchObject({ code: "PROVIDER_UNAVAILABLE", details: { reason: "notBundled" } });
    expect(existsSync(join(data, "providers"))).toBe(false);
  });

  test("signin_commands_golden_unix", () => {
    const i = { path: "/opt/it's fin/codex", serverData: "/srv/fin data", platform: "linux" as const };
    const env = "env -u OPENAI_API_KEY -u CODEX_API_KEY -u CODEX_ACCESS_TOKEN -u OPENAI_BASE_URL -u OPENAI_ORG_ID -u OPENAI_PROJECT_ID";
    const home = "'/srv/fin data/providers/codex/home'";
    const run = `  ${env} CODEX_HOME=${home} '/opt/it'\\''s fin/codex' -c 'cli_auth_credentials_store="file"'`;
    expect(codexSigninCommands("login", i)).toEqual([
      "# Run in sh, bash or zsh on this server, as the account that runs finisherd.",
      "# Sign in with a browser (a ChatGPT account, or an API key it offers):",
      "(",
      "  umask 077",
      `  mkdir -p ${home}`,
      `  chmod 700 ${home}`,
      `${run} login`,
      ")",
      "# Or, on a headless server, with a device code:",
      "(",
      "  umask 077",
      `  mkdir -p ${home}`,
      `  chmod 700 ${home}`,
      `${run} login --device-auth`,
      ")",
    ]);
    expect(codexSigninCommands("logout", { ...i, enabled: false })).toEqual([
      "# Run in sh, bash or zsh on this server, as the account that runs finisherd.",
      "# Codex is off on this server: sessions start on it once agents.codex.enabled is true in settings.json.",
      "(",
      "  umask 077",
      `${run} logout`,
      ")",
    ]);
  });

  test("signin_commands_powershell_quotes", () => {
    // On any host: the printer's Windows form, with `'` and U+2019 in the paths (each doubled).
    const login = codexSigninCommands("login", { path: "C:\\Users\\O'Neil\\codex.exe", serverData: "C:\\Fin\u2019s Data", platform: "win32" });
    const home = "'C:\\Fin\u2019\u2019s Data\\providers\\codex\\home'";
    const exe = "'C:\\Users\\O''Neil\\codex.exe'";
    expect(login).toEqual([
      "# Run in a new PowerShell window on this server, as the account that runs finisherd (it changes that window's environment).",
      `New-Item -ItemType Directory -Force -Path ${home} | Out-Null`,
      "Remove-Item Env:OPENAI_API_KEY, Env:CODEX_API_KEY, Env:CODEX_ACCESS_TOKEN, Env:OPENAI_BASE_URL, Env:OPENAI_ORG_ID, Env:OPENAI_PROJECT_ID -ErrorAction SilentlyContinue",
      `$env:CODEX_HOME = ${home}`,
      "# Sign in with a browser (a ChatGPT account, or an API key it offers):",
      `& ${exe} -c 'cli_auth_credentials_store="file"' login`,
      "# Or, on a headless server, with a device code:",
      `& ${exe} -c 'cli_auth_credentials_store="file"' login --device-auth`,
    ]);
    const logout = codexSigninCommands("logout", { path: "C:\\Users\\O\u2019Neil\\codex.exe", serverData: "C:\\Data", platform: "win32" });
    expect(logout).toContain("& 'C:\\Users\\O\u2019\u2019Neil\\codex.exe' -c 'cli_auth_credentials_store=\"file\"' logout");
    expect(logout.join("\n")).not.toContain("New-Item");
  });
});
