// Plugin commands through a whole server in this process (sp8b Task 9 review): `main.ts`'s
// `serve`, so the real `HandlersLive`, `PluginCommandsTag`, plugin host, job engine, chain and
// agent runtime. The test-only probe (`fixtures/plugins/probe`) is installed and enabled by
// writing its rows into the running server's `control.db` (the registry reads them on each
// ask), as `@test/probe` (a registry plugin) and `@dev/probe` (a developer plugin, Decision 19).
// Then a `local` client and an agent session's credential run its commands.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Rule } from "@finisher/contracts";
import type { PluginCommandList, PluginCommandRunResult } from "@finisher/core-types";
import { serverIdentity } from "@finisher/fin-node";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { openControlStore } from "../src/control/db.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { AgentServer, code, open } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, insert, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ARGS = {
  type: "object",
  properties: { op: { type: "string", enum: ["command"] }, out: { type: "string", maxLength: 100000 } },
  required: ["op"],
  additionalProperties: false,
};
const COMMANDS = [
  { name: "edit", title: "cmd.edit", args: ARGS, writes: true },
  { name: "look", title: "cmd.look", args: ARGS },
];
const txOut = (ops: unknown[]): string => JSON.stringify({ kind: "tx", ops });

let dir: string;
let server: AgentServer;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "plugin-cmds-server-"));
  const serverData = join(dir, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(dir, { recursive: true, force: true });
});

/** Installs the probe as `id` (`source`) in the running server's store and control.db, enabled with read and write grants. */
async function installProbe(id: string, source: "registry" | "dev"): Promise<void> {
  const sd = server.serverData;
  const rel = `store/${id}/1.0.0-t`;
  const full = join(sd, "plugins", ...rel.split("/"));
  mkdirSync(dirname(full), { recursive: true });
  cpSync(FIXTURE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, id, messages: "messages", locales: ["en"], contributes: { commands: COMMANDS } }));
  mkdirSync(join(full, "messages"));
  writeFileSync(join(full, "messages", "en.json"), JSON.stringify({ "cmd.edit": "Edit", "cmd.look": "Look" }));
  const control = openControlStore(sd, { serverId: serverIdentity(sd).id, checkpoint: false });
  try {
    const tables = new PluginTables(control);
    // A developer plugin runs from its own folder: its `dir` is that folder's real path (Task 11).
    tables.addPlugin({ id, version: "1.0.0", source, treeHash: "t", dir: source === "dev" ? realpathSync(full) : rel, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
    tables.setGrants(id, "1.0.0", [JSON.stringify({ perm: "project:read" }), JSON.stringify({ perm: "project:write" })], "console:console", 0);
  } finally {
    await control.close();
  }
}

async function setUp() {
  await server.start();
  await installProbe("@test/probe", "registry");
  await installProbe("@dev/probe", "dev");
  const local = await server.local();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(dir, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01T00000000000000000000001", 0, [addAsset()])));
  ok(await commit(local, projectId, tx("01T00000000000000000000002", 1, [insert(0, 5)])));
  return { local, projectId };
}

const run = (c: NdjsonClient, projectId: string, name: string, out: string, more: Record<string, unknown> = {}) =>
  c.call("plugin.command.run", { projectId, name, args: { op: "command", out }, ...more });

describe("plugin commands on a server", () => {
  test("handlers_live_runs_and_commits: the real handlers list, run, dry-run and commit with via", async () => {
    const { local, projectId } = await setUp();
    const list = ok<PluginCommandList>(await local.call("plugin.commands", {}));
    // A person sees the developer plugin's commands too.
    expect(list.commands.map((c) => c.name)).toEqual(["plugin:@dev/probe/edit", "plugin:@dev/probe/look", "plugin:@test/probe/edit", "plugin:@test/probe/look"]);
    const data = ok<PluginCommandRunResult>(await run(local, projectId, "plugin:@test/probe/look", '{"kind":"data","value":{"n":1}}'));
    expect(data).toEqual({ outcome: "data", data: { n: 1 } });
    const ops = [insert(5, 5)];
    const dry = ok<PluginCommandRunResult>(await run(local, projectId, "plugin:@test/probe/edit", txOut(ops), { dryRun: true }));
    expect(dry).toMatchObject({ outcome: "data", dryRun: true, rev: 2 });
    const done = ok<PluginCommandRunResult>(await run(local, projectId, "plugin:@test/probe/edit", txOut(ops)));
    expect(done).toMatchObject({ outcome: "accepted", rev: 3 });
    const got = ok<{ tx: { via?: unknown; author: string } }>(await local.call("tx.get", { projectId, txId: done.txId }));
    expect(got.tx.via).toEqual({ plugin: "@test/probe@1.0.0", command: "edit" });
    expect(code(await run(local, projectId, "plugin:@test/probe/nope", "{}"))).toBe("PLUGIN_NOT_FOUND");
  });

  test("agent_session_runs_on_its_own_branch: session-root reach, dev plugins hidden, rules see the plugin command", async () => {
    const { local, projectId } = await setUp();
    const opened = await open(server.core, local, projectId);
    const root = opened.session.rootBranch!;
    const a = await server.agent(opened.credentialPath);
    // Developer plugins are never listed to, nor run by, an agent (Decision 19); the list needs
    // only `project.read`, which every session holds.
    const list = ok<PluginCommandList>(await a.call("plugin.commands", {}));
    expect(list.commands.map((c) => c.name)).toEqual(["plugin:@test/probe/edit", "plugin:@test/probe/look"]);
    expect(code(await run(a, projectId, "plugin:@dev/probe/look", '{"kind":"data","value":1}'))).toBe("PLUGIN_NOT_FOUND");
    // On its own branch the command commits; on `main` (outside its subtree) it is refused.
    const ops = [insert(5, 5)];
    const mine = ok<PluginCommandRunResult>(await run(a, projectId, "plugin:@test/probe/edit", txOut(ops), { branch: root }));
    expect(mine.outcome).toBe("accepted");
    const refused = code(await run(a, projectId, "plugin:@test/probe/edit", txOut(ops), { branch: "main" }));
    expect(["BRANCH_NOT_ALLOWED", "PROJECT_NOT_FOUND"]).toContain(refused);

    // A rule that asks before `tx.commit`: the host's dry run is a dry run to the policy (no card),
    // the real commit asks a person.
    const owner = await server.console();
    ok<{ rule: Rule }>(await confirmed(owner, "agent.rules.add", { effect: "ask", command: "rpc:tx.commit", sessionId: opened.session.id }));
    server.core.store.startRun(opened.session.id, { trigger: "prompt" });
    const before = server.core.store.approvals(opened.session.id).length;
    const dry = ok<PluginCommandRunResult>(await run(a, projectId, "plugin:@test/probe/edit", txOut([insert(10, 5)]), { branch: root, dryRun: true }));
    expect(dry).toMatchObject({ outcome: "data", dryRun: true });
    expect(server.core.store.approvals(opened.session.id).length).toBe(before);
    expect(code(await run(a, projectId, "plugin:@test/probe/edit", txOut([insert(10, 5)]), { branch: root }))).toBe("APPROVAL_PENDING");
    // The card names the plugin command (from the host's `via`), not just the RPC.
    const card = server.core.store.approvals(opened.session.id).at(-1)!;
    expect((card.subject as { command: string }).command).toBe("rpc:tx.commit");
    // A deny rule on the plugin's commands holds at its commit too.
    ok(await confirmed(owner, "agent.rules.add", { effect: "deny", command: "plugin:@test/*", sessionId: opened.session.id }));
    expect(code(await run(a, projectId, "plugin:@test/probe/edit", txOut([insert(15, 5)]), { branch: root }))).toBe("RULE_DENIED");
  });
});
