// The Codex capability probe and the per-instance check (plan 7E Task 8, Decision 8; Review
// Focus 1 and 6): free and unsigned, never a thread or a turn when something else signed Codex
// in, cached per `(pinned version, flags hash, OS)` with no account data, Decision 4's keys and
// Decision 6's features read back from `config/read` (effective value and origin), managed
// requirements, the five lists, `instructionSources`, dynamic tools, the version, and the gate
// that stops an instance whose check fails before any event reaches the runtime. Processes are
// `fake-codex` through a host sandbox (`direct`); `real_binary_passes_predicate` runs the pinned
// binary unsigned when `.cache/codex` holds it.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Fiber, Stream } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { type CodexLaunch, SHELL_DYNAMIC_TOOL } from "../src/agent/codex/launch.ts";
import {
  codexProbeCachePath,
  codexProbeFlagsHash,
  completeProbe,
  flagsHashOf,
  type InstanceFacts,
  type InstanceItem,
  instanceGate,
  type ProbeKey,
  probeCodex,
  probeInstance,
  probeLaunch,
  readCodexProbeCache,
} from "../src/agent/codex/probe.ts";
import { checkConfig, checkInstance, checkLists, checkRequirements, checkThread, expectedConfig, recheckSkills } from "../src/agent/codex/probe-check.ts";
import { ALLOWED_METHODS, type CodexRpc, codexRpc, type ServerNotification } from "../src/agent/codex/rpc.ts";
import { openHostSandbox, type SandboxedChild, type SessionSandbox, type SessionSandboxDeps } from "../src/agent/sandbox.ts";
import type { SandboxStatus } from "../src/sandbox/status.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-codex${WIN ? ".exe" : ""}`, import.meta.url));
const REQUIRE_FAKE = process.env.FIN_REQUIRE_SERVER === "1";
const withFake = test.skipIf(!existsSync(FAKE) && !REQUIRE_FAKE);
const PIN = "0.167.0";
const TARGET = `${process.platform}-${process.arch}`;
const REAL = fileURLToPath(new URL(`../../../.cache/codex/${PIN}/${TARGET}/codex${WIN ? ".exe" : ""}`, import.meta.url));
const OS = process.platform;

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-codex-probe-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });
const parent = WIN ? { Path: process.env.Path ?? process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows" } : { PATH: "/usr/bin:/bin", LANG: "C.UTF-8" };
const data = () => join(tmp, "data");
function sandboxDeps(): SessionSandboxDeps {
  mkdirSync(data(), { recursive: true, mode: 0o700 });
  mkdirSync(join(tmp, "hosts"), { recursive: true });
  return { serverData: data(), projectDirOf: () => undefined, finPath: "/bin/sh", localEndpoint: join(data(), "run", "fin.sock"), status: NOT_READY, killGraceMs: 2000, parent, hostRoot: join(tmp, "hosts") };
}

/** `fake-codex` as `<dir>/codex` with its sidecar and script; `observed` records every process. */
function install(script: Record<string, unknown> = {}, version = PIN): { binary: string; observed: string } {
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, WIN ? "codex.exe" : "codex");
  copyFileSync(FAKE, binary);
  const observed = join(tmp, "observed.json");
  writeFileSync(join(dir, "script.json"), JSON.stringify(script));
  writeFileSync(join(dir, "fake-codex.json"), JSON.stringify({ script: "script.json", observed, version }));
  return { binary, observed };
}

interface Received {
  readonly method: string;
  readonly params?: Record<string, unknown>;
}
type Observed = { pid: number; argv: string[]; envNames: string[]; capabilities: Record<string, unknown>; received: Received[] }[];
const observed = (path: string): Observed => (existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as Observed) : []);
const methods = (o: Observed[number]) => o.received.map((r) => r.method);

async function run<A>(e: Effect.Effect<A, FinError>): Promise<A> {
  const exit = await Effect.runPromiseExit(e);
  if (Exit.isSuccess(exit)) return exit.value;
  throw new Error(`failed: ${JSON.stringify(exit.cause)}`);
}
const asFin = (e: unknown): FinError => e as FinError;
/** The failure of `e` as `{code, ...details}`. */
async function failure(e: Effect.Effect<unknown, FinError> | Promise<unknown>): Promise<Record<string, unknown>> {
  let err: FinError;
  if (e instanceof Promise) {
    err = await e.then(
      () => {
        throw new Error("expected a failure");
      },
      (x: unknown) => asFin(x),
    );
  } else {
    const exit = await Effect.runPromiseExit(e);
    if (Exit.isSuccess(exit)) throw new Error("expected a failure");
    const f = exit.cause.reasons.find((r) => r._tag === "Fail");
    err = (f as { error: FinError }).error;
  }
  return { code: err.code, ...(err.details as Record<string, unknown>) };
}
const verdictOf = (e: FinError | null) => (e === null ? null : { code: e.code, ...(e.details as Record<string, unknown>) });

const probeDeps = (binary: string, over: { os?: NodeJS.Platform; timeoutMs?: number } = {}) => ({ sandbox: sandboxDeps(), binary, timeoutMs: 15_000, ...over });

// ---- a minimal instance harness (Task 10's session does this for real) ----

interface Instance {
  readonly box: SessionSandbox;
  readonly child: SandboxedChild;
  readonly rpc: CodexRpc;
  readonly codexHome: string;
}

/** Runs `f` with a host sandbox whose `CODEX_HOME` and workspace `plant` may fill first. */
async function withBox<A>(deps: SessionSandboxDeps, f: (box: SessionSandbox) => Promise<A>): Promise<A> {
  return run(Effect.scoped(Effect.flatMap(openHostSandbox("probe", "codex", deps), (box) => Effect.promise(() => f(box)))));
}

async function spawnInstance(box: SessionSandbox, launch: CodexLaunch): Promise<Instance> {
  const child = await run(box.spawn(launch.argv, { cwd: box.workspace, stdio: "pipe", env: launch.env, extraRead: [launch.argv[0]!] }));
  child.stderr.resume();
  return { box, child, rpc: codexRpc({ stdin: child.stdin, stdout: child.stdout }), codexHome: box.env.CODEX_HOME! };
}

/** A real instance's sequence, unchecked: what the session collects for `checkInstance`. */
async function collectFacts(i: Instance, launch: CodexLaunch): Promise<InstanceFacts> {
  const { rpc } = i;
  const initialize = await run(rpc.request("initialize", { clientInfo: { name: "finisher", version: "test" }, capabilities: { experimentalApi: true } }));
  await run(rpc.notify("initialized"));
  const config = (await run(rpc.requestRaw("config/read", { includeLayers: true }))).raw;
  const requirements = (await run(rpc.requestRaw("configRequirements/read", undefined))).raw;
  const cwds = [launch.startParams.cwd!];
  const skills = await run(rpc.request("skills/list", { cwds }));
  const hooks = await run(rpc.request("hooks/list", { cwds }));
  const plugins = await run(rpc.request("plugin/installed", {}));
  const mcpServers = await run(rpc.request("mcpServerStatus/list", {}));
  const apps = await run(rpc.request("app/installed", {}));
  const { model: _, ...start } = launch.startParams;
  const thread = await run(rpc.request("thread/start", start));
  return { initialize, config, requirements, skills, hooks, plugins, mcpServers, apps, thread, codexHome: i.codexHome };
}

/** The launch with `drop` removed from its argv (`-c k=v` pairs by value, `--disable F` by name). */
function without(l: CodexLaunch, drop: { config?: string; disable?: string }): CodexLaunch {
  const argv: string[] = [];
  for (let n = 0; n < l.argv.length; n++) {
    const [a, b] = [l.argv[n]!, l.argv[n + 1]];
    if ((a === "-c" && drop.config !== undefined && b?.startsWith(drop.config)) || (a === "--disable" && b === drop.disable)) {
      n++;
      continue;
    }
    argv.push(a);
  }
  return { ...l, argv };
}

/** Facts from one fake instance with the probe's own launch (`mutate` may change the plan's argv). */
async function factsOf(binary: string, mutate: (l: CodexLaunch) => CodexLaunch = (l) => l, plant: (box: SessionSandbox) => void = () => 0): Promise<{ facts: InstanceFacts; plan: CodexLaunch }> {
  return withBox(sandboxDeps(), async (box) => {
    plant(box);
    const plan = probeLaunch(box, OS, binary);
    const i = await spawnInstance(box, mutate(plan));
    try {
      return { facts: await collectFacts(i, plan), plan };
    } finally {
      i.rpc.end();
      i.child.kill("SIGKILL");
    }
  });
}

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;
const SKILL = "---\nname: x\ndescription: planted\n---\nbody\n";
const HOOKS = JSON.stringify({ hooks: { PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "echo hi", timeout: 5 }] }] } });
const PLANTED = {
  plugins: [{ name: "m", plugins: [{ id: "p@m", name: "p", enabled: true, installed: true, authPolicy: "ON_USE", installPolicy: "AVAILABLE", source: { type: "remote" } }] }],
  apps: [{ id: "a", enabled: true, callable: true }],
  mcpServers: [{ name: "s", authStatus: "unsupported", tools: {}, resources: [], resourceTemplates: [] }],
};

// ---- the probe ----

describe("the probe", () => {
  withFake("probe_is_free", async () => {
    const { binary, observed: obs } = install();
    const r = await run(probeCodex(probeDeps(binary)));
    expect(r).toMatchObject({ version: PIN, sandbox: "read-only", approval: "on-request" });
    expect(r.initPending).toBeUndefined();
    expect(r.models.length).toBeGreaterThan(0);
    expect(r.models.filter((m) => m.isDefault)).toHaveLength(1);
    const [p, ...rest] = observed(obs);
    expect(rest).toHaveLength(0);
    // Exactly Decision 8's sequence: free methods only, no turn, no login, no feature list.
    expect(methods(p!)).toEqual([
      "initialize",
      "initialized",
      "config/read",
      "configRequirements/read",
      "skills/list",
      "hooks/list",
      "plugin/installed",
      "mcpServerStatus/list",
      "app/installed",
      "account/read",
      ...methods(p!).filter((m) => m === "model/list"),
      "thread/start",
    ]);
    expect(methods(p!).every((m) => m === "initialized" || ALLOWED_METHODS.has(m as never))).toBe(true);
    expect(methods(p!).some((m) => m.startsWith("turn/") || /login|logout|oauth/i.test(m))).toBe(false);
    const init = p!.received[0]!.params!;
    expect(init.clientInfo).toMatchObject({ name: "finisher" });
    expect(init.capabilities).toEqual({ experimentalApi: true });
    expect(p!.received.find((x) => x.method === "config/read")!.params).toEqual({ includeLayers: true });
    const start = p!.received.find((x) => x.method === "thread/start")!.params!;
    expect(start.ephemeral).toBe(true);
    expect(start).toMatchObject({ sandbox: "read-only", approvalPolicy: "on-request", approvalsReviewer: "user", serviceName: "finisher", dynamicTools: [SHELL_DYNAMIC_TOOL] });
    expect(start.model).toBe(r.models.find((m) => m.isDefault)!.id);
    expect(p!.argv).toContain("--strict-config");
    // The host sandbox is gone.
    expect(readdirSync(join(data(), "agents")).filter((n) => n.startsWith("host-"))).toEqual([]);
  });

  withFake("probe_has_no_store", async () => {
    // A shared sign-in store exists; the probe's CODEX_HOME still has no auth.json (the fake is
    // signed out exactly when CODEX_HOME holds none), so the probe completes unsigned.
    mkdirSync(join(data(), "providers", "codex", "home"), { recursive: true });
    writeFileSync(join(data(), "providers", "codex", "home", "auth.json"), JSON.stringify({ account: { email: "x@example.com" } }));
    const { binary, observed: obs } = install();
    const r = await run(probeCodex(probeDeps(binary)));
    expect(r.initPending).toBeUndefined();
    const [p] = observed(obs);
    expect(p!.envNames).toContain("CODEX_HOME");
    expect(p!.envNames.filter((n) => /AUTH|TOKEN|API_KEY|STORE/i.test(n))).toEqual([]);
    // Directly: the probe launch adds no variable but NO_COLOR, and CODEX_HOME is the box's own
    // provider/ with no auth.json in it.
    await withBox(sandboxDeps(), async (box) => {
      const l = probeLaunch(box, OS, binary);
      expect(Object.keys(l.env)).toEqual(["NO_COLOR"]);
      expect(box.env.CODEX_HOME).toMatch(/[\\/]agents[\\/]host-[0-9A-Z]{26}[\\/]provider$/);
      expect(existsSync(join(box.env.CODEX_HOME!, "auth.json"))).toBe(false);
    });
  });

  withFake("probe_signed_in_by_other_means_defers", async () => {
    const { binary, observed: obs } = install({ signedInElsewhere: true });
    const r = await run(probeCodex(probeDeps(binary)));
    expect(r).toEqual({ version: PIN, sandbox: "read-only", approval: "on-request", models: [], initPending: true });
    const [p] = observed(obs);
    // Nothing after account/read: no model list, no thread, no turn.
    expect(methods(p!).at(-1)).toBe("account/read");
    expect(methods(p!)).not.toContain("thread/start");
    const text = readFileSync(codexProbeCachePath(data()), "utf8");
    expect(text).not.toMatch(/person@example\.com|chatgpt|plus|account/i);
    // The first real instance completes it (its own check passes). Its models are the signed-in
    // account's (Decision 10: per account, Task 9's cache), so the probe keeps its own.
    const { facts, plan } = await factsOf(binary);
    const key: ProbeKey = { version: PIN, flagsHash: codexProbeFlagsHash(OS), os: OS };
    const done = await run(completeProbe(data(), key, facts, plan));
    expect(done.initPending).toBeUndefined();
    expect(done.models).toEqual([]);
    expect((await readCodexProbeCache(data()))[0]!.result).toEqual({ version: PIN, sandbox: "read-only", approval: "on-request", models: [] });
    // A failing instance does not complete it.
    rmSync(codexProbeCachePath(data()));
    await run(probeCodex(probeDeps(binary)));
    const bad = clone(facts) as { thread: { instructionSources: string[] } } & InstanceFacts;
    bad.thread.instructionSources = ["/x/AGENTS.md"];
    expect(await failure(completeProbe(data(), key, bad, plan))).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation" });
    expect((await readCodexProbeCache(data()))[0]!.result.initPending).toBe(true);
  });

  withFake("probe_cached_per_version_flags_os", async () => {
    const { binary, observed: obs } = install();
    const first = await run(probeCodex(probeDeps(binary)));
    expect(await run(probeCodex(probeDeps(binary)))).toEqual(first);
    expect(observed(obs)).toHaveLength(1);
    // The entry is keyed by the hash of the launch actually probed.
    const path = codexProbeCachePath(data());
    expect((await readCodexProbeCache(data())).map((e) => e.flagsHash)).toEqual([codexProbeFlagsHash(OS)]);
    // An entry for other flags is not this build's.
    const other = JSON.parse(readFileSync(path, "utf8")) as { entries: { flagsHash: string }[] };
    other.entries[0]!.flagsHash = "0000000000000000";
    writeFileSync(path, JSON.stringify(other));
    await run(probeCodex(probeDeps(binary)));
    expect(observed(obs)).toHaveLength(2);
    if (!WIN) {
      await run(probeCodex(probeDeps(binary, { os: OS === "darwin" ? "linux" : "darwin" })));
      expect(observed(obs)).toHaveLength(3);
    }
    // Another pinned version's entry is not this one's.
    const file = JSON.parse(readFileSync(path, "utf8")) as { entries: { version: string; flagsHash: string; os: string }[] };
    for (const e of file.entries) e.version = "0.166.0";
    writeFileSync(path, JSON.stringify(file));
    const before = observed(obs).length;
    await run(probeCodex(probeDeps(binary)));
    expect(observed(obs)).toHaveLength(before + 1);
    // No account data, 0600, no temp file left, an excess key invalidates the file.
    const text = readFileSync(path, "utf8");
    expect(text).not.toMatch(/email|planType|authMethod|account|chatgpt/i);
    if (!WIN) expect(statSync(path).mode & 0o777).toBe(0o600);
    expect(readdirSync(join(data(), "agents")).filter((n) => n.endsWith(".tmp"))).toEqual([]);
    const tampered = JSON.parse(text) as { entries: { result: Record<string, unknown> }[] };
    tampered.entries[0]!.result.email = "x@example.com";
    writeFileSync(path, JSON.stringify(tampered));
    expect(await readCodexProbeCache(data())).toEqual([]);
  });

  withFake("flags_hash_covers_flags_not_session", async () => {
    const { binary } = install();
    await withBox(sandboxDeps(), async (box) => {
      const l = probeLaunch(box, OS, binary);
      const h = flagsHashOf(l);
      // Per-session values do not change it.
      expect(flagsHashOf({ ...l, argv: ["/other/codex", ...l.argv.slice(1)], startParams: { ...l.startParams, cwd: "/elsewhere", model: "m", developerInstructions: "x" } })).toBe(h);
      const set = l.argv.findIndex((a) => a.startsWith("shell_environment_policy.set="));
      const argv = [...l.argv];
      argv[set] = 'shell_environment_policy.set={A="b"}';
      expect(flagsHashOf({ ...l, argv })).toBe(h);
      // A flag, the sandbox or the tools do.
      expect(flagsHashOf(without(l, { disable: "hooks" }))).not.toBe(h);
      expect(flagsHashOf({ ...l, argv: [...l.argv, "--disable", "x"] })).not.toBe(h);
      expect(flagsHashOf({ ...l, startParams: { ...l.startParams, sandbox: "danger-full-access" } })).not.toBe(h);
      expect(flagsHashOf({ ...l, startParams: { ...l.startParams, dynamicTools: [] } })).not.toBe(h);
    });
    // The build's hash is the hash of the launch the probe runs.
    await withBox(sandboxDeps(), async (box) => expect(flagsHashOf(probeLaunch(box, OS, binary))).toBe(codexProbeFlagsHash(OS)));
    // The build's hash is stable and per OS-independent flags.
    expect(codexProbeFlagsHash("linux")).toBe(codexProbeFlagsHash("linux"));
    expect(codexProbeFlagsHash("linux")).toMatch(/^[0-9a-f]{16}$/);
    expect(codexProbeFlagsHash("win32")).toMatch(/^[0-9a-f]{16}$/);
  });

  withFake("version_other_than_pin_refused", async () => {
    const { binary, observed: obs } = install({}, "0.168.0");
    expect(await failure(probeCodex(probeDeps(binary)))).toMatchObject({ code: "PROVIDER_VERSION", version: "0.168.0", pinned: PIN });
    // Stopped at initialize; nothing cached.
    expect(methods(observed(obs)[0]!)).toEqual(["initialize"]);
    expect(await readCodexProbeCache(data())).toEqual([]);
  });

  withFake("no_dynamic_tools_refused", async () => {
    const { binary } = install({ refuseDynamicTools: true });
    expect(await failure(probeCodex(probeDeps(binary)))).toMatchObject({ code: "PROVIDER_VERSION", missing: "dynamicTools" });
    expect(await readCodexProbeCache(data())).toEqual([]);
  });
});

// ---- the predicate ----

describe("config/read", () => {
  withFake("config_read_mismatch_refused", async () => {
    const { binary } = install();
    const { facts, plan } = await factsOf(binary);
    const home = facts.codexHome!;
    expect(checkConfig(facts.config, plan, home)).toBeNull();
    expect(checkInstance(facts, plan)).toBeNull();
    type Raw = { config: Record<string, unknown>; origins: Record<string, unknown>; layers: Record<string, unknown>[] };
    const set = (o: Record<string, unknown>, path: string, v: unknown) => {
      const ks = path.split(".");
      let n = o;
      for (const k of ks.slice(0, -1)) n = n[k] as Record<string, unknown>;
      n[ks.at(-1)!] = v;
    };
    const other = (v: unknown): unknown => (Array.isArray(v) ? ["AGENTS.md"] : typeof v === "object" ? { x: {} } : typeof v === "number" ? 32768 : typeof v === "boolean" ? !v : `${String(v)}-other`);
    // Each key of Decision 4: a wrong effective value, then (scalars) an origin from a file.
    for (const [key, want] of expectedConfig(plan)) {
      const r = clone(facts.config) as Raw;
      set(r.config, key, other(want));
      expect(verdictOf(checkConfig(r, plan, home)), key).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item: `config:${key}` });
      if (typeof want !== "object") {
        const o = clone(facts.config) as Raw;
        o.origins[key] = { name: { type: "user", file: join(home, "config.toml") }, version: "x" };
        expect(verdictOf(checkConfig(o, plan, home)), `${key} origin`).toMatchObject({ missing: "isolation", item: `config:${key}` });
      }
      const gone = clone(facts.config) as Raw;
      const ks = key.split(".");
      delete ks.slice(0, -1).reduce((n, k) => n[k] as Record<string, unknown>, gone.config)[ks.at(-1)!];
      expect(verdictOf(checkConfig(gone, plan, home)), `${key} absent`).toMatchObject({ missing: `config.${key}` });
    }
    // Each disabled feature of Decision 6: on, then off but not by Finisher's flags.
    expect(plan.disabledFeatures.length).toBeGreaterThan(50);
    for (const f of plan.disabledFeatures) {
      const on = clone(facts.config) as Raw;
      (on.config.features as Record<string, unknown>)[f] = true;
      expect(verdictOf(checkConfig(on, plan, home)), f).toMatchObject({ missing: "isolation", item: `feature:${f}` });
      const notOurs = clone(facts.config) as Raw;
      delete notOurs.origins[`features.${f}`];
      delete notOurs.origins[`features.${f}.enabled`];
      expect(verdictOf(checkConfig(notOurs, plan, home)), `${f} origin`).toMatchObject({ missing: "isolation", item: `feature:${f}` });
    }
    // A feature on that Finisher never named (a re-pin's new default).
    const extra = clone(facts.config) as Raw;
    (extra.config.features as Record<string, unknown>).brand_new = true;
    expect(verdictOf(checkConfig(extra, plan, home))).toMatchObject({ item: "feature:brand_new" });
    // The shell environment is the argv's table; forced_login_method never set.
    const env = clone(facts.config) as Raw;
    set(env.config, "shell_environment_policy.set", { PATH: "/evil" });
    expect(verdictOf(checkConfig(env, plan, home))).toMatchObject({ item: "config:shell_environment_policy.set" });
    const forced = clone(facts.config) as Raw;
    forced.config.forced_login_method = "api";
    expect(verdictOf(checkConfig(forced, plan, home))).toMatchObject({ item: "config:forced_login_method" });
    // Layers: a system file with content, a user file elsewhere or not empty, a project layer.
    const layer = (f: (l: Record<string, unknown>[]) => void) => {
      const r = clone(facts.config) as Raw;
      f(r.layers);
      return verdictOf(checkConfig(r, plan, home));
    };
    const byType = (ls: Record<string, unknown>[], t: string) => ls.find((l) => (l.name as { type: string }).type === t)!;
    expect(layer((ls) => (byType(ls, "system").config = { model: "x" }))).toMatchObject({ item: "configLayer:system" });
    expect(layer((ls) => (byType(ls, "user").config = { model: "x" }))).toMatchObject({ item: "configLayer:user" });
    expect(layer((ls) => ((byType(ls, "user").name as Record<string, unknown>).file = "/home/u/.codex/config.toml"))).toMatchObject({ item: "configLayer:user" });
    expect(layer((ls) => ls.push({ name: { type: "project", dotCodexFolder: "/w/.codex" }, version: "v", config: {} }))).toMatchObject({ item: "configLayer:project" });
    expect(layer((ls) => ls.push({ name: { type: "brandNewSource" }, version: "v", config: {} }))).toMatchObject({ item: "configLayer:brandNewSource" });
    // A disabled layer contributes nothing.
    expect(layer((ls) => ls.push({ name: { type: "project", dotCodexFolder: "/w/.codex" }, version: "v", config: { model: "x" }, disabledReason: "untrusted" }))).toBeNull();
    // No layers at all (includeLayers ignored) fails closed.
    const noLayers = clone(facts.config) as Partial<Raw>;
    delete noLayers.layers;
    expect(verdictOf(checkConfig(noLayers, plan, home))).toMatchObject({ missing: "configLayers" });
  });

  withFake("config_read_mismatch_refused_end_to_end", async () => {
    // The instance really started without a key or a --disable: the fake reports the default.
    const { binary } = install();
    const deps = sandboxDeps();
    for (const [drop, item] of [
      [{ config: "project_doc_max_bytes=" }, "config:project_doc_max_bytes"],
      [{ disable: "multi_agent" }, "feature:multi_agent"],
    ] as const) {
      const got = await withBox(deps, async (box) => {
        const plan = probeLaunch(box, OS, binary);
        const i = await spawnInstance(box, without(plan, drop));
        try {
          return await failure(probeInstance(i.rpc, plan, i.codexHome));
        } finally {
          i.child.kill("SIGKILL");
        }
      });
      expect(got).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item });
    }
  });
});

describe("requirements, lists, thread", () => {
  withFake("managed_requirement_refused", async () => {
    const { binary } = install({ requirements: { allowedApprovalPolicies: ["never"] } });
    expect(await failure(probeCodex(probeDeps(binary)))).toMatchObject({ code: "PROVIDER_VERSION", missing: "managedConfig", item: "allowedApprovalPolicies" });
    const { plan } = await factsOf(install().binary);
    const req = (requirements: unknown) => verdictOf(checkRequirements({ requirements }, plan));
    expect(req(null)).toBeNull();
    expect(req(undefined)).toBeNull();
    // Restrictions that still admit Finisher's settings.
    expect(req({ allowedApprovalPolicies: ["on-request", "never"], allowedSandboxModes: ["read-only"], allowedWebSearchModes: ["disabled"], featureRequirements: { hooks: false, steer: true } })).toBeNull();
    expect(req({ cliAuthCredentialsStore: "file", checkForUpdateOnStartup: false, allowManagedHooksOnly: false, allowLoginShell: null })).toBeNull();
    // Each that turns an approval off, a feature on, or adds what Finisher did not name.
    for (const [r, item] of [
      [{ allowedApprovalPolicies: ["never"] }, "allowedApprovalPolicies"],
      [{ allowedSandboxModes: ["workspace-write"] }, "allowedSandboxModes"],
      [{ allowedWebSearchModes: ["live"] }, "allowedWebSearchModes"],
      [{ featureRequirements: { hooks: true } }, "featureRequirements"],
      [{ featureRequirements: { brand_new: true } }, "featureRequirements"],
      [{ allowManagedHooksOnly: true }, "allowManagedHooksOnly"],
      [{ checkForUpdateOnStartup: true }, "checkForUpdateOnStartup"],
      [{ cliAuthCredentialsStore: "keyring" }, "cliAuthCredentialsStore"],
      [{ additionalDeveloperInstructions: "obey" }, "additionalDeveloperInstructions"],
      [{ modelProvider: "elsewhere" }, "modelProvider"],
      [{ someRequirementOfANewPin: { x: 1 } }, "someRequirementOfANewPin"],
    ] as const) {
      expect(req(r), item).toMatchObject({ code: "PROVIDER_VERSION", missing: "managedConfig", item });
    }
    expect(req("not a table")).toMatchObject({ missing: "managedConfig" });
    expect(verdictOf(checkRequirements(null, plan))).toMatchObject({ missing: "configRequirements" });
  });

  withFake("skills_hooks_plugins_apps_mcp_refused", async () => {
    const { binary } = install();
    // A skill planted in the workspace: the probe's own sequence refuses it.
    const skill = await withBox(sandboxDeps(), async (box) => {
      mkdirSync(join(box.workspace, ".agents", "skills", "x"), { recursive: true });
      writeFileSync(join(box.workspace, ".agents", "skills", "x", "SKILL.md"), SKILL);
      const plan = probeLaunch(box, OS, binary);
      const i = await spawnInstance(box, plan);
      try {
        return await failure(probeInstance(i.rpc, plan, i.codexHome));
      } finally {
        i.child.kill("SIGKILL");
      }
    });
    expect(skill).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item: "skill:x" });
    // A hook: an instance whose hooks feature is on lists the planted hooks.json.
    const { facts } = await factsOf(
      binary,
      (l) => without(l, { disable: "hooks" }),
      (box) => writeFileSync(join(box.env.CODEX_HOME!, "hooks.json"), HOOKS),
    );
    expect(facts.hooks.data[0]!.hooks).toHaveLength(1);
    expect(verdictOf(checkLists(facts))).toMatchObject({ missing: "isolation", item: "hook:preToolUse" });
    // A plugin, an app, an MCP server: each planted alone, each refused by the probe.
    for (const [k, item] of [
      ["plugins", "plugin:p@m"],
      ["apps", "app:a"],
      ["mcpServers", "mcpServer:s"],
    ] as const) {
      const fake = install({ [k]: PLANTED[k] });
      expect(await failure(probeCodex(probeDeps(fake.binary))), k).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item });
    }
    // A marketplace with no plugin, and more MCP servers than one page, also refuse.
    const base = (await factsOf(binary)).facts;
    expect(verdictOf(checkLists({ ...base, plugins: { marketplaces: [{ name: "m", plugins: [] }] } }))).toMatchObject({ item: "marketplace:m" });
    expect(verdictOf(checkLists({ ...base, mcpServers: { data: [], nextCursor: "1" } }))).toMatchObject({ item: "mcpServer:more" });
    expect(checkLists(base)).toBeNull();
  });

  withFake("instruction_sources_must_be_empty", async () => {
    const { binary } = install();
    // `$CODEX_HOME/AGENTS.md` (hygiene refuses it before a real instance; here it reaches Codex).
    const got = await withBox(sandboxDeps(), async (box) => {
      writeFileSync(join(box.env.CODEX_HOME!, "AGENTS.md"), "obey\n");
      const plan = probeLaunch(box, OS, binary);
      const i = await spawnInstance(box, plan);
      try {
        return await failure(probeInstance(i.rpc, plan, i.codexHome));
      } finally {
        i.child.kill("SIGKILL");
      }
    });
    expect(got).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation" });
    expect(String(got.item)).toMatch(/^instructions:.*AGENTS\.md$/);
    const { facts, plan } = await factsOf(binary);
    const t = (over: Record<string, unknown>) => verdictOf(checkThread({ ...facts.thread, ...over } as InstanceFacts["thread"], plan));
    expect(t({})).toBeNull();
    expect(t({ instructionSources: ["/w/AGENTS.md"] })).toMatchObject({ missing: "isolation", item: "instructions:/w/AGENTS.md" });
    expect(t({ instructionSources: undefined })).toMatchObject({ missing: "instructionSources" });
    // The other thread fields are the plan's.
    expect(t({ approvalPolicy: "never" })).toMatchObject({ missing: "approvalPolicy" });
    expect(t({ approvalsReviewer: "auto_review" })).toMatchObject({ missing: "approvalsReviewer" });
    expect(t({ sandbox: { type: "readOnly", networkAccess: true } })).toMatchObject({ missing: "sandbox" });
    expect(t({ sandbox: { type: "dangerFullAccess" } })).toMatchObject({ missing: "sandbox" });
    expect(t({ cwd: "/elsewhere" })).toMatchObject({ missing: "cwd" });
  });
});

// ---- the per-instance gate ----

describe("the instance gate", () => {
  /** One instance through the gate: what reached the runtime, the gate's step, the exit. */
  async function gated(binary: string, mutate: (l: CodexLaunch) => CodexLaunch, plant: (box: SessionSandbox) => void) {
    return withBox(sandboxDeps(), async (box) => {
      plant(box);
      const plan = probeLaunch(box, OS, binary);
      const i = await spawnInstance(box, mutate(plan));
      const gate = instanceGate<ServerNotification>(plan);
      const runtime: ServerNotification[] = [];
      let held = 0;
      const pump = Effect.runFork(
        Stream.runForEach(i.rpc.notifications, (n) =>
          Effect.sync(() => {
            const s = gate.push({ kind: "event", event: n });
            if (s.kind === "hold") held++;
            if (s.kind === "forward") runtime.push(...s.lines.flatMap((l: InstanceItem<ServerNotification>) => (l.kind === "event" ? [l.event] : [])));
          }),
        ),
      );
      const facts = await collectFacts(i, plan);
      // Let the thread/started notification arrive before the check.
      await new Promise((r) => setTimeout(r, 100));
      const step = gate.verify({ kind: "facts", facts });
      if (step.kind === "refuse") i.child.kill("SIGKILL");
      if (step.kind === "forward") runtime.push(...step.lines.flatMap((l) => (l.kind === "event" ? [l.event] : [])));
      const heldAtCheck = held;
      // A refused gate drops whatever comes later.
      const later = gate.push({ kind: "event", event: { method: "unknown", name: "late", params: {} } });
      if (step.kind !== "refuse") {
        i.rpc.end();
        i.child.kill("SIGKILL");
      }
      const exit = await i.child.exited;
      await Effect.runPromise(Fiber.interrupt(pump));
      return { step, runtime, heldAtCheck, later, exit };
    });
  }

  withFake("instance_check_stops_instance", async () => {
    const { binary } = install();
    // config/read shows the hooks feature on (its --disable dropped) and a hook is planted.
    const bad = await gated(
      binary,
      (l) => without(l, { disable: "hooks" }),
      (box) => writeFileSync(join(box.env.CODEX_HOME!, "hooks.json"), HOOKS),
    );
    expect(bad.step.kind).toBe("refuse");
    if (bad.step.kind !== "refuse") throw new Error("unreachable");
    expect(bad.step.kill).toBe(true);
    expect({ code: bad.step.error.code, ...(bad.step.error.details as object) }).toMatchObject({ code: "PROVIDER_VERSION", harness: "codex", missing: "isolation", item: "feature:hooks" });
    // Events came (remoteControl/status/changed, thread/started), none reached the runtime.
    expect(bad.heldAtCheck).toBeGreaterThan(0);
    expect(bad.runtime).toEqual([]);
    expect(bad.later.kind).toBe("drop");
    // The group was killed, not left to end on its own.
    expect(bad.exit.signal).toBe("SIGKILL");
    // Control: the same instance with Finisher's flags opens the gate and releases what it held.
    const good = await gated(binary, (l) => l, () => 0);
    expect(good.step.kind).toBe("forward");
    expect(good.runtime.length).toBe(good.heldAtCheck);
    expect(good.runtime.map((n) => n.method)).toContain("thread/started");
  });
});

describe("the instance gate, mid-instance", () => {
  withFake("skills_changed_stops_instance", async () => {
    const { binary } = install();
    const { facts, plan } = await factsOf(binary);
    const ev = (n: ServerNotification): InstanceItem<ServerNotification> => ({ kind: "event", event: n });
    // Before the check: refused at once, nothing held is released.
    const early = instanceGate<ServerNotification>(plan);
    expect(early.push(ev({ method: "unknown", name: "thread/started", params: {} })).kind).toBe("hold");
    const s1 = early.push(ev({ method: "unknown", name: "skills/changed", params: {} }));
    expect(s1.kind).toBe("refuse");
    expect(s1.kind === "refuse" && (s1.error.details as { item?: string }).item).toBe("skills:changed");
    expect(early.verify({ kind: "facts", facts }).kind).toBe("drop");
    // After the check opened the gate: refused too, later events dropped.
    const late = instanceGate<ServerNotification>(plan);
    expect(late.verify({ kind: "facts", facts }).kind).toBe("forward");
    expect(late.push(ev({ method: "unknown", name: "other", params: {} })).kind).toBe("forward");
    expect(late.push(ev({ method: "unknown", name: "skills/changed", params: {} })).kind).toBe("refuse");
    expect(late.push(ev({ method: "unknown", name: "other", params: {} })).kind).toBe("drop");
  });

  withFake("fallback_sandbox_refused_until_exec_verified", async () => {
    // Decision 7's fallback (danger-full-access + granular) is refused: unified_exec is not
    // provably off unsigned (TODO(7E Task 0 Step 3)).
    const { binary } = install();
    const { facts, plan } = await factsOf(binary);
    const granular = { granular: { sandbox_approval: true, rules: true, mcp_elicitations: false, request_permissions: false, skill_approval: false } };
    const fallback: CodexLaunch = { ...plan, startParams: { ...plan.startParams, sandbox: "danger-full-access", approvalPolicy: granular } };
    const thread = { ...facts.thread, sandbox: { type: "dangerFullAccess" }, approvalPolicy: granular } as InstanceFacts["thread"];
    expect(verdictOf(checkInstance({ ...facts, thread }, fallback))).toMatchObject({ code: "PROVIDER_VERSION", missing: "patchApproval", reason: "unverifiedExec" });
    expect(verdictOf(checkThread(thread, fallback))).toMatchObject({ missing: "patchApproval" });
    // Read-only with a granular policy, or full access with on-request, neither.
    expect(verdictOf(checkThread(facts.thread, { startParams: { ...plan.startParams, approvalPolicy: granular } }))).toMatchObject({ missing: "patchApproval" });
    expect(verdictOf(checkThread(facts.thread, { startParams: { ...plan.startParams, sandbox: "danger-full-access" } }))).toMatchObject({ missing: "patchApproval" });
    expect(checkThread(facts.thread, plan)).toBeNull();
  });
});

describe("review fixes", () => {
  withFake("config_keys_cover_argv", async () => {
    // Every `-c` key the launch passes is one the check reads (the `set` table on its own).
    const { binary } = install();
    await withBox(sandboxDeps(), async (box) => {
      const plan = probeLaunch(box, OS, binary);
      const keys = plan.argv.flatMap((a, i) => (plan.argv[i - 1] === "-c" ? [a.slice(0, a.indexOf("="))] : [])).filter((k) => k !== "shell_environment_policy.set");
      expect(keys.length).toBe(11);
      expect(new Set(expectedConfig(plan).map(([k]) => k))).toEqual(new Set(keys));
    });
  });

  withFake("instance_codex_home_mismatch_refused", async () => {
    const { binary } = install();
    const { facts, plan } = await factsOf(binary);
    expect(checkInstance(facts, plan)).toBeNull();
    // The instance runs on another CODEX_HOME than the session's own.
    expect(verdictOf(checkInstance({ ...facts, codexHome: "/elsewhere/provider" }, plan))).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item: "codexHome" });
    expect(verdictOf(checkInstance({ ...facts, initialize: { ...facts.initialize, codexHome: "/elsewhere/provider" } }, plan))).toMatchObject({ missing: "isolation", item: "codexHome" });
  });

  withFake("skills_recheck_before_turn", async () => {
    // A skill planted in the workspace mid-instance raises no `skills/changed` on the real pin;
    // `recheckSkills` (`skills/list {forceReload}`) before a turn sees it.
    const { binary, observed: obs } = install();
    const got = await withBox(sandboxDeps(), async (box) => {
      const plan = probeLaunch(box, OS, binary);
      const i = await spawnInstance(box, plan);
      try {
        const facts = await collectFacts(i, plan);
        expect(checkInstance(facts, plan)).toBeNull();
        const cwds = [plan.startParams.cwd!];
        await run(recheckSkills(i.rpc, cwds));
        mkdirSync(join(box.workspace, ".agents", "skills", "x"), { recursive: true });
        writeFileSync(join(box.workspace, ".agents", "skills", "x", "SKILL.md"), SKILL);
        return await failure(recheckSkills(i.rpc, cwds));
      } finally {
        i.child.kill("SIGKILL");
      }
    });
    expect(got).toMatchObject({ code: "PROVIDER_VERSION", missing: "isolation", item: "skill:x" });
    const lists = observed(obs)[0]!.received.filter((r) => r.method === "skills/list");
    expect(lists.at(-1)!.params).toMatchObject({ forceReload: true });
  });

  withFake("config_read_nits", async () => {
    const { binary } = install();
    const { facts, plan } = await factsOf(binary);
    const home = facts.codexHome;
    type Raw = { config: { features: Record<string, unknown> }; origins: Record<string, unknown>; layers: Record<string, unknown>[] };
    // A feature Finisher did not name that is not off: any value but false/{enabled: false}/unset.
    for (const v of [true, "x", 1, { enabled: true }, {}, { enabled: null }]) {
      const r = clone(facts.config) as Raw;
      r.config.features.brand_new = v;
      expect(verdictOf(checkConfig(r, plan, home)), JSON.stringify(v)).toMatchObject({ missing: "isolation", item: "feature:brand_new" });
    }
    for (const v of [false, { enabled: false }, null]) {
      const r = clone(facts.config) as Raw;
      r.config.features.brand_new = v;
      expect(checkConfig(r, plan, home), JSON.stringify(v)).toBeNull();
    }
    // A key outside Decision 4 whose origin is a file.
    const o = clone(facts.config) as Raw;
    o.origins.model = { name: { type: "user", file: join(home, "config.toml") }, version: "x" };
    expect(verdictOf(checkConfig(o, plan, home))).toMatchObject({ missing: "isolation", item: "configOrigin:model" });
    // A layer with an empty `disabledReason` is not disabled.
    const l = clone(facts.config) as Raw;
    l.layers.push({ name: { type: "project", dotCodexFolder: "/w/.codex" }, version: "v", config: { model: "x" }, disabledReason: "" });
    expect(verdictOf(checkConfig(l, plan, home))).toMatchObject({ item: "configLayer:project" });
  });

  test.skipIf(WIN)("probe_times_out", async () => {
    // A binary that never answers: the probe gives up, kills it and caches nothing.
    const dir = mkdtempSync(join(tmp, "mute-"));
    const binary = join(dir, "codex");
    writeFileSync(binary, "#!/bin/sh\nexec sleep 30\n", { mode: 0o755 });
    const t0 = Date.now();
    expect(await failure(probeCodex({ sandbox: sandboxDeps(), binary, timeoutMs: 500 }))).toMatchObject({ code: "PROVIDER_VERSION", missing: "answer", timeoutMs: 500 });
    expect(Date.now() - t0).toBeLessThan(10_000);
    expect(await readCodexProbeCache(data())).toEqual([]);
  });
});

// ---- the real binary ----

describe("the pinned binary", () => {
  // Skipped without the cached binary, unless FIN_REQUIRE_SERVER=1 (then a missing binary fails).
  test.skipIf(!existsSync(REAL) && !REQUIRE_FAKE)(
    "real_binary_passes_predicate",
    async () => {
      // Unsigned: a throwaway host home, no auth.json, the parent environment PATH and LANG only.
      expect(existsSync(REAL), `the pinned binary at ${REAL} (pnpm codex:pin)`).toBe(true);
      const r = await run(probeCodex({ sandbox: sandboxDeps(), binary: REAL, timeoutMs: 60_000 }));
      expect(r).toMatchObject({ version: PIN, sandbox: "read-only", approval: "on-request" });
      expect(r.initPending).toBeUndefined();
      expect(r.models.length).toBeGreaterThan(0);
    },
    120_000,
  );
});
