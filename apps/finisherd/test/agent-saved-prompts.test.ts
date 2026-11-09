// Saved prompts (sub-project 7B Task 10, Decision 15; spec §6.4): bundled, owner-defined and
// project-folder prompts expand into a session start and a first prompt, for people only.
// Sessions run on the scripted `fake` harness (a test-hooks build, `FIN_TEST_HOOKS=1`).
import { mkdirSync, mkdtempSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ListedSavedPrompt, Session } from "@finisher/contracts";
import { REGISTRY } from "@finisher/contracts/registry";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { RPC_ONLY } from "../src/agent/command-of.ts";
import type { FakeScript } from "../src/agent/fake.ts";
import type { AgentStore } from "../src/agent/store.ts";
import { BUNDLED_ROLES } from "../src/agent/roles.ts";
import { expand, loadPrompt } from "../src/agent/saved-prompts.ts";
import { runConsole } from "../src/console/main.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type Exit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;
const hooks = process.env.FIN_TEST_HOOKS;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
const SCRIPT: FakeScript = { name: "main", turns: [[{ emit: [msg("on it"), { type: "turnCompleted", stopReason: "endTurn" }] }]] };

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-saved-prompts-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData, { timers: false, fakeScripts: [SCRIPT] });
});
afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

async function setUp() {
  await server.start();
  const local = await server.local();
  const dir = join(tmp, "P");
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
  return { local, owner: await server.console(), dir, projectId };
}

const run = (c: NdjsonClient, projectId: string, name: string, params?: Record<string, unknown>, more: Record<string, unknown> = {}) =>
  c.call("agent.savedPrompt.run", { projectId, name, harness: "fake", ...(params === undefined ? {} : { params }), ...more });
const list = async (c: NdjsonClient, projectId?: string) =>
  ok<{ prompts: ListedSavedPrompt[]; ignored?: ListedSavedPrompt[]; invalid?: Array<{ source: string; name?: string; reason: string }>; projectPrompts?: string; promptsHash?: string }>(
    await c.call("agent.savedPrompt.list", projectId === undefined ? {} : { projectId }),
  );
const settings = (agents: unknown) => writeFileSync(join(server.serverData, "settings.json"), JSON.stringify({ agents }));
const prompt = (name: string, more: Record<string, unknown> = {}) => ({
  name,
  title: `Prompt ${name}`,
  role: "editor",
  mode: "draft",
  template: "Tighten the {{scope}} pass, tone {{tone}}, notes: {{notes}}",
  params: [
    { name: "scope", type: "enum", values: ["intro", "outro"], default: "intro" },
    { name: "tone", type: "enum", values: ["calm", "punchy"], default: "calm" },
    { name: "notes", type: "string", default: "none" },
  ],
  ...more,
});
const firstPrompt = (sessionId: string) => {
  const n = (server.agents.runtime as unknown as { store: AgentStore }).store.nodes(sessionId, 1, 50).find((x) => x.kind === "userMessage");
  return (n?.payload as { text?: string } | undefined)?.text;
};
const errOf = (e: Exit) => (e.ok ? undefined : e.error);

describe("saved prompts", () => {
  test("make_shorts_expands: count=5 starts a clip-finder session with the expanded first prompt", async () => {
    const { local, projectId } = await setUp();
    const r = ok<{ session: Session; run?: { id: string } }>(await run(local, projectId, "make-shorts", { count: 5 }));
    expect(r.session).toMatchObject({ role: "clip-finder", mode: "draft", trigger: "savedPrompt", title: "Make short clips" });
    expect(r.run).toBeDefined();
    expect(firstPrompt(r.session.id)).toContain("Find the 5 strongest short clips");
    expect(firstPrompt(r.session.id)).not.toContain("{{");
    // The default applies when the parameter is left out; the trigger is the session's, for good.
    const d = ok<{ session: Session }>(await run(local, projectId, "make-shorts"));
    expect(firstPrompt(d.session.id)).toContain("Find the 5 strongest");
    expect(ok<{ session: Session }>(await local.call("agent.session.get", { sessionId: d.session.id })).session.trigger).toBe("savedPrompt");
    // A session started any other way has none.
    const plain = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "fake" }));
    expect(plain.session.trigger).toBeUndefined();
    expect((await list(local)).prompts.find((p) => p.name === "make-shorts")).toMatchObject({ source: "bundle", role: "clip-finder", params: [{ name: "count", type: "int", min: 1, max: 20, default: 5 }] });
  });

  test("params_validated: a bad value is AGENT_ARGS naming the field", async () => {
    const { local, projectId } = await setUp();
    for (const [params, field, reason] of [
      [{ count: 0 }, "count", "outOfRange"],
      [{ count: 21 }, "count", "outOfRange"],
      [{ count: "x" }, "count", "notInt"],
      [{ count: "1.5" }, "count", "notInt"],
      [{ count: 3, bogus: 1 }, "bogus", "unknown"],
    ] as const) {
      const e = errOf(await run(local, projectId, "make-shorts", params));
      expect([e?.code, (e?.details as { field?: string })?.field, (e?.details as { reason?: string })?.reason], JSON.stringify(params)).toEqual(["AGENT_ARGS", field, reason]);
    }
    // Digits are an int (the console sends text).
    expect(ok<{ session: Session }>(await run(local, projectId, "make-shorts", { count: "7" })).session.id).toBeTruthy();
    // Unit: required, enum and string.
    const p = loadPrompt(prompt("t", { params: [{ name: "scope", type: "enum", values: ["a", "b"] }, { name: "notes", type: "string" }], template: "{{scope}}/{{notes}}" }), "test", BUNDLED_ROLES);
    expect(() => expand(p, { notes: "x" })).toThrow(expect.objectContaining({ code: "AGENT_ARGS", details: expect.objectContaining({ field: "scope", reason: "required" }) }));
    expect(() => expand(p, { scope: "c", notes: "x" })).toThrow(expect.objectContaining({ details: expect.objectContaining({ field: "scope", reason: "notInValues" }) }));
    expect(() => expand(p, { scope: "a", notes: "x".repeat(2001) })).toThrow(expect.objectContaining({ details: expect.objectContaining({ field: "notes", reason: "tooLong" }) }));
    // One pass: a value holding a slot is not expanded again.
    expect(expand(p, { scope: "a", notes: "{{scope}}" })).toBe("a/{{scope}}");
  });

  test("template_has_no_expressions: a {{ that is not a declared name is refused at load", () => {
    const load = (template: string, params = [{ name: "count", type: "int" }]) => () => loadPrompt(prompt("t", { template, params }), "test", BUNDLED_ROLES);
    expect(load("Find {{count}} clips")).not.toThrow();
    for (const bad of ["{{count + 1}}", "{{other}}", "{{ count }}", "{{count|upper}}", "{{#if count}}x{{/if}}", "{{count", "a {{ b"]) {
      expect(load(bad), bad).toThrow(expect.objectContaining({ code: "INVALID_REQUEST", details: expect.objectContaining({ field: "template" }) }));
    }
    // The other load checks: role, mode, parameter shapes.
    const bad = (more: Record<string, unknown>) => () => loadPrompt(prompt("t", { template: "x", params: [], ...more }), "test", BUNDLED_ROLES);
    expect(bad({ role: "nobody" })).toThrow(expect.objectContaining({ details: expect.objectContaining({ field: "role" }) }));
    expect(bad({ role: "critic", mode: "auto" })).toThrow(expect.objectContaining({ details: expect.objectContaining({ field: "mode" }) }));
    expect(bad({ params: [{ name: "n", type: "int", min: 5, max: 1 }] })).toThrow();
    expect(bad({ params: [{ name: "n", type: "int", default: "x" }] })).toThrow();
    expect(bad({ params: [{ name: "n", type: "enum" }] })).toThrow();
    expect(bad({ params: [{ name: "n", type: "string" }, { name: "n", type: "string" }] })).toThrow();
    expect(bad({ extra: true })).toThrow();
  });

  test("owner_prompts_from_settings: listed, run, one bad prompt reported, owner over bundle", async () => {
    const { local, projectId } = await setUp();
    settings({ savedPrompts: [prompt("tighten"), prompt("broken", { template: "{{nope}}" }), { ...prompt("make-shorts"), template: "My own shorts: {{scope}}" }] });
    const l = await list(local, projectId);
    expect(l.prompts.map((p) => [p.name, p.source])).toEqual([["make-shorts", "settings"], ["tighten", "settings"]]);
    expect(l.invalid).toEqual([expect.objectContaining({ name: "broken", source: expect.stringContaining("savedPrompts[1]") })]);
    const r = ok<{ session: Session }>(await run(local, projectId, "tighten", { scope: "outro", tone: "punchy", notes: "keep the logo" }));
    expect(firstPrompt(r.session.id)).toBe("Tighten the outro pass, tone punchy, notes: keep the logo");
    expect(r.session.role).toBe("editor");
    expect(firstPrompt(ok<{ session: Session }>(await run(local, projectId, "make-shorts")).session.id)).toBe("My own shorts: intro");
  });

  test("project_folder_prompts_load_only_when_trusted", async () => {
    const { local, owner, dir, projectId } = await setUp();
    const write = (name: string, p: unknown) => {
      mkdirSync(join(dir, "prompts"), { recursive: true });
      writeFileSync(join(dir, "prompts", `${name}.json`), JSON.stringify(p));
    };
    expect((await list(local, projectId)).projectPrompts).toBe("none");
    write("outro", prompt("outro"));
    // Untrusted: listed for a person to read, but not runnable.
    const before = await list(local, projectId);
    expect(before).toMatchObject({ projectPrompts: "untrusted", promptsHash: expect.stringMatching(/^sha256:/) });
    expect(before.prompts.map((p) => p.name)).toEqual(["make-shorts"]);
    expect(before.ignored?.map((p) => p.name)).toEqual(["outro"]);
    expect(errOf(await run(local, projectId, "outro"))?.code).toBe("SAVED_PROMPT_UNKNOWN");
    // Trusting the rules alone leaves the prompt files untrusted.
    const rules = ok<{ rulesHash: string }>(await owner.call("agent.rules.list", { projectId })).rulesHash;
    ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rules }));
    expect((await list(local, projectId)).projectPrompts).toBe("untrusted");
    // A person trusts what they saw; a different hash is refused and changes nothing.
    const stale = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rules, expectedPromptsHash: `sha256:${"0".repeat(64)}` });
    expect(errOf(stale)).toMatchObject({ code: "INVALID_REQUEST", details: expect.objectContaining({ field: "expectedPromptsHash", reason: "changed" }) });
    expect((await list(local, projectId)).projectPrompts).toBe("untrusted");
    const trusted = ok<{ promptsHash: string }>(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rules, expectedPromptsHash: before.promptsHash }));
    expect(trusted.promptsHash).toBe(before.promptsHash);
    const after = await list(local, projectId);
    expect(after.projectPrompts).toBe("trusted");
    expect(after.prompts.find((p) => p.name === "outro")).toMatchObject({ source: "project" });
    expect(ok<{ session: Session }>(await run(local, projectId, "outro", { scope: "outro" })).session.trigger).toBe("savedPrompt");
    // A project prompt may shadow the bundle's, but never an owner's.
    write("make-shorts", { ...prompt("make-shorts"), template: "Project shorts {{scope}}" });
    ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rules, expectedPromptsHash: (await list(local, projectId)).promptsHash }));
    expect((await list(local, projectId)).prompts.find((p) => p.name === "make-shorts")?.source).toBe("project");
    settings({ savedPrompts: [{ ...prompt("make-shorts"), template: "Owner shorts {{scope}}" }] });
    expect((await list(local, projectId)).prompts.find((p) => p.name === "make-shorts")?.source).toBe("settings");
    expect(firstPrompt(ok<{ session: Session }>(await run(local, projectId, "make-shorts")).session.id)).toBe("Owner shorts intro");
    // The file changes (an agent could edit it): every project prompt stops until trusted again.
    write("outro", { ...prompt("outro"), template: "Different {{scope}}" });
    const changed = await list(local, projectId);
    expect(changed.projectPrompts).toBe("changed");
    expect(changed.prompts.every((p) => p.source !== "project")).toBe(true);
    expect(errOf(await run(local, projectId, "outro"))?.code).toBe("SAVED_PROMPT_UNKNOWN");
    // A file taken away is a change too; an invalid file is reported and cannot be trusted.
    unlinkSync(join(dir, "prompts", "outro.json"));
    unlinkSync(join(dir, "prompts", "make-shorts.json"));
    expect((await list(local, projectId)).projectPrompts).toBe("changed");
    write("bad", { ...prompt("bad"), template: "{{nope}}" });
    expect((await list(local, projectId)).invalid).toEqual([expect.objectContaining({ source: "prompts/bad.json" })]);
    const e = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: rules, expectedPromptsHash: (await list(local, projectId)).promptsHash });
    expect(errOf(e)?.code).toBe("INVALID_REQUEST");
  });

  test("people_only: an agent principal is SCOPE_DENIED; fin has no command for it", async () => {
    const { local, projectId } = await setUp();
    const opened = await open(server.core, local, projectId, { harness: "fake" });
    const a = await server.agent(opened.credentialPath);
    expect(code(await run(a, projectId, "make-shorts"))).toBe("SCOPE_DENIED");
    expect(REGISTRY.commands.flatMap((c) => c.rpcs.map((r) => r.method)).filter((m) => m.startsWith("agent.savedPrompt."))).toEqual([]);
    expect(RPC_ONLY.has("agent.savedPrompt.run")).toBe(true);
    expect(RPC_ONLY.has("agent.savedPrompt.list")).toBe(true);
  });

  test("unknown_is_saved_prompt_unknown: over RPC and as the console's exit 3", async () => {
    const { local, dir, projectId } = await setUp();
    const e = errOf(await run(local, projectId, "nope"));
    expect(e).toMatchObject({ code: "SAVED_PROMPT_UNKNOWN", details: { name: "nope", prompts: ["make-shorts"] } });
    let err = "";
    const argv = (...a: string[]) => runConsole([...a, "--server-data", server.serverData], { output: { out: () => undefined, err: (t: string) => void (err += `${t}\n`) } } as never);
    expect(await argv("agent", "run", "nope", "--project", dir)).toBe(3);
    expect(err).toContain("SAVED_PROMPT_UNKNOWN");
    void projectId;
  });

  test("the console runs and lists prompts: finisherd agent run <name> --param k=v", async () => {
    const { dir } = await setUp();
    let out = "";
    let err = "";
    const argv = (...a: string[]) => runConsole([...a, "--server-data", server.serverData], { output: { out: (t: string) => void (out += `${t}\n`), err: (t: string) => void (err += `${t}\n`) } } as never);
    const code0 = await argv("agent", "run", "make-shorts", "--project", dir, "--param", "count=3", "--harness", "fake");
    expect([code0, err]).toEqual([0, ""]);
    expect(out).toContain("Started agent session");
    expect(await argv("agent", "run", "make-shorts", "--project", dir, "--param", "count=0", "--harness", "fake")).toBe(2);
    expect(err).toContain("AGENT_ARGS");
    expect(await argv("agent", "run", "make-shorts", "--project", dir, "--param", "count", "--harness", "fake")).toBe(2);
    out = "";
    expect(await argv("agent", "prompts", "--project", dir)).toBe(0);
    expect(out).toContain("make-shorts");
  });
});
