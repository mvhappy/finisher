// A parent resumes a child it interrupted, and only then (plan 7B Decision 16 as amended by the
// owner decision 2026-11-13); `agent.session.resume {resendReturned}` (plan 7D Decision 13).
// Against a real server whose sessions run on the scripted `fake` harness (a test-hooks build run
// with `FIN_TEST_HOOKS=1`); each agent calls with its session's own credential.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { CredentialFile, Run, Session } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { credentialPath } from "../src/agent/credentials.ts";
import type { FakeAdapter, FakeScript } from "../src/agent/fake.ts";
import { AgentServer, code } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit as Answer, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let serverData: string;
let server: AgentServer | undefined;
const hooks = process.env.FIN_TEST_HOOKS;

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-child-resume-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  writeFileSync(join(serverData, "settings.json"), "{}");
});
afterEach(async () => {
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

let n = 0;
const id = () => `01R${String(++n).padStart(23, "0")}`;
const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
const usage = (input: number, output: number) => ({ type: "usage", input, output, cacheRead: 0, cacheWrite: 0, reasoning: 0 });
/** Runs that never end by themselves (a parent's, a working child's, every resumed one's). */
const WORKING: FakeScript = { name: "working", turns: [0, 1, 2, 3].map((k) => [{ emit: [msg(`working ${k}`)] }, { awaitAnswer: "never" }]) };
const returned = (...texts: string[]) => ({ type: "notice", name: "steerReturned", details: { harness: "fake", steers: texts.map((text) => ({ text })) } });
/** A brief that hands steers back twice (`notice {steerReturned}`: the last one counts) and goes on. */
const RETURNS: FakeScript = { name: "returns", turns: [[{ emit: [returned("stale"), returned("first", "second"), msg("working")] }, { awaitAnswer: "never" }]] };
/** A brief that spends past a small budget. */
const OVER: FakeScript = { name: "over", turns: [[{ emit: [usage(800, 400), msg("spent")] }, { awaitAnswer: "never" }]] };
/** A brief whose process exits. */
const EXITS: FakeScript = { name: "exits", turns: [[{ emit: [msg("bye")] }, { crash: { code: 1, signal: null } }]] };

async function boot(scripts: readonly FakeScript[], agents?: Record<string, unknown>) {
  if (agents !== undefined) writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents }));
  server = new AgentServer(serverData, { timers: false, fakeScripts: scripts });
  await server.start();
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx(id(), 0, [addAsset()])));
  return { local, projectId };
}

const srv = () => server!;
const get = (sessionId: string) => srv().core.store.get(sessionId)!;
const fake = () => srv().agents.adapters.get("fake") as unknown as ReturnType<typeof FakeAdapter>;
const clientOf = (sessionId: string) => srv().agent(credentialPath(srv().core.store.root, sessionId));
const details = (e: Answer) => (e.ok ? undefined : (e.error.details as Record<string, unknown>));

async function until<A>(f: () => A | undefined | false, what: string, ms = 8000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** A running parent (started by a person) and its client. */
async function parentOf(local: NdjsonClient, projectId: string) {
  const { session } = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "go" }));
  await until(() => get(session.id).status === "running", "the parent runs");
  return { session, client: await clientOf(session.id) };
}

/** A child `by` spawns, running its brief. */
async function spawn(by: NdjsonClient, projectId: string, extra: Record<string, unknown> = {}): Promise<Session> {
  const child = ok<{ session: Session }>(await by.call("agent.session.start", { projectId, role: "fixer", prompt: "brief", ...extra })).session;
  await until(() => srv().core.store.runs(child.id).length === 1, "the brief's run");
  return child;
}

const interrupted = (sessionId: string) => until(() => get(sessionId).status === "interrupted" && get(sessionId), `${sessionId} interrupted`);

function expectDenied(e: Answer, reason: string, what: string) {
  expect(code(e), what).toBe("SCOPE_DENIED");
  expect(details(e), what).toMatchObject({ reason });
}

describe("a parent resumes a child it interrupted (7B Decision 16 amendment)", () => {
  test("the parent's interrupt is recorded as its own; it resumes with resendReturned, then with a prompt", async () => {
    const { local, projectId } = await boot([WORKING, RETURNS, WORKING]);
    const p = await parentOf(local, projectId);
    const c = await spawn(p.client, projectId);
    await until(() => srv().core.store.nodes(c.id).filter((x) => x.kind === "notice" && "steerReturned" in (x.payload as object)).length === 2, "the notices");
    ok(await p.client.call("agent.session.interrupt", { sessionId: c.id }));
    expect(await interrupted(c.id)).toMatchObject({ statusReason: "parentInterrupt" });
    // The texts come from the notice: nobody copies them.
    const resumed = ok<{ session: Session; run?: Run }>(await p.client.call("agent.session.resume", { sessionId: c.id, resendReturned: true }));
    expect(resumed.run?.trigger).toBe("prompt");
    await until(() => fake().log.turns.at(-1)?.text === "first\n\nsecond", "the resent texts");
    const prompts = srv().core.store.nodes(c.id).filter((x) => x.kind === "userMessage" && x.runId === resumed.run!.id);
    expect(prompts.map((x) => (x.payload as { text: string }).text)).toEqual(["first\n\nsecond"]);
    // Interrupted again by the parent: a resume with a prompt.
    ok(await p.client.call("agent.session.interrupt", { sessionId: c.id }));
    await interrupted(c.id);
    const again = ok<{ session: Session; run?: Run }>(await p.client.call("agent.session.resume", { sessionId: c.id, prompt: "go on" }));
    expect(again.run).toBeDefined();
    await until(() => fake().log.turns.at(-1)?.text === "go on", "the prompt");
    // The last run handed nothing back: resendReturned has nothing to send.
    ok(await p.client.call("agent.session.interrupt", { sessionId: c.id }));
    await interrupted(c.id);
    const none = await p.client.call("agent.session.resume", { sessionId: c.id, resendReturned: true });
    expect(code(none)).toBe("INVALID_REQUEST");
    expect(details(none)).toMatchObject({ reason: "noSteerReturned" });
    expect(get(c.id).status).toBe("interrupted");
    // Both at once is ambiguous.
    const both = await p.client.call("agent.session.resume", { sessionId: c.id, prompt: "x", resendReturned: true });
    expect(details(both)).toMatchObject({ reason: "promptAndResend" });
  });

  test("any other cause needs a person: a person's interrupt, a budget stop, a provider exit", async () => {
    const { local, projectId } = await boot([WORKING, WORKING, OVER, EXITS, WORKING]);
    const p = await parentOf(local, projectId);
    const byPerson = await spawn(p.client, projectId);
    ok(await local.call("agent.session.interrupt", { sessionId: byPerson.id }));
    expect(await interrupted(byPerson.id)).toMatchObject({ statusReason: "userInterrupt" });
    const budget = await spawn(p.client, projectId, { budget: { tokens: 500 } });
    expect((await interrupted(budget.id)).statusReason).toBe("BUDGET_EXCEEDED");
    const exited = await spawn(p.client, projectId);
    const exitReason = (await interrupted(exited.id)).statusReason;
    expect(exitReason).toMatch(/PROVIDER_EXITED|providerExit/);
    for (const [child, what] of [
      [byPerson, "a person's interrupt"],
      [budget, "a budget stop"],
      [exited, "a provider exit"],
    ] as const) {
      expectDenied(await p.client.call("agent.session.resume", { sessionId: child.id }), "interruptedByOther", what);
      // The parent's own interrupt afterwards changes nothing: the first cause stays.
      ok(await p.client.call("agent.session.interrupt", { sessionId: child.id }));
      expectDenied(await p.client.call("agent.session.resume", { sessionId: child.id, prompt: "x" }), "interruptedByOther", `${what}, then the parent's`);
      expect(get(child.id).status, what).toBe("interrupted");
    }
    // A person resumes any of them.
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: byPerson.id })).session.status).toBe("idle");
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: exited.id })).session.status).toBe("idle");
  });

  test("a finisherd crash needs a person too", async () => {
    const { local, projectId } = await boot([WORKING, WORKING]);
    const p = await parentOf(local, projectId);
    const c = await spawn(p.client, projectId);
    await srv().stop();
    server = new AgentServer(serverData, { timers: false, fakeScripts: [WORKING] });
    await server.start();
    expect(get(c.id)).toMatchObject({ status: "interrupted", statusReason: "serverRestart" });
    const parent = await clientOf(p.session.id);
    expectDenied(await parent.call("agent.session.resume", { sessionId: c.id }), "interruptedByOther", "a crash");
    expect(ok<{ session: Session }>(await (await srv().local()).call("agent.session.resume", { sessionId: c.id })).session.status).toBe("idle");
  });

  test("a sibling, a grandparent, the child itself and an agent of no session are refused", async () => {
    const { local, projectId } = await boot([WORKING]);
    const p = await parentOf(local, projectId);
    // A clip-finder spawns clip-finders (a fixer none).
    const c1 = await spawn(p.client, projectId, { role: "clip-finder" });
    const c2 = await spawn(p.client, projectId);
    const c1Client = await clientOf(c1.id);
    const g = await spawn(c1Client, projectId, { role: "clip-finder" });
    ok(await c1Client.call("agent.session.interrupt", { sessionId: g.id }));
    ok(await p.client.call("agent.session.interrupt", { sessionId: c2.id }));
    expect(await interrupted(g.id)).toMatchObject({ statusReason: "parentInterrupt" });
    await interrupted(c2.id);
    const c2Client = await clientOf(c2.id);
    for (const [who, target, what] of [
      [p.client, g.id, "a grandparent"],
      [c2Client, g.id, "an uncle"],
      [c1Client, c2.id, "a sibling"],
      [c2Client, c2.id, "itself"],
      [p.client, "01NOSUCHSESSION00000000000", "a missing one"],
    ] as const) {
      expectDenied(await who.call("agent.session.resume", { sessionId: target }), "notOwnChild", what);
    }
    const owner = await srv().console();
    const minted = ok<{ file: CredentialFile }>(await confirmed(owner, "auth.credential.mint", { kind: "agent", label: "hand-made", scopes: ["agent.run", "project.read"], projects: [projectId] }));
    const { client, exit } = await srv().proveAs(minted.file);
    ok(exit);
    expectDenied(await client.call("agent.session.resume", { sessionId: g.id }), "noSession", "no session's");
    // Its own parent does; a person may resume anything in reach.
    expect(ok<{ session: Session }>(await c1Client.call("agent.session.resume", { sessionId: g.id })).session.status).toBe("idle");
    expect(ok<{ session: Session }>(await local.call("agent.session.resume", { sessionId: c2.id })).session.status).toBe("idle");
  });

  test("the runtime checks again under the session's lock (a cause that changed after stage 4)", async () => {
    const { local, projectId } = await boot([WORKING]);
    const p = await parentOf(local, projectId);
    const c = await spawn(p.client, projectId);
    ok(await local.call("agent.session.interrupt", { sessionId: c.id }));
    await interrupted(c.id);
    const asParent = { kind: "agent", id: p.session.id, scopes: ["agent.run"], projects: [projectId], branches: "*", human: false } as const;
    const exit = await Effect.runPromiseExit(srv().agents.runtime.resume(c.id, asParent as never));
    expect(Exit.isFailure(exit) && JSON.stringify(exit.cause)).toMatch(/SCOPE_DENIED.*interruptedByOther/);
    expect(get(c.id).status).toBe("interrupted");
  });
});

describe("a queued run's steers are handed back too (review of the hand-back)", () => {
  /** The `steerReturned` notices of `sessionId`'s last run. */
  const returnedIn = (sessionId: string) => {
    const run = srv().core.store.runs(sessionId).at(-1)!.id;
    return srv().core.store.nodes(sessionId).filter((x) => x.runId === run && x.kind === "notice" && "steerReturned" in (x.payload as object)).map((x) => (x.payload as { steerReturned: unknown }).steerReturned);
  };
  const steerNodes = (sessionId: string) => srv().core.store.nodes(sessionId).filter((x) => x.kind === "userMessage" && (x.payload as { trigger?: string }).trigger === "steer");
  const expectBack = (sessionId: string, texts: string[]) => {
    const nodes = steerNodes(sessionId);
    expect(returnedIn(sessionId)).toEqual([{ harness: "fake", steers: texts.map((text, i) => ({ text, nodeId: nodes[i]!.id, at: nodes[i]!.createdAt })) }]);
  };

  test("interrupted before admission: the held steers come back in a notice, and resendReturned sends them", async () => {
    // One slot: the parent's run holds it, so its child's brief waits.
    const { local, projectId } = await boot([WORKING], { maxLiveBuiltin: 1 });
    const p = await parentOf(local, projectId);
    const c = ok<{ session: Session }>(await p.client.call("agent.session.start", { projectId, role: "fixer", prompt: "brief" })).session;
    await until(() => srv().core.store.nodes(c.id).some((x) => x.kind === "notice"), "queued");
    expect(ok<{ queued: boolean }>(await p.client.call("agent.session.steer", { sessionId: c.id, text: "one" }))).toEqual({ queued: true });
    expect(ok<{ queued: boolean }>(await p.client.call("agent.session.steer", { sessionId: c.id, text: "two" }))).toEqual({ queued: true });
    ok(await p.client.call("agent.session.interrupt", { sessionId: c.id }));
    expect(await interrupted(c.id)).toMatchObject({ statusReason: "parentInterrupt" });
    await until(() => returnedIn(c.id).length > 0, "the hand-back");
    expectBack(c.id, ["one", "two"]);
    const resumed = ok<{ session: Session; run?: Run }>(await p.client.call("agent.session.resume", { sessionId: c.id, resendReturned: true }));
    const prompts = srv().core.store.nodes(c.id).filter((x) => x.kind === "userMessage" && x.runId === resumed.run!.id);
    expect(prompts.map((x) => (x.payload as { text: string }).text)).toEqual(["one\n\ntwo"]);
  });

  test("ended before admission: the held steer comes back in the ended run", async () => {
    const { local, projectId } = await boot([WORKING], { maxLiveBuiltin: 1 });
    await parentOf(local, projectId);
    const b = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job b" })).session;
    await until(() => srv().core.store.nodes(b.id).some((x) => x.kind === "notice"), "queued");
    expect(ok<{ queued: boolean }>(await local.call("agent.session.steer", { sessionId: b.id, text: "faster" }))).toEqual({ queued: true });
    ok(await local.call("agent.session.end", { sessionId: b.id }));
    await until(() => returnedIn(b.id).length > 0, "the hand-back");
    expectBack(b.id, ["faster"]);
  });

  test("a queued run whose turn cannot start once admitted: its held steer comes back", async () => {
    // The parent holds the one slot; the queued session's process cannot start when it is admitted.
    const { local, projectId } = await boot([WORKING, { name: "broken", spawnFails: true, turns: [] }], { maxLiveBuiltin: 1 });
    const a = await parentOf(local, projectId);
    const b = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job b" })).session;
    await until(() => srv().core.store.nodes(b.id).some((x) => x.kind === "notice"), "queued");
    ok(await local.call("agent.session.steer", { sessionId: b.id, text: "later" }));
    ok(await local.call("agent.session.end", { sessionId: a.session.id }));
    await interrupted(b.id);
    await until(() => returnedIn(b.id).length > 0, "the hand-back");
    expectBack(b.id, ["later"]);
  });

  test("an admitted run's held steers are steered in, never handed back", async () => {
    const { local, projectId } = await boot([WORKING], { maxLiveBuiltin: 1 });
    const a = await parentOf(local, projectId);
    const b = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake", prompt: "job b" })).session;
    await until(() => srv().core.store.nodes(b.id).some((x) => x.kind === "notice"), "queued");
    ok(await local.call("agent.session.steer", { sessionId: b.id, text: "go on" }));
    ok(await local.call("agent.session.end", { sessionId: a.session.id }));
    await until(() => fake().log.steers.some((x) => x.text === "go on"), "the steer");
    expect(returnedIn(b.id)).toEqual([]);
  });
});
