// The harness-neutral CLI modules (plan 7E Task 1): the account record keyed by harness, the
// confirmation taking a harness (console and RPC), the generic first-event gate and the
// idle-close process loop. `claude_account_state_survives_lift` reads `claude-account.json`
// files written by the pre-lift code (`fixtures/cli-account/`, made with 7D's `ClaudeAccounts`
// at a fixed clock over the fixture's `account.key`, a test key): the lifted code must read them,
// hash, refuse, confirm and expire exactly as before, and write the same bytes.
import { createHmac } from "node:crypto";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, ProviderEvent } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import type { ProviderSession } from "../src/agent/adapter.ts";
import { type AuthStatus, accountCheck, accountFile, accountKey, accountProblem, accountsOf, CliAccounts, EXPECT_TTL_MS, instanceAccount, registerAccountAudit } from "../src/agent/cli/account.ts";
import { eventGate } from "../src/agent/cli/gate.ts";
import { type CliInstance, CliSession, type EventSink } from "../src/agent/cli/instances.ts";
import { invalidateCodexDetection } from "../src/agent/codex/detect.ts";
import { pinnedCodexVersion } from "../src/agent/codex/pin.ts";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { finError } from "../src/errors.ts";
import { AgentServer } from "./support/agent-server.ts";
import { ok } from "./support/ndjson-client.ts";
import { person } from "./support/person.ts";

const FIXTURES = join(import.meta.dirname, "fixtures", "cli-account");
const HASHES = JSON.parse(readFileSync(join(FIXTURES, "hashes.json"), "utf8")) as { a: string; b: string };
/** The labels and clock the fixtures were written with. */
const A: AuthStatus = { signedIn: true, authMethod: "claude.ai", label: "a@example.com" };
const B: AuthStatus = { signedIn: true, authMethod: "claude.ai", label: "b@example.com" };
const T0 = Date.parse("2026-11-14T10:00:00.000Z");

let tmp: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-cli-shared-")));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

/** A server data holding the fixture key and, if given, a fixture as `claude-account.json`. */
function serverData(fixture?: string): string {
  const data = join(tmp, "data");
  mkdirSync(join(data, "auth"), { recursive: true, mode: 0o700 });
  mkdirSync(join(data, "agents"), { recursive: true, mode: 0o700 });
  copyFileSync(join(FIXTURES, "account.key"), join(data, "auth", "account.key"));
  if (fixture !== undefined) copyFileSync(join(FIXTURES, fixture), join(data, "agents", "claude-account.json"));
  return data;
}

const fileOf = (data: string, h: "claudeCode" | "codex") => join(data, "agents", accountFile(h));
const read = (path: string) => readFileSync(path, "utf8");
const hmac = (domain: string, label: string, key: Buffer) => createHmac("sha256", key).update(domain, "utf8").update(`label\0${label}`, "utf8").digest("hex").slice(0, 8);

const thrown = (f: () => unknown): FinError => {
  try {
    f();
  } catch (e) {
    return e as FinError;
  }
  throw new Error("did not throw");
};

describe("the account record", () => {
  test("accounts_are_per_harness: a Codex change does not refuse Claude Code", () => {
    const data = serverData();
    const key = accountKey(data);
    expect(accountFile("claudeCode")).toBe("claude-account.json");
    expect(accountFile("codex")).toBe("codex-account.json");
    // Each harness hashes under its own domain: one label, two hashes.
    expect(accountCheck(A, key, null).hash).toBe(hmac("finisher.claudeCode.account.v1\0", A.label!, key));
    expect(accountCheck(A, key, null, "codex").hash).toBe(hmac("finisher.codex.account.v1\0", A.label!, key));
    const codex = new CliAccounts(data, { harness: "codex" });
    expect(codex.observe(A, key).changed).toBe(false);
    expect(codex.observe(B, key).changed).toBe(true);
    expect(existsSync(fileOf(data, "codex"))).toBe(true);
    expect(existsSync(fileOf(data, "claudeCode"))).toBe(false);
    // Claude Code sees nothing waiting; Codex does, under its own name.
    const claude = new CliAccounts(data, { harness: "claudeCode" });
    expect(claude.pending()).toBeUndefined();
    expect(accountProblem(data)).toBeUndefined();
    expect(accountProblem(data, "claudeCode")).toBeUndefined();
    expect(accountProblem(data, "codex")).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { harness: "codex", accountHash: accountCheck(B, key, null, "codex").hash } });
    expect(instanceAccount(data, A, { newSession: true }).refused).toBeNull();
    // And the reverse: a Claude Code change leaves Codex's record alone.
    expect(claude.observe(A, key).changed).toBe(false);
    const before = read(fileOf(data, "codex"));
    expect(claude.observe(B, key).changed).toBe(true);
    expect(read(fileOf(data, "codex"))).toBe(before);
    expect(codex.pending()).toBe(accountCheck(B, key, null, "codex").hash);
    expect(codex.confirm(accountCheck(B, key, null, "codex").hash).confirmed).toBe(true);
    expect(claude.pending()).toBe(accountCheck(B, key, null).hash);
  });

  test("method_only_is_codex_only: a Claude Code sign-in without a label still throws", () => {
    const data = serverData();
    const key = accountKey(data);
    const apiKey: AuthStatus = { signedIn: true, authMethod: "apiKey" };
    // The frozen hash refuses an unnamed account for either harness.
    expect(thrown(() => accountCheck(apiKey, key, null)).code).toBe("INTERNAL");
    expect(thrown(() => accountCheck(apiKey, key, null, "codex")).code).toBe("INTERNAL");
    // Claude Code never records a method alone, and may not be built to.
    expect(thrown(() => new CliAccounts(data, { harness: "claudeCode", methodOnly: ["apiKey"] })).code).toBe("INTERNAL");
    const claude = accountsOf(data, "claudeCode");
    expect(claude.identified(apiKey)).toBe(false);
    expect(thrown(() => claude.observe(apiKey, key)).code).toBe("INTERNAL");
    expect(thrown(() => claude.observe({ signedIn: true, authMethod: "claude.ai" }, key)).code).toBe("INTERNAL");
    expect(existsSync(fileOf(data, "claudeCode"))).toBe(false);
    // Codex without the option: the same refusal.
    expect(thrown(() => new CliAccounts(data, { harness: "codex" }).observe(apiKey, key)).code).toBe("INTERNAL");
    // Owner question 7(a): Codex's API key is recorded by its method alone; any other unnamed method is not.
    const codex = accountsOf(data, "codex");
    expect(codex.identified(apiKey)).toBe(true);
    expect(codex.identified({ signedIn: true, authMethod: "chatgpt" })).toBe(false);
    expect(thrown(() => codex.observe({ signedIn: true, authMethod: "chatgpt" }, key)).code).toBe("INTERNAL");
    const first = codex.observe(apiKey, key);
    expect(first).toMatchObject({ changed: false });
    expect(first.hash).toMatch(/^[0-9a-f]{8}$/);
    expect(codex.observe(apiKey, key)).toEqual(first);
    // A switch between a key and a ChatGPT plan is still a change.
    expect(codex.observe({ signedIn: true, authMethod: "chatgpt", label: "a@example.com" }, key).changed).toBe(true);
    // The method's hash is never a label's.
    expect(first.hash).not.toBe(accountCheck({ signedIn: true, label: "apiKey" }, key, null, "codex").hash);
  });

  test("claude_account_state_survives_lift: the pre-lift record reads back with the same hashes and rules", () => {
    // `current`: the same hash, no write when nothing changed, a change pending on another account.
    let data = serverData("current.json");
    const key = accountKey(data);
    expect(accountCheck(A, key, null).hash).toBe(HASHES.a);
    expect(accountCheck(B, key, null, "claudeCode").hash).toBe(HASHES.b);
    let now = T0 + 120_000;
    let accounts = new CliAccounts(data, { harness: "claudeCode", now: () => new Date(now) });
    expect(accounts.state()).toEqual(JSON.parse(read(join(FIXTURES, "current.json"))));
    expect(accounts.observe(A, key)).toEqual({ hash: HASHES.a, changed: false });
    expect(read(fileOf(data, "claudeCode"))).toBe(read(join(FIXTURES, "current.json")));
    expect(accounts.observe(B, key)).toEqual({ hash: HASHES.b, changed: true });
    rmSync(data, { recursive: true });

    // `pending`: refused, stale confirmation refused, the shown one confirmed.
    data = serverData("pending.json");
    accounts = new CliAccounts(data, { harness: "claudeCode" });
    expect(accounts.pending()).toBe(HASHES.b);
    expect(accountProblem(data)).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", message: "Claude Code is signed in to another account than the one this server's sessions ran on", details: { harness: "claudeCode", accountHash: HASHES.b }, fix: "a person at the server confirms the new account with `finisherd provider account confirm claudeCode`" });
    expect(instanceAccount(data, B, { newSession: true }).refused).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED" });
    expect(instanceAccount(data, B, { newSession: false }).refused).toBeNull();
    expect(thrown(() => accounts.expectNext({ kind: "console", id: "p" }))).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED", details: { reason: "changeWaiting", accountHash: HASHES.b } });
    expect(thrown(() => accounts.confirm(HASHES.a))).toMatchObject({ code: "ACCOUNT_CONFIRM_STALE", details: { harness: "claudeCode", shown: HASHES.a, accountHash: HASHES.b } });
    expect(accounts.confirm(HASHES.b)).toEqual({ confirmed: true, accountHash: HASHES.b, previous: HASHES.a });
    const pendingSeen = (JSON.parse(read(join(FIXTURES, "pending.json"))) as { pending: unknown }).pending;
    expect(read(fileOf(data, "claudeCode"))).toBe(`${JSON.stringify({ v: 1, current: pendingSeen })}\n`);
    rmSync(data, { recursive: true });

    // `expectNext`: taken within its time (audited under the person who expected it), not after.
    data = serverData("expect-next.json");
    const audited: unknown[] = [];
    registerAccountAudit(data, (by, d, harness) => audited.push({ by, d, harness }));
    now = T0 + 60_000 + EXPECT_TTL_MS - 1;
    accounts = new CliAccounts(data, { harness: "claudeCode", now: () => new Date(now) });
    expect(accounts.observe(B, key)).toEqual({ hash: HASHES.b, changed: false });
    expect(audited).toEqual([{ by: { kind: "console", id: "person-1" }, d: { accountHash: HASHES.b, previousHash: HASHES.a }, harness: "claudeCode" }]);
    rmSync(data, { recursive: true });
    data = serverData("expect-next.json");
    now = T0 + 60_000 + EXPECT_TTL_MS + 1;
    accounts = new CliAccounts(data, { harness: "claudeCode", now: () => new Date(now) });
    expect(accounts.observe(B, key)).toEqual({ hash: HASHES.b, changed: true });
    rmSync(data, { recursive: true });

    // And the lifted code writes the pre-lift bytes for the same steps.
    data = serverData();
    now = T0;
    accounts = new CliAccounts(data, { harness: "claudeCode", now: () => new Date(now) });
    accounts.observe(A, key);
    expect(read(fileOf(data, "claudeCode"))).toBe(read(join(FIXTURES, "current.json")));
    now += 60_000;
    accounts.observe(B, key);
    expect(read(fileOf(data, "claudeCode"))).toBe(read(join(FIXTURES, "pending.json")));
    rmSync(data, { recursive: true });
    data = serverData("current.json");
    now = T0 + 60_000;
    new CliAccounts(data, { harness: "claudeCode", now: () => new Date(now) }).expectNext({ kind: "console", id: "person-1" });
    expect(read(fileOf(data, "claudeCode"))).toBe(read(join(FIXTURES, "expect-next.json")));
  });
});

const FAKE_CODEX = fileURLToPath(new URL(`../../../target/debug/fake-codex${process.platform === "win32" ? ".exe" : ""}`, import.meta.url));
const NO_FAKE_CODEX = !existsSync(FAKE_CODEX) && process.env.FIN_REQUIRE_SERVER !== "1";
const SAVED_ENV = { FIN_RESOURCES_DIR: process.env.FIN_RESOURCES_DIR, FIN_TEST_HOOKS: process.env.FIN_TEST_HOOKS, FIN_TEST_SANDBOX_READY: process.env.FIN_TEST_SANDBOX_READY };

/** `fake-codex` as the bundled Codex (`FIN_RESOURCES_DIR`, a checkout's), signed in to `account`. */
function installFakeCodex(data: string, account: Record<string, unknown>): void {
  const res = join(tmp, "resources");
  const dir = join(res, "codex", `${process.platform}-${process.arch}`);
  mkdirSync(dir, { recursive: true });
  const bin = join(dir, process.platform === "win32" ? "codex.exe" : "codex");
  copyFileSync(FAKE_CODEX, bin);
  chmodSync(bin, 0o755);
  writeFileSync(join(dir, "fake-codex.json"), JSON.stringify({ observed: join(tmp, "observed.json"), version: pinnedCodexVersion() }));
  const home = join(data, "providers", "codex", "home");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  writeFileSync(join(home, "auth.json"), JSON.stringify({ account, tokens: { access_token: "TOKEN" } }));
  writeFileSync(join(data, "settings.json"), JSON.stringify({ agents: { codex: { enabled: true } } }));
  process.env.FIN_RESOURCES_DIR = res;
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
  invalidateCodexDetection();
}

describe("confirming takes the harness", () => {
  let server: AgentServer | undefined;
  afterEach(async () => {
    await server?.stop();
    server = undefined;
    for (const [k, v] of Object.entries(SAVED_ENV)) {
      if (v === undefined) delete process.env[k];
      else process.env[k] = v;
    }
    invalidateCodexDetection();
  });

  const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };
  async function run(argv: string[], prompt: Prompt = NOT_A_TERMINAL): Promise<{ code: number; out: string; err: string }> {
    let out = "";
    let err = "";
    const code = await runConsole([...argv, "--server-data", server!.serverData], { prompt, output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) } });
    return { code, out, err };
  }

  test("confirm_takes_harness: over RPC, a Codex confirmation touches Codex's record only", async () => {
    const data = serverData("pending.json");
    const claudeBefore = read(fileOf(data, "claudeCode"));
    server = new AgentServer(data);
    await server.start();
    const owner = await server.console();
    const challenge = async () => ok<{ code: string }>(await owner.call("console.challenge", {})).code;
    // `next` for Codex: Codex's record expects, Claude Code's pending change still waits.
    expect(ok(await owner.call("agent.account.confirm", { harness: "codex", next: true, challenge: await challenge() }))).toEqual({ confirmed: false, expecting: true });
    expect(JSON.parse(read(fileOf(data, "codex")))).toMatchObject({ v: 1, expectBy: { kind: "console" } });
    expect(read(fileOf(data, "claudeCode"))).toBe(claudeBefore);
    // A Codex change confirmed by its hash; Claude Code's hash is no Codex account.
    const key = accountKey(data);
    const codex = new CliAccounts(data, { harness: "codex" });
    rmSync(fileOf(data, "codex"));
    codex.observe(A, key);
    const waiting = codex.observe(B, key).hash;
    const stale = await owner.call("agent.account.confirm", { harness: "codex", accountHash: HASHES.b, challenge: await challenge() });
    expect(stale.ok ? undefined : stale.error?.code).toBe("ACCOUNT_CONFIRM_STALE");
    expect(ok(await owner.call("agent.account.confirm", { harness: "codex", accountHash: waiting, challenge: await challenge() }))).toMatchObject({ confirmed: true, accountHash: waiting });
    expect(codex.pending()).toBeUndefined();
    expect(read(fileOf(data, "claudeCode"))).toBe(claudeBefore);
    // Claude Code's own confirmation still takes Claude Code's.
    expect(ok(await owner.call("agent.account.confirm", { harness: "claudeCode", accountHash: HASHES.b, challenge: await challenge() }))).toEqual({ confirmed: true, accountHash: HASHES.b, previous: HASHES.a });
    const unknown = await owner.call("agent.account.confirm", { harness: "gemini", next: true, challenge: await challenge() });
    expect(unknown.ok).toBe(false);
    const audit = ok<{ entries: Array<{ details: Record<string, unknown> }> }>(await owner.call("auth.audit.list", { kind: "settings.write" }));
    expect(audit.entries.filter((e) => e.details.rpc === "agent.account.confirm").map((e) => e.details.value).sort()).toEqual(["claudeCode", "codex", "codex.next"]);
  });

  test("confirm_takes_harness: at the console, `provider account confirm <harness>`", async () => {
    server = new AgentServer(serverData("pending.json"));
    await server.start();
    // Codex: nothing of Codex's waits, so nothing is asked (Claude Code's change is not Codex's).
    const p = person();
    const codex = await run(["provider", "account", "confirm", "codex"], p);
    expect(codex.code, codex.err).toBe(0);
    expect(codex.out).toContain("No Codex account change waits for a confirmation.");
    expect(p.asked).toEqual([]);
    expect(new CliAccounts(server.serverData, { harness: "claudeCode" }).pending()).toBe(HASHES.b);
    // Not a harness with an account: a usage error before anything connects.
    expect((await run(["provider", "account", "confirm", "gemini"], person())).code).toBe(2);
    expect((await run(["provider", "account", "confirm"], person())).code).toBe(2);
    // Not at a terminal: refused for either harness.
    expect((await run(["provider", "account", "confirm", "codex"])).code).toBe(7);
    // A Codex change waits (`fake-codex` signed in to another account than the recorded one):
    // the console shows and confirms Codex's, and Claude Code's file is untouched.
    if (NO_FAKE_CODEX) return;
    const data = server.serverData;
    const claudeBefore = read(fileOf(data, "claudeCode"));
    const key = accountKey(data);
    const was = new CliAccounts(data, { harness: "codex" }).observe({ signedIn: true, authMethod: "chatgpt", label: "a@example.invalid" }, key).hash;
    installFakeCodex(data, { type: "chatgpt", email: "b@example.invalid", planType: "pro" });
    const yes = person();
    const confirmed = await run(["provider", "account", "confirm", "codex"], yes);
    expect(confirmed.code, confirmed.err).toBe(0);
    const now = new CliAccounts(data, { harness: "codex" });
    expect(now.pending()).toBeUndefined();
    const current = (JSON.parse(read(fileOf(data, "codex"))) as { current: { hash: string } }).current.hash;
    expect(current, confirmed.out).not.toBe(was);
    expect(yes.shown.join("\n")).toContain(current);
    expect(confirmed.out).toContain(`Codex sessions now run on account ${current} (was ${was})`);
    expect(read(fileOf(data, "claudeCode"))).toBe(claudeBefore);
    expect(new CliAccounts(data, { harness: "claudeCode" }).pending()).toBe(HASHES.b);
  });
});

describe("the event gate", () => {
  const refusal = finError("PROVIDER_VERSION", "not isolated", { missing: "isolation" });

  test("gate_holds_until_check: nothing passes before the check, all of it after, in order", () => {
    const checked: string[] = [];
    const gate = eventGate<string>((first) => (checked.push(first), first === "bad" ? refusal : null));
    expect(gate.state).toBe("waiting");
    expect(gate.push("a")).toEqual({ kind: "hold" });
    expect(gate.push("b")).toEqual({ kind: "hold" });
    expect(gate.withdraw((x) => x === "b")).toBe(true);
    expect(gate.withdraw((x) => x === "z")).toBe(false);
    expect(gate.push("c")).toEqual({ kind: "hold" });
    expect(checked).toEqual([]);
    expect(gate.verify("init")).toEqual({ kind: "forward", lines: ["a", "c", "init"] });
    expect(gate.state).toBe("open");
    expect(gate.push("d")).toEqual({ kind: "forward", lines: ["d"] });
    // Every later checked item is checked again, and forwarded alone.
    expect(gate.verify("init2")).toEqual({ kind: "forward", lines: ["init2"] });
    expect(checked).toEqual(["init", "init2"]);
    expect(gate.verify("bad")).toEqual({ kind: "refuse", error: refusal, kill: true });
    expect(gate.push("e")).toEqual({ kind: "drop" });

    // A failing check releases nothing it held, and drops what follows.
    const g2 = eventGate<string>((first) => (first === "bad" ? refusal : null));
    g2.push("x");
    expect(g2.verify("bad")).toEqual({ kind: "refuse", error: refusal, kill: true });
    expect(g2.state).toBe("refused");
    expect(g2.push("y")).toEqual({ kind: "drop" });
    expect(g2.verify("ok")).toEqual({ kind: "drop" });

    // Discarded before any check: what was held never comes out.
    const g3 = eventGate<string>(() => null);
    g3.push("x");
    g3.discard();
    expect(g3.state).toBe("waiting");
    expect(g3.verify("init")).toEqual({ kind: "forward", lines: ["init"] });
    const g4 = eventGate<string>(() => null);
    g4.push("x");
    expect(g4.refuse(refusal)).toEqual({ kind: "refuse", error: refusal, kill: true });
    expect(g4.push("y")).toEqual({ kind: "drop" });
  });
});

describe("the idle-close process loop", () => {
  /** A process double: its close resolves when the test says. */
  function fakeProcess(n: number, log: string[]) {
    let closeDone!: (gone: boolean) => void;
    const closing = new Promise<boolean>((r) => (closeDone = r));
    let exited = false;
    const session: ProviderSession = {
      instance: "i",
      events: undefined as never,
      startTurn: () => Effect.sync(() => void log.push(`turn@${n}`)),
      steer: () => Effect.void,
      interrupt: () => Effect.void,
      respond: () => Effect.void,
      rollback: () => Effect.void,
      ref: () => null,
    };
    const instance: CliInstance = {
      session: () => session,
      busy: () => false,
      close: () => (log.push(`close@${n}`), closing),
      exited: () => exited,
      turnWritten: async () => true,
      notice: () => undefined,
    };
    return { instance, finish: (gone: boolean) => ((exited = gone), closeDone(gone)) };
  }

  test("idle_loop_one_process_at_a_time: the next process starts only once the closed one is gone", async () => {
    const log: string[] = [];
    const procs: ReturnType<typeof fakeProcess>[] = [];
    const sinks: EventSink[] = [];
    const loop = await Effect.runPromise(
      CliSession.open(
        {
          harness: "codex",
          instance: "01INSTANCE",
          ref: { kind: "codex", id: "thread-1" },
          idleCloseMs: () => 5,
          rollbackIdle: () => Effect.fail(finError("PROVIDER_UNAVAILABLE", "no process", {})),
          start: ({ resume, sink }) =>
            Effect.sync(() => {
              const p = fakeProcess(procs.length, log);
              log.push(`start@${procs.length}:${resume}`);
              procs.push(p);
              sinks.push(sink);
              return p.instance;
            }),
        },
        false,
      ),
    );
    const s = loop.session();
    await Effect.runPromise(s.startTurn({ text: "one" } as never));
    sinks[0]!.offer({ type: "turnCompleted", stopReason: "end" } as ProviderEvent);
    await new Promise((r) => setTimeout(r, 40));
    expect(loop.running()).toBe(false);
    expect(log).toEqual(["start@0:false", "turn@0", "close@0"]);
    // A turn while the closed process is still going waits for it: no second process beside it.
    const turn = Effect.runPromise(s.startTurn({ text: "two" } as never));
    await new Promise((r) => setTimeout(r, 20));
    expect(log).toEqual(["start@0:false", "turn@0", "close@0"]);
    procs[0]!.finish(true);
    await turn;
    expect(log).toEqual(["start@0:false", "turn@0", "close@0", "start@1:true", "turn@1"]);
    // A closed process still running after its close: the reopen is refused, not overlapped.
    sinks[1]!.offer({ type: "turnCompleted", stopReason: "end" } as ProviderEvent);
    await new Promise((r) => setTimeout(r, 40));
    procs[1]!.finish(false);
    const refused = await Effect.runPromiseExit(s.startTurn({ text: "three" } as never));
    expect(Exit.isFailure(refused)).toBe(true);
    expect(JSON.stringify(refused)).toContain("previousProcessAlive");
    expect(procs).toHaveLength(2);
    // The closed process's sink is inert: its late events are dropped.
    sinks[0]!.offer({ type: "notice", name: "late", details: {} } as ProviderEvent);
    await loop.close();
  });

  test("idle_loop_close_during_reopen: a turn waiting on the idle close starts nothing once the session closes", async () => {
    const log: string[] = [];
    const procs: ReturnType<typeof fakeProcess>[] = [];
    const sinks: EventSink[] = [];
    const loop = await Effect.runPromise(
      CliSession.open(
        {
          harness: "codex",
          instance: "01INSTANCE",
          ref: { kind: "codex", id: "thread-1" },
          idleCloseMs: () => 5,
          rollbackIdle: () => Effect.void,
          start: ({ resume, sink }) =>
            Effect.sync(() => {
              const p = fakeProcess(procs.length, log);
              log.push(`start@${procs.length}:${resume}`);
              procs.push(p);
              sinks.push(sink);
              return p.instance;
            }),
        },
        false,
      ),
    );
    const s = loop.session();
    await Effect.runPromise(s.startTurn({ text: "one" } as never));
    sinks[0]!.offer({ type: "turnCompleted", stopReason: "end" } as ProviderEvent);
    await new Promise((r) => setTimeout(r, 40));
    expect(log).toEqual(["start@0:false", "turn@0", "close@0"]);
    // The turn waits on the idle close; the session closes meanwhile; then the old process goes.
    const turn = Effect.runPromiseExit(s.startTurn({ text: "two" } as never));
    await new Promise((r) => setTimeout(r, 10));
    const closed = loop.close();
    procs[0]!.finish(true);
    await closed;
    const exit = await turn;
    expect(Exit.isFailure(exit)).toBe(true);
    expect(JSON.stringify(exit)).toContain("SESSION_INTERRUPTED");
    // No second process: or, had one started, it is closed before `close()` resolves.
    const starts = log.filter((l) => l.startsWith("start@")).length;
    const closes = log.filter((l) => l.startsWith("close@")).length;
    expect(starts).toBe(closes);
    expect(loop.running()).toBe(false);
  });

  test("idle_loop_close_during_start: a process still starting when the session closes is closed before close() resolves", async () => {
    const log: string[] = [];
    const procs: ReturnType<typeof fakeProcess>[] = [];
    const sinks: EventSink[] = [];
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const loop = await Effect.runPromise(
      CliSession.open(
        {
          harness: "codex",
          instance: "01INSTANCE",
          ref: { kind: "codex", id: "thread-1" },
          idleCloseMs: () => 5,
          rollbackIdle: () => Effect.void,
          start: ({ resume, sink }) =>
            Effect.promise(async () => {
              if (resume) await gate;
              const p = fakeProcess(procs.length, log);
              log.push(`start@${procs.length}:${resume}`);
              procs.push(p);
              sinks.push(sink);
              if (resume) p.finish(true);
              return p.instance;
            }),
        },
        false,
      ),
    );
    const s = loop.session();
    await Effect.runPromise(s.startTurn({ text: "one" } as never));
    sinks[0]!.offer({ type: "turnCompleted", stopReason: "end" } as ProviderEvent);
    await new Promise((r) => setTimeout(r, 40));
    procs[0]!.finish(true);
    // Two turns at once: one process starts for both, and the session closes while it starts.
    const turns = [Effect.runPromiseExit(s.startTurn({ text: "two" } as never)), Effect.runPromiseExit(s.startTurn({ text: "three" } as never))];
    await new Promise((r) => setTimeout(r, 10));
    const closed = loop.close();
    release();
    await closed;
    expect(log).toEqual(["start@0:false", "turn@0", "close@0", "start@1:true", "close@1"]);
    for (const exit of await Promise.all(turns)) expect(JSON.stringify(exit)).toContain("SESSION_INTERRUPTED");
    expect(loop.running()).toBe(false);
  });
});
