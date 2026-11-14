// Session events and context delivery (7F plan Task 6; Decisions 1, 16, 18): `UserPrompt` once
// per person's prompt (before anything is opened at a start; never for a child's brief),
// `SessionStart` awaited for the first run's context, `SessionEnd` once, the context of events
// without a prompt steered into a running turn or held for the next run, and `Notification` and
// `Approval` from the runtime engine's tap, whether or not anyone watches. A real runtime over
// the `fake` harness (as `agent-runtime.test.ts`), with a real hook engine running the fake hook.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, HookEvent, Node, Principal, Session, WatchItem } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Fiber, Stream } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { FakeAdapter, type FakeScript } from "../src/agent/fake.ts";
import { AgentStore } from "../src/agent/store.ts";
import { type ControlStore, openControlStore } from "../src/control/db.ts";
import type { AuditInput } from "../src/control/audit.ts";
import { HookConfig } from "../src/hooks/config.ts";
import { buildHooks, type HooksWiring } from "../src/hooks/wiring.ts";
import { makeRuntime, META, PEOPLE } from "./support/runtime-doubles.ts";

const FAKE = join(import.meta.dirname, "fixtures", "hooks", "fake-hook.mjs");
const DIRECT = () => ({ ...sandboxStatus(), hook: { kind: "disabled" as const, reason: "profileNotReady" as const } });

let tmp: string;
let serverData: string;
let control: ControlStore;
let clock: ReturnType<typeof manualClock>;
let store: AgentStore;
let audits: AuditInput[];
let logs: string[];
const cleanups: (() => Promise<void>)[] = [];

beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "hooks-session-")));
  serverData = join(tmp, "server-data");
  mkdirSync(join(tmp, "P"), { recursive: true });
  mkdirSync(serverData, { recursive: true });
  mkdirSync(join(tmp, "control"), { recursive: true });
  copyFileSync(FAKE, join(serverData, "fake-hook.mjs"));
  clock = manualClock(Date.now());
  control = openControlStore(join(tmp, "control"), { serverId: "sid", checkpoint: false });
  store = new AgentStore(control, join(tmp, "control"), clock, systemIds(clock));
  audits = [];
  logs = [];
});

afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
  await control.close();
  rmSync(tmp, { recursive: true, force: true, maxRetries: 3 });
});

const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
const failure = (e: Effect.Effect<unknown, FinError>) => Effect.runPromise(Effect.flip(e));

async function until<A>(f: () => A | undefined | false, what: string, ms = 10_000): Promise<A> {
  const t0 = Date.now();
  for (;;) {
    const v = f();
    if (v !== undefined && v !== false) return v;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 10));
  }
}

/** The fake hook in the user layer (`<server-data>/settings.json`). */
const hook = (id: string, event: HookEvent, args: readonly string[], more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs", ...args], file: "fake-hook.mjs", ...more });
/** Appends each request it gets to `<tmp>/<name>.log`. */
const rec = (name: string) => ["--append", join(tmp, `${name}.log`)];
const requests = (name: string): Array<{ event: HookEvent; session?: { id: string }; payload: Record<string, unknown> }> => {
  const p = join(tmp, `${name}.log`);
  return existsSync(p) ? readFileSync(p, "utf8").split("\n").filter((l) => l !== "").map((l) => JSON.parse(l)) : [];
};

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
const done = { type: "turnCompleted", stopReason: "endTurn" };
const usage = (input: number, costMicros?: number) => ({ type: "usage", input, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0, ...(costMicros === undefined ? {} : { costMicros }) });
/** Every turn answers and ends. */
const ENDS: FakeScript = { name: "ends", turns: [[{ emit: [msg("ok"), done] }]] };
/** A run that goes on until interrupted (the session stays `running`). */
const WORKING: FakeScript = { name: "working", turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }]] };

interface Hooked extends ReturnType<typeof makeRuntime> {
  readonly adapter: ReturnType<typeof FakeAdapter>;
  readonly wiring: HooksWiring;
}

function hooked(scripts: readonly FakeScript[], hooks: unknown[], opts: Parameters<typeof makeRuntime>[2] = {}): Hooked {
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks }));
  const config = new HookConfig({ serverData, trustOf: () => undefined, dirOf: (id) => (id === "P1" ? join(tmp, "P") : undefined) });
  const wiring = buildHooks({ config, serverData, audit: { append: (a) => void audits.push(a) }, log: (m) => void logs.push(m), options: { status: DIRECT } });
  const adapter = FakeAdapter(scripts);
  const h = makeRuntime({ store, control }, adapter, { clock, hooks: wiring.session, sessionEvents: wiring.session, ...opts });
  cleanups.push(() => wiring.close());
  cleanups.push(() => h.runtime.close());
  return { ...h, adapter, wiring };
}

const kinds = (id: string) => store.nodes(id).map((n) => n.kind);
const payload = (n: Node) => n.payload as Record<string, unknown>;
const status = (id: string) => store.get(id)!.status;
const block = (texts: string[]) => `\n\n<hook-context>\n${texts.join("\n\n")}\n</hook-context>`;
const AGENT: Principal = { kind: "agent", id: "a1", scopes: [], projects: "*", branches: "*", human: false };

function watch(h: Hooked, sessionId: string) {
  const items: WatchItem[] = [];
  const fiber = Effect.runFork(Stream.runForEach(h.runtime.watch(sessionId), (i) => Effect.sync(() => void items.push(i))));
  cleanups.push(() => Effect.runPromise(Fiber.interrupt(fiber)));
  return items;
}

describe("UserPrompt (Decision 16)", () => {
  test("user_prompt_block_refuses_prompt: HOOK_BLOCKED, no run, no node, one hook.block row", async () => {
    const h = hooked([ENDS], [hook("no-prompts", "UserPrompt", ["--reply", "block:not today"])]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    const e = await failure(h.runtime.prompt(session.id, "trim it", PEOPLE));
    expect(e).toMatchObject({ code: "HOOK_BLOCKED", details: { hook: "no-prompts", reason: "not today", event: "UserPrompt" } });
    expect(store.runs(session.id)).toEqual([]);
    expect(store.nodes(session.id)).toEqual([]);
    expect(h.adapter.log.turns).toEqual([]);
    expect(audits.filter((a) => a.kind === "hook.block")).toMatchObject([{ principal: { kind: "local", id: "local" }, details: { event: "UserPrompt", hookId: "no-prompts", session: session.id } }]);
  });

  test("user_prompt_block_on_start_creates_nothing: no session, no branch, no credential", async () => {
    const h = hooked([ENDS], [hook("no-prompts", "UserPrompt", ["--reply", "block:not today"])]);
    await h.runtime.boot();
    const e = await failure(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    expect(e).toMatchObject({ code: "HOOK_BLOCKED", details: { hook: "no-prompts", reason: "not today" } });
    expect(store.list().sessions).toEqual([]);
    expect(h.registry.created).toEqual([]);
    expect(h.core.auth.list().filter((c) => c.kind === "agent")).toEqual([]);
    expect(h.adapter.log.opened).toEqual([]);
    // A start without a prompt fires no UserPrompt: it opens.
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    expect(session.status).toBe("idle");
  });

  test("user_prompt_on_steer_and_saved_prompt: once per prompt, with its origin", async () => {
    const h = hooked([WORKING], [hook("seen", "UserPrompt", rec("up"))]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "from a saved prompt" }, PEOPLE, META, undefined, "savedPrompt"));
    await until(() => status(session.id) === "running" && h.adapter.log.turns.length === 1, "running");
    expect(await run(h.runtime.steer(session.id, "and the outro", PEOPLE))).toEqual({ queued: false });
    await until(() => h.adapter.log.steers.length === 1, "the steer");
    expect(h.adapter.log.steers[0]!.text).toBe("and the outro");
    expect(requests("up").map((r) => [r.payload, r.session?.id])).toEqual([
      [{ text: "from a saved prompt", origin: "start" }, undefined],
      [{ text: "and the outro", origin: "steer" }, session.id],
    ]);
  });

  test("child_brief_fires_no_user_prompt: the parent's prompt fires it, the brief does not; both sessions start", async () => {
    const h = hooked([WORKING, ENDS], [hook("seen", "UserPrompt", rec("up")), hook("started", "SessionStart", rec("start"))]);
    await h.runtime.boot();
    const { session: parent } = await run(h.runtime.start({ projectId: "P1", prompt: "lead" }, PEOPLE, META));
    await until(() => status(parent.id) === "running", "the parent runs");
    const { session: child } = await run(h.runtime.spawnChild(store.get(parent.id)!, { projectId: "P1", role: "fixer", prompt: "the brief" }, AGENT, META));
    await until(() => h.adapter.log.turns.some((t) => t.text === "the brief"), "the brief's turn");
    expect(requests("up").map((r) => r.payload.text)).toEqual(["lead"]);
    expect(requests("start").map((r) => r.session?.id)).toEqual([parent.id, child.id]);
    expect(requests("start")[1]!.payload).toEqual({ parent: parent.id });
  });

  test("user_prompt_context_reaches_provider_and_notice: the input ends with the block, the node keeps the typed text", async () => {
    const h = hooked([ENDS], [hook("style", "UserPrompt", ["--reply", "context:use the style guide"])]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    await run(h.runtime.prompt(session.id, "tighten the intro", PEOPLE));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "the run");
    expect(h.adapter.log.turns[0]!.text).toBe(`tighten the intro${block(["use the style guide"])}`);
    const nodes = store.nodes(session.id);
    expect(nodes.slice(0, 2).map((n) => n.kind)).toEqual(["notice", "userMessage"]);
    expect(payload(nodes[0]!)).toEqual({ hookContext: { event: "UserPrompt", hooks: ["style"], text: "use the style guide" } });
    expect(payload(nodes[1]!)).toMatchObject({ text: "tighten the intro", trigger: "prompt" });
  });
});

describe("command order and live slots (Task 6 review)", () => {
  /** Holds every `UserPrompt` whose request holds `GATED` until `<tmp>/open` exists. */
  const gated = () => hook("gate", "UserPrompt", ["--gate-if", "GATED", join(tmp, "open")]);
  const open = () => writeFileSync(join(tmp, "open"), "");

  test("slow_user_prompt_keeps_command_order: a later steer waits for an earlier one's hook", async () => {
    const h = hooked([WORKING], [gated()]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => status(session.id) === "running" && h.adapter.log.turns.length === 1, "running");
    const first = run(h.runtime.steer(session.id, "first GATED", PEOPLE));
    const second = run(h.runtime.steer(session.id, "second", PEOPLE));
    await new Promise((r) => setTimeout(r, 500));
    expect(h.adapter.log.steers).toEqual([]);
    open();
    await Promise.all([first, second]);
    expect(h.adapter.log.steers.map((s) => s.text)).toEqual(["first GATED", "second"]);
  });

  test("end_cancels_a_pending_prompt: the end waits for the running hook, then the prompt never runs", async () => {
    const h = hooked([ENDS], [gated()]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    const prompted = failure(h.runtime.prompt(session.id, "trim GATED", PEOPLE));
    // The prompt's hook is under way (gated) when the end comes.
    await new Promise((r) => setTimeout(r, 200));
    const ended = run(h.runtime.end(session.id, PEOPLE));
    await new Promise((r) => setTimeout(r, 300));
    expect(status(session.id)).toBe("idle");
    open();
    expect(await prompted).toMatchObject({ code: "SESSION_ENDED" });
    expect((await ended).status).toBe("ended");
    expect(h.adapter.log.turns).toEqual([]);
    expect(store.runs(session.id)).toEqual([]);
  });

  test("interrupt_does_not_wait_for_a_prompt_hook: it completes while the hook is gated; the steer never runs", async () => {
    const h = hooked([WORKING], [gated()]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => status(session.id) === "running" && h.adapter.log.turns.length === 1, "running");
    const steered = failure(h.runtime.steer(session.id, "late GATED", PEOPLE));
    await new Promise((r) => setTimeout(r, 200));
    const interrupted = await Promise.race([run(h.runtime.interrupt(session.id, PEOPLE)), new Promise((r) => setTimeout(() => r("still waiting"), 3_000))]);
    expect(interrupted).toMatchObject({ status: "interrupted" });
    expect(status(session.id)).toBe("interrupted");
    open();
    expect(await steered).toMatchObject({ code: "SESSION_INTERRUPTED" });
    await new Promise((r) => setTimeout(r, 200));
    expect(h.adapter.log.steers).toEqual([]);
    expect(store.nodes(session.id).filter((n) => n.kind === "userMessage").map((n) => payload(n).text)).toEqual(["go"]);
  });

  test("approval_answer_does_not_wait_for_a_prompt_hook: a budget card settles while the hook is gated", async () => {
    const OVER: FakeScript = { name: "over", turns: [[{ emit: [usage(1200, 0)] }, { awaitAnswer: "never" }]] };
    const h = hooked([OVER, ENDS], [gated()]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => status(session.id) === "interrupted" && store.approvals(session.id).length === 1, "the budget card");
    const card = store.approvals(session.id)[0]!;
    const prompted = run(h.runtime.prompt(session.id, "more GATED", PEOPLE));
    await new Promise((r) => setTimeout(r, 200));
    const answered = await Promise.race([run(h.runtime.approve(card.id, "allow", { tokens: 5000 }, PEOPLE)), new Promise((r) => setTimeout(() => r("still waiting"), 3_000))]);
    expect(answered).toMatchObject({ approval: { id: card.id, status: "answered" } });
    expect(store.approval(card.id)!.status).not.toBe("open");
    open();
    await prompted;
  });

  test("steer_to_a_run_waiting_for_a_slot_keeps_its_context: delivered with the steer at admission", async () => {
    const h = hooked([WORKING], [hook("audio", "UserPrompt", ["--reply", "context:preserve the audio"])], { limits: { maxLiveBuiltin: 1 } });
    await h.runtime.boot();
    const { session: a } = await run(h.runtime.start({ projectId: "P1", prompt: "busy" }, PEOPLE, META));
    await until(() => status(a.id) === "running" && h.adapter.log.turns.length === 1, "a runs");
    const { session: b, run: rb } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.nodes(b.id).some((n) => n.kind === "notice" && payload(n).code === "SESSION_QUEUED"), "b queued");
    expect(await run(h.runtime.steer(b.id, "trim", PEOPLE))).toEqual({ queued: true });
    await run(h.runtime.interrupt(a.id, PEOPLE));
    await until(() => h.adapter.log.steers.length === 2, "b's steers");
    const ctx = "<hook-context>\npreserve the audio\n</hook-context>";
    expect(h.adapter.log.turns[1]!.text).toBe(`go\n\n${ctx}`);
    expect(h.adapter.log.steers.map((s) => s.text)).toEqual(["trim", ctx]);
    const steered = store.nodes(b.id).filter((n) => n.kind === "userMessage" && n.runId === rb!.id).map((n) => payload(n));
    expect(steered).toMatchObject([{ text: "go", trigger: "prompt" }, { text: "trim", trigger: "steer" }, { text: ctx, trigger: "hook", hookContext: { event: "UserPrompt", hooks: ["audio"] } }]);
  });
  test("interrupted_queued_run_hands_back_steers_not_context: the steer comes back, the context waits for the next run", async () => {
    const h = hooked([WORKING], [hook("audio", "UserPrompt", ["--reply", "context:preserve the audio"])], { limits: { maxLiveBuiltin: 1 } });
    await h.runtime.boot();
    const { session: a } = await run(h.runtime.start({ projectId: "P1", prompt: "busy" }, PEOPLE, META));
    await until(() => status(a.id) === "running" && h.adapter.log.turns.length === 1, "a runs");
    const { session: b, run: rb } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.nodes(b.id).some((n) => n.kind === "notice" && payload(n).code === "SESSION_QUEUED"), "b queued");
    expect(await run(h.runtime.steer(b.id, "trim", PEOPLE))).toEqual({ queued: true });
    await run(h.runtime.interrupt(b.id, PEOPLE));
    const returned = await until(() => store.nodes(b.id).find((n) => n.runId === rb!.id && n.kind === "notice" && payload(n).steerReturned !== undefined), "b's steers handed back");
    expect((payload(returned).steerReturned as { steers: Array<{ text: string }> }).steers.map((x) => x.text)).toEqual(["trim"]);
    await run(h.runtime.interrupt(a.id, PEOPLE));
    // The resume's prompt is the handed-back steer; its UserPrompt hook runs on it, after the held context.
    await run(h.runtime.resume(b.id, PEOPLE, undefined, true));
    await until(() => h.adapter.log.turns.length === 2, "b's next run");
    expect(h.adapter.log.turns[1]!.text).toBe(`trim${block(["preserve the audio", "preserve the audio"])}`);
    expect(h.adapter.log.steers).toEqual([]);
  });
});

describe("hook context a provider hands back (7F with 7D Decision 13)", () => {
  const CTX = "preserve the audio";
  const UP = () => [hook("audio", "UserPrompt", ["--reply", `context:${CTX}`])];
  /** A Claude-like provider: steers held, handed back at an interrupt (the turn's tail). */
  const HOLDING: FakeScript = { name: "holding", holdSteers: true, turns: [[{ emit: [msg("working")] }, { awaitAnswer: "never" }], [{ emit: [msg("again")] }, { awaitAnswer: "never" }]] };
  const texts = (id: string) => store.nodes(id).filter((n) => n.kind === "notice" && payload(n).steerReturned !== undefined).map((n) => (payload(n).steerReturned as { steers: Array<{ text: string }> }).steers.map((x) => x.text));

  test("tail_hand_back_keeps_context_out_of_resend: the steer comes back, the context waits for the next run", async () => {
    const h = hooked([HOLDING], UP());
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => h.adapter.log.turns.length === 1, "the run");
    expect(await run(h.runtime.steer(session.id, "trim", PEOPLE))).toEqual({ queued: false });
    await until(() => h.adapter.log.steers.length === 2, "the steer and its context");
    await run(h.runtime.interrupt(session.id, PEOPLE));
    await until(() => texts(session.id).length === 1, "handed back");
    expect(texts(session.id)).toEqual([["trim"]]);
    await run(h.runtime.resume(session.id, PEOPLE, undefined, true));
    await until(() => h.adapter.log.turns.length === 2, "the resend");
    expect(h.adapter.log.turns[1]!.text).toBe(`trim${block([CTX, CTX])}`);
  });

  test("in_run_hand_back_holds_context_for_next_prompt: only the steer in the notice; the context joins the next run", async () => {
    const script: FakeScript = { name: "returns", holdSteers: true, turns: [[{ emit: [msg("working")] }, { returnHeld: 1000 }], [{ emit: [msg("ok"), done] }]] };
    const h = hooked([script], UP());
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => h.adapter.log.turns.length === 1, "the run");
    await run(h.runtime.steer(session.id, "trim", PEOPLE));
    await until(() => status(session.id) === "idle", "the run's end");
    expect(texts(session.id)).toEqual([["trim"]]);
    await run(h.runtime.prompt(session.id, "next", PEOPLE));
    await until(() => h.adapter.log.turns.length === 2, "the next run");
    expect(h.adapter.log.turns[1]!.text).toBe(`next${block([CTX, CTX])}`);
  });

  test("context_only_notice_keeps_earlier_resend: a later hand-back of context alone leaves the person's steer to resend", async () => {
    const script: FakeScript = { name: "twice", holdSteers: true, turns: [[{ emit: [msg("working")] }, { returnHeld: 800 }, { returnHeld: 800 }, { awaitAnswer: "never" }], [{ emit: [msg("again")] }, { awaitAnswer: "never" }]] };
    const h = hooked([script], UP());
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => h.adapter.log.turns.length === 1, "the run");
    await run(h.runtime.steer(session.id, "trim", PEOPLE));
    await until(() => texts(session.id).length === 1, "the first hand-back");
    h.wiring.session.deliver({ projectId: session.projectId, session: store.get(session.id)!, principal: AGENT }, "Notification", { ran: 1, context: "CI is red", contextHooks: ["ci"] });
    const notices = () => store.nodes(session.id).filter((n) => n.kind === "notice" && (payload(n).steerReturned !== undefined || payload(n).hookContextHeld !== undefined));
    await until(() => notices().length === 2, "the second hand-back");
    expect(Object.keys(payload(notices()[1]!))).toEqual(["hookContextHeld"]);
    await run(h.runtime.interrupt(session.id, PEOPLE));
    await run(h.runtime.resume(session.id, PEOPLE, undefined, true));
    await until(() => h.adapter.log.turns.length === 2, "the resend");
    expect(h.adapter.log.turns[1]!.text).toBe(`trim${block([CTX, "CI is red", CTX])}`);
  });

  test("hand_back_after_admission_keeps_context_out: steered in at admission, handed back at the interrupt", async () => {
    const h = hooked([HOLDING], UP(), { limits: { maxLiveBuiltin: 1 } });
    await h.runtime.boot();
    const { session: a } = await run(h.runtime.start({ projectId: "P1", prompt: "busy" }, PEOPLE, META));
    await until(() => status(a.id) === "running" && h.adapter.log.turns.length === 1, "a runs");
    const { session: b } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => store.nodes(b.id).some((n) => n.kind === "notice" && payload(n).code === "SESSION_QUEUED"), "b queued");
    expect(await run(h.runtime.steer(b.id, "trim", PEOPLE))).toEqual({ queued: true });
    await run(h.runtime.interrupt(a.id, PEOPLE));
    await until(() => h.adapter.log.steers.length === 2, "b's steers at admission");
    await run(h.runtime.interrupt(b.id, PEOPLE));
    await until(() => texts(b.id).length === 1, "handed back");
    expect(texts(b.id)).toEqual([["trim"]]);
    await run(h.runtime.resume(b.id, PEOPLE, undefined, true));
    await until(() => h.adapter.log.turns.length === 3, "b's resend");
    expect(h.adapter.log.turns[2]!.text).toBe(`trim${block([CTX, CTX])}`);
  });
});

describe("SessionStart and SessionEnd (Decisions 1, 16)", () => {
  test("session_start_context_joins_first_run: held without a prompt, delivered with the first", async () => {
    const h = hooked([ENDS], [hook("intro", "SessionStart", ["--reply", "context:the project uses British spelling"])]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    expect(store.nodes(session.id)).toEqual([]);
    expect(h.adapter.log.turns).toEqual([]);
    await run(h.runtime.prompt(session.id, "go", PEOPLE));
    await until(() => h.adapter.log.turns.length === 1, "the run");
    expect(h.adapter.log.turns[0]!.text).toBe(`go${block(["the project uses British spelling"])}`);
    expect(payload(store.nodes(session.id)[0]!)).toEqual({ hookContext: { event: "SessionStart", hooks: ["intro"], text: "the project uses British spelling" } });
  });

  test("delayed_session_start_then_one_user_prompt: the start's context, then the prompt's; UserPrompt ran once", async () => {
    const h = hooked([ENDS], [hook("slow", "SessionStart", ["--sleep", "1000", "--reply", "context:S"]), hook("up", "UserPrompt", [...rec("up"), "--reply", "context:U"])]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => h.adapter.log.turns.length === 1, "the run");
    expect(h.adapter.log.turns[0]!.text).toBe(`go${block(["S", "U"])}`);
    expect(kinds(session.id).slice(0, 3)).toEqual(["notice", "notice", "userMessage"]);
    expect(requests("up")).toHaveLength(1);
  });

  test("session_start_block_is_ignored: the session starts; an ignoredBlock notice in its first run", async () => {
    const h = hooked([ENDS], [hook("gate", "SessionStart", ["--reply", "block:no"])]);
    await h.runtime.boot();
    const { session, run: r } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    expect(r).toBeDefined();
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "the run");
    expect(h.adapter.log.turns[0]!.text).toBe("go");
    expect(payload(store.nodes(session.id)[0]!)).toEqual({ hook: { event: "SessionStart", id: "gate", layer: "user", ignoredBlock: "no" } });
    expect(audits).toEqual([]);
  });

  test("session_end_observed_once: at an end and at a delete", async () => {
    const h = hooked([ENDS], [hook("bye", "SessionEnd", rec("end"))]);
    await h.runtime.boot();
    const { session: a } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    await run(h.runtime.end(a.id, PEOPLE));
    const { session: b } = await run(h.runtime.start({ projectId: "P1" }, PEOPLE, META));
    await run(h.runtime.delete(b.id, PEOPLE));
    await until(() => requests("end").length === 2, "two ends");
    await h.wiring.engine.settled();
    expect(requests("end").map((r) => [r.session?.id, r.payload])).toEqual([
      [a.id, { reason: "userEnded" }],
      [b.id, { reason: "deleted" }],
    ]);
  });
});

describe("context delivery (Decision 16)", () => {
  const at = (s: Session) => ({ projectId: s.projectId, session: s, principal: AGENT });

  test("context_port_steers_running_session: a userMessage {trigger: hook} in the run, steered to the provider", async () => {
    const h = hooked([WORKING], []);
    await h.runtime.boot();
    const { session, run: r } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => status(session.id) === "running" && h.adapter.log.turns.length === 1, "running");
    h.wiring.session.deliver(at(session), "PostCommand", { ran: 2, context: "lint: 3 warnings", contextHooks: ["lint"], blocked: { hook: "guard", reason: "frozen", layer: "user" } });
    await until(() => h.adapter.log.steers.length === 2, "two steers");
    expect(h.adapter.log.steers.map((s) => s.text)).toEqual(["<hook-context>\nlint: 3 warnings\n</hook-context>", "<hook-context>\nThe PostCommand hook guard blocked this: frozen\n</hook-context>"]);
    const steered = store.nodes(session.id).filter((n) => n.kind === "userMessage" && payload(n).trigger === "hook");
    expect(steered.map((n) => [n.runId, payload(n).hookContext])).toEqual([
      [r!.id, { event: "PostCommand", hooks: ["lint"] }],
      [r!.id, { event: "PostCommand", hooks: ["guard"] }],
    ]);
    expect(h.runtime.stats().live).toBe(1);
    expect(store.runs(session.id)).toHaveLength(1);
  });

  test("context_port_holds_when_idle: no run starts; delivered with the next prompt", async () => {
    const h = hooked([ENDS], []);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "first" }, PEOPLE, META));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "idle");
    h.wiring.session.deliver(at(session), "Notification", { ran: 1, context: "CI is red", contextHooks: ["ci"] });
    await new Promise((r) => setTimeout(r, 100));
    expect(store.runs(session.id)).toHaveLength(1);
    expect(status(session.id)).toBe("idle");
    await run(h.runtime.prompt(session.id, "second", PEOPLE));
    await until(() => h.adapter.log.turns.length === 2, "the second run");
    expect(h.adapter.log.turns[1]!.text).toBe(`second${block(["CI is red"])}`);
  });
});

describe("Notification and Approval from the runtime's tap (Decision 18)", () => {
  /** Turn 1 crosses 80 % of the token budget, turn 2 of the money budget. */
  const CROSSING: FakeScript = { name: "crossing", turns: [[{ emit: [usage(850, 0), done] }], [{ emit: [usage(10, 850), done] }]] };
  const BUDGET = { tokens: 1000, moneyMicros: 1000 };

  test("budget_80_percent_fires_notification_unwatched: once per kind, match budget.*", async () => {
    const h = hooked([CROSSING], [hook("spend", "Notification", rec("note"), { match: "budget.*" })]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "one", budget: BUDGET }, PEOPLE, META));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "turn 1");
    await run(h.runtime.prompt(session.id, "two", PEOPLE));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 2, "turn 2");
    await until(() => requests("note").length === 2, "two notifications");
    await h.wiring.engine.settled();
    expect(requests("note").map((r) => r.payload)).toEqual([
      { kind: "budget", budget: "tokens", used: 850, limit: 1000, at: 0.8 },
      { kind: "budget", budget: "money", used: 850, limit: 1000, at: 0.8 },
    ]);
  });

  test("tail_usage_crossing_fires_notification_unwatched: a replaced instance's late usage crosses 80 %", async () => {
    const crashed: FakeScript = { name: "crashed", turns: [[{ late: [usage(850, 0)] }, { crash: { code: null, signal: "SIGKILL" } }]] };
    const h = hooked([crashed, WORKING], [hook("spend", "Notification", rec("note"), { match: "budget.*" })]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "one", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => status(session.id) === "interrupted", "the crash");
    await run(h.runtime.resume(session.id, PEOPLE, "again"));
    await until(() => store.get(session.id)!.usage.input === 850, "the tail charged");
    await until(() => requests("note").length === 1, "the notification");
    await h.wiring.engine.settled();
    expect(requests("note").map((r) => r.payload)).toEqual([{ kind: "budget", budget: "tokens", used: 850, limit: 1000, at: 0.8 }]);
  });

  test("attaching_a_watcher_changes_no_hook_runs: each status change and budget notice runs its hook once", async () => {
    const h = hooked([CROSSING], [hook("all", "Notification", rec("note"))]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "one", budget: BUDGET }, PEOPLE, META));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "turn 1");
    const items = watch(h, session.id);
    await run(h.runtime.prompt(session.id, "two", PEOPLE));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 2, "turn 2");
    await until(() => items.some((i) => i.type === "statusChanged" && i.status === "idle"), "the watcher saw it");
    await until(() => requests("note").length === 7, "seven notifications");
    await new Promise((r) => setTimeout(r, 200));
    await h.wiring.engine.settled();
    const seen = requests("note").map((r) => (r.payload.kind === "budget" ? `budget.${r.payload.budget}` : `status.${r.payload.status}`));
    expect(seen).toEqual(["status.idle", "status.running", "budget.tokens", "status.idle", "status.running", "budget.money", "status.idle"]);
  });

  test("status_change_fires_notification_unwatched: match status.running", async () => {
    const h = hooked([ENDS], [hook("busy", "Notification", rec("note"), { match: "status.running" })]);
    await h.runtime.boot();
    const { session, run: r } = await run(h.runtime.start({ projectId: "P1", prompt: "go" }, PEOPLE, META));
    await until(() => status(session.id) === "idle" && h.adapter.log.turns.length === 1, "the run");
    await until(() => requests("note").length === 1, "the notification");
    await h.wiring.engine.settled();
    expect(requests("note")).toMatchObject([{ event: "Notification", session: { id: session.id }, payload: { kind: "status", status: "running", runId: r!.id } }]);
  });

  test("approval_card_fires_approval: a budget card, match by its kind", async () => {
    const OVER: FakeScript = { name: "over", turns: [[{ emit: [usage(1200, 0)] }, { awaitAnswer: "never" }]] };
    const h = hooked([OVER], [hook("cards", "Approval", rec("card"), { match: "budget" }), hook("other", "Approval", rec("other"), { match: "rule" })]);
    await h.runtime.boot();
    const { session } = await run(h.runtime.start({ projectId: "P1", prompt: "go", budget: { tokens: 1000 } }, PEOPLE, META));
    await until(() => status(session.id) === "interrupted", "the budget stop");
    await until(() => requests("card").length === 1, "the card's hook");
    await h.wiring.engine.settled();
    const card = store.approvals(session.id)[0]!;
    expect(requests("card")).toMatchObject([{ event: "Approval", session: { id: session.id }, payload: { approval: { id: card.id, kind: "budget", subject: { kind: "tokens" } } } }]);
    expect(requests("other")).toEqual([]);
  });
});

test("runtime_files_within_size: runtime.ts and runtime-engine.ts take call-site lines only", () => {
  const lines = (f: string) => readFileSync(join(import.meta.dirname, "..", "src", "agent", f), "utf8").split("\n").length - 1;
  expect(lines("runtime.ts")).toBeLessThanOrEqual(595);
  expect(lines("runtime-engine.ts")).toBeLessThanOrEqual(580);
});
