// Plugin commands (sp8b plan Task 9; spec §3.8, Decisions 7, 8, 12): the registry of enabled
// plugins' commands and its digest, argument checks before any job, the `plugin.command` job
// owned by the invoker, the snapshot held from submit (its rev is every page's and the
// transaction's `baseRev`), the commit through the chain's own `tx.commit` as the live invoker
// with `Tx.via`, dry runs, conflicts, invalid ops, read-only commands and timeouts. The
// commands run in the test-only probe (`fixtures/plugins/probe`) under the real plugin host,
// job engine, project registry and middleware chain.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, JobView, Principal } from "@finisher/contracts";
import { defaultScopes } from "@finisher/contracts";
import type { CommittedTx, PluginCommandList, PluginCommandRunResult } from "@finisher/core-types";
import { Effect, Exit, Fiber, Layer, Scope } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { commandsOf } from "../src/agent/command-of.ts";
import { chainOf, withChain, writeGuardOf } from "../src/chain/chain.ts";
import { withoutVia } from "../src/chain/client-via.ts";
import { Connections } from "../src/connections.ts";
import { ControlStoreTag, openControlStore } from "../src/control/db.ts";
import { actorFor } from "../src/diagnostics.ts";
import type { JobEngine } from "../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../src/jobs/events.ts";
import { pluginCommandKind } from "../src/jobs/kinds/plugin.command.ts";
import { buildJobEngine } from "../src/jobs/layer.ts";
import { commandHandlers, type PluginCommandsService } from "../src/plugins/host/command-rpc.ts";
import type { CommandRunEvent } from "../src/plugins/host/command-run.ts";
import { makeCommandRegistry } from "../src/plugins/host/commands.ts";
import { txToCommit } from "../src/plugins/host/commit-channel.ts";
import { keyTmpDir } from "../src/plugins/host/keys.ts";
import { makePluginHost, type PluginHost } from "../src/plugins/host/processes.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { LOCAL } from "../src/principal.ts";
import { makeProjectRegistry, type ProjectRegistryShape } from "../src/projects.ts";
import { addAsset, insert, tx } from "./support/ndjson-client.ts";
import { run } from "./support/jobs.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ID = "@test/probe";
const MCP_ID = "01mcp00000000000000000000a";
const mcp = (more: Partial<Principal> = {}): Principal => ({ kind: "mcp", id: MCP_ID, scopes: defaultScopes("local").filter((s) => s.startsWith("project.") || s.startsWith("plugin.read")), projects: "*", branches: "*", human: false, ...more });

const ARGS = {
  type: "object",
  properties: {
    op: { type: "string", enum: ["command"] },
    out: { type: "string", maxLength: 100000 },
    read: { type: "boolean" },
    waitFor: { type: "string", pattern: "^[a-z]+$" },
    sleepMs: { type: "integer", minimum: 0, maximum: 120000 },
  },
  required: ["op"],
  additionalProperties: false,
};
const COMMANDS = [
  { name: "edit", title: "cmd.edit", args: ARGS, writes: true },
  { name: "look", title: "cmd.look", args: ARGS },
  { name: "slow", title: "cmd.slow", args: ARGS, writes: true, timeoutS: 1 },
];
const GRANTS = [{ perm: "project:read" }, { perm: "project:write" }];

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

type Handler = (payload: unknown, options: object) => Effect.Effect<unknown, FinError>;

interface Rig {
  readonly serverData: string;
  readonly engine: JobEngine;
  readonly host: PluginHost;
  readonly tables: PluginTables;
  readonly projects: ProjectRegistryShape;
  readonly projectId: string;
  readonly runs: CommandRunEvent[];
  /** Binds `p` to the rig's connection (the live invoker the chain reads). */
  as(p: Principal): void;
  call<A>(tag: string, payload: unknown): Promise<Exit.Exit<A, FinError>>;
  /** `call`, as a fiber the test can interrupt (the caller going away). */
  fork(tag: string, payload: unknown): Fiber.Fiber<unknown, FinError>;
  /** `plugin.command.run` of `command` with the probe's `command` op. */
  runCommand(command: string, a: Record<string, unknown>, more?: Record<string, unknown>): Promise<Exit.Exit<PluginCommandRunResult, FinError>>;
  head(): Promise<number>;
  commit(t: Record<string, unknown>): Promise<CommittedTx>;
}

async function rig(opts: { grants?: ReadonlyArray<object>; commands?: ReadonlyArray<object> } = {}): Promise<Rig> {
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-cmds-")));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const tables = new PluginTables(control);
  const host = makePluginHost({ serverData, control, tables, log: () => {} });
  const registry = makeCommandRegistry({ serverData, tables });
  const sink = memorySink();
  const engine = await Effect.runPromise(
    buildJobEngine(serverData, {
      testKinds: false,
      cgroups: "off",
      cores: 4,
      timing: { killWaitMs: 2000, exitDrainMs: 100, hungMs: 1500 },
      register: (r) => {
        host.register(r);
        r.registerKind(pluginCommandKind({ contextOf: (j, o) => host.contextOf(j, o), timeoutOf: registry.timeoutOf }) as never);
      },
    }).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, sink)))),
  );
  host.bind(engine);
  const scope = Effect.runSync(Scope.make());
  const projects = await Effect.runPromise(Scope.provide(scope)(makeProjectRegistry(join(serverData, "registry"))));
  cleanups.push(async () => {
    await Effect.runPromise(host.close());
    await run(engine.close());
    await Effect.runPromise(Scope.close(scope, Exit.void));
    await control.close();
    rmSync(serverData, { recursive: true, force: true });
  });
  // The probe as an installed, enabled code plugin with three commands.
  const dir = "store/@test/probe/1.0.0-t";
  const full = join(serverData, "plugins", ...dir.split("/"));
  mkdirSync(dirname(full), { recursive: true });
  cpSync(FIXTURE, full, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(full, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  writeFileSync(join(full, "finisher-plugin.json"), JSON.stringify({ ...manifest, messages: "messages", locales: ["en"], contributes: { commands: opts.commands ?? COMMANDS } }));
  mkdirSync(join(full, "messages"));
  writeFileSync(join(full, "messages", "en.json"), JSON.stringify({ "cmd.edit": "Edit", "cmd.look": "Look", "cmd.slow": "Slow" }));
  tables.addPlugin({ id: ID, version: "1.0.0", source: "registry", treeHash: "t", dir, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
  tables.setGrants(ID, "1.0.0", (opts.grants ?? GRANTS).map((g) => JSON.stringify(g)), "console:console", 0);

  const { projectId } = await Effect.runPromise(projects.create(join(serverData, "p"), "Commands", "30"));
  await Effect.runPromise(projects.commit(projectId, tx("t-asset", 0, [addAsset()])));
  await Effect.runPromise(projects.commit(projectId, tx("t-base", 1, [insert(0, 5)])));

  // The chain over the real `tx.commit` (as `handlers.ts` serves it) and the command handlers.
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  let principal: Principal = LOCAL;
  connections.get(1)!.principal = principal;
  let liveInvoker: Principal | undefined = principal;
  host.serve({
    live: (o) => (liveInvoker !== undefined && o.kind === liveInvoker.kind && o.id === liveInvoker.id ? liveInvoker : undefined),
    reach: { subtree: (_p, _root, _project, branch) => Effect.succeed(branch) },
    snapshot: (id, branch) => projects.snapshot(id, branch),
  });
  const runs: CommandRunEvent[] = [];
  const service: PluginCommandsService = { registry, host, engine, onRun: (e) => void runs.push(e) };
  let chained: Record<string, Handler> = {};
  const commands = commandHandlers({ service, projects, chained: () => chained, guard: (_t, self) => self });
  const chainProjects = {
    isOpen: projects.isOpen,
    dirOf: projects.dirOf,
    indexedDirOf: projects.indexedDirOf,
    resolveBranch: projects.resolveBranch,
    branches: (id: string) => Effect.map(projects.branches(id, true), (r) => r.branches),
  };
  const handlers: Record<string, Handler> = {
    "tx.commit": (payload, options) => {
      const { projectId: p, tx: t, generation } = payload as { projectId: string; tx: unknown; generation?: never };
      const sent = txToCommit(t, options, withoutVia);
      return Effect.flatMap(writeGuardOf(options), (guard): Effect.Effect<unknown, FinError> => ("dryRun" in sent ? Effect.succeed(sent) : projects.commit(p, sent.tx, generation, { ...actorFor(chainOf(options)!.principal, {}), guard })));
    },
    "plugin.commands": commands["plugin.commands"] as Handler,
    "plugin.command.run": commands["plugin.command.run"] as Handler,
  };
  chained = withChain(handlers, { connections, projects: chainProjects, pluginCommands: { writes: registry.writes } }) as unknown as Record<string, Handler>;
  const call = <A,>(tag: string, payload: unknown) => Effect.runPromiseExit(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Effect.Effect<A, FinError>);
  const r: Rig = {
    serverData,
    engine,
    host,
    tables,
    projects,
    projectId,
    runs,
    as(p) {
      principal = p;
      liveInvoker = p;
      connections.get(1)!.principal = p;
    },
    call,
    fork: (tag, payload) => Effect.runFork(chained[tag]!(payload, { client: { id: 1 }, headers: {} })),
    runCommand: (command, a, more = {}) => call<PluginCommandRunResult>("plugin.command.run", { projectId, name: `plugin:${ID}/${command}`, args: { op: "command", ...a }, ...more }),
    head: async () => (await Effect.runPromise(projects.snapshot(projectId, "main"))).rev,
    commit: (t) => Effect.runPromise(projects.commit(projectId, t)),
  };
  void principal;
  return r;
}

const value = <A,>(e: Exit.Exit<A, FinError>): A => {
  if (Exit.isSuccess(e)) return e.value;
  throw new Error(`expected success, got ${JSON.stringify(Exit.findErrorOption(e))}`);
};
const errorOf = (e: Exit.Exit<unknown, FinError>): FinError | undefined => (Exit.isSuccess(e) ? undefined : (Exit.findErrorOption(e) as { value?: FinError }).value);
const txOut = (ops: unknown[]): string => JSON.stringify({ kind: "tx", ops });
const items = async (r: Rig): Promise<Array<{ id: string; start: string }>> =>
  ((await Effect.runPromise(r.projects.snapshot(r.projectId, "main"))).project as { timelines: Array<{ tracks: Array<{ items: Array<{ id: string; start: string }> }> }> }).timelines[0]!.tracks[0]!.items;
const jobsOf = (r: Rig): JobView[] => r.engine.list({ limit: 50 }).jobs.filter((j) => j.kind === "plugin.command");

describe("plugin commands", () => {
  test("command_commits_as_invoker: author, principal and job owner are the invoker's; via names the plugin and command", async () => {
    const r = await rig();
    r.as(mcp());
    const list = value(await r.call<PluginCommandList>("plugin.commands", {}));
    expect(list.commands.map((c) => [c.name, c.plugin, c.writes, c.timeoutS])).toEqual([
      [`plugin:${ID}/edit`, `${ID}@1.0.0`, true, 10],
      [`plugin:${ID}/look`, `${ID}@1.0.0`, false, 10],
      [`plugin:${ID}/slow`, `${ID}@1.0.0`, true, 1],
    ]);
    expect(list.digest).toMatch(/^[0-9a-f]{64}$/);
    expect(value(await r.call<PluginCommandList>("plugin.commands", {})).digest).toBe(list.digest);

    const base = await r.head();
    const done = value(await r.runCommand("edit", { out: txOut([insert(5, 5)]) }));
    expect(done).toMatchObject({ outcome: "accepted", rev: base + 1 });
    const got = (await Effect.runPromise(r.projects.txGet(r.projectId, done.txId!))) as CommittedTx;
    expect(got.tx.via).toEqual({ plugin: `${ID}@1.0.0`, command: "edit" });
    expect(got.tx.author).toBe(`mcp:${MCP_ID}`);
    expect(got.tx.baseRev).toBe(base);
    const [job] = jobsOf(r);
    expect(job).toMatchObject({ owner: { kind: "mcp", id: MCP_ID }, via: { plugin: `${ID}@1.0.0` }, project: r.projectId, params: { command: "edit", rev: base } });
    expect((job!.rights as { principal: unknown }).principal).toEqual({ kind: "mcp", id: MCP_ID });
    expect(r.runs.at(-1)).toMatchObject({ plugin: `${ID}@1.0.0`, command: "edit", principal: { kind: "mcp" }, outcome: "accepted", dryRun: false });

    // The commit is the invoker's own `tx.commit`: an MCP credential limited to another branch
    // is refused there, and nothing is committed.
    r.as(mcp({ branches: ["feature"] }));
    const head = await r.head();
    expect(errorOf(await r.runCommand("edit", { out: txOut([insert(10, 5)]) }))?.code).toBe("BRANCH_NOT_ALLOWED");
    expect(await r.head()).toBe(head);
    // Without `project.write` the run is refused before any job (the rule's escalation).
    r.as(mcp({ scopes: ["project.read", "plugin.read"] }));
    const before = jobsOf(r).length;
    expect(errorOf(await r.runCommand("edit", { out: txOut([]) }))?.code).toBe("SCOPE_DENIED");
    // A read-only command needs only `project.read`.
    expect(value(await r.runCommand("look", { out: '{"kind":"data","value":{"n":1}}' }))).toEqual({ outcome: "data", data: { n: 1 } });
    expect(jobsOf(r).length).toBe(before + 1);
    // 7B's command rules see the run under its registry name (and as `rpc:`), never a malformed one.
    expect(commandsOf("plugin.command.run", { name: `plugin:${ID}/edit` })).toEqual([`plugin:${ID}/edit`, "rpc:plugin.command.run"]);
    expect(commandsOf("plugin.command.run", { name: "plugin:../x" })).toEqual(["rpc:plugin.command.run"]);
    // A client's own `tx.commit` still cannot name a plugin.
    r.as(LOCAL);
    const c = value(await r.call<CommittedTx>("tx.commit", { projectId: r.projectId, tx: { ...tx("t-client", await r.head(), [insert(0, 1)]), via: { plugin: `${ID}@1.0.0`, command: "edit" } } }));
    expect(c.tx.via).toBeUndefined();
  });

  test("conflicting_tx_rebases_or_conflicts: the plugin's base is the snapshot held at submit", async () => {
    const r = await rig();
    // A commit lands between submit and the plugin's first read: the plugin still reads the
    // submit's rev, its transaction's `baseRev` is that rev, and fin-core rebases it once.
    const base = await r.head();
    const running = r.runCommand("edit", { waitFor: "go", read: true, out: txOut([insert(5, 5)]) });
    let job: JobView | undefined;
    for (let i = 0; i < 400 && job === undefined; i++) {
      job = jobsOf(r).find((j) => j.state === "running");
      if (job === undefined) await new Promise((x) => setTimeout(x, 25));
    }
    expect(job).toBeDefined();
    const between = await r.commit(tx("t-between", base, [insert(0, 3)]));
    expect(between.rev).toBe(base + 1);
    writeFileSync(join(keyTmpDir(r.serverData, r.host.warmKeyOf(job!)), "go"), "");
    const done = value(await running);
    expect(done).toMatchObject({ outcome: "rebased", rev: base + 2 });
    const got = (await Effect.runPromise(r.projects.txGet(r.projectId, done.txId!))) as CommittedTx;
    // The plugin's base was the held rev (its job's `CommandRun.rev`); fin-core rebased it over
    // the commit in between, which the stored record's `baseRev` now names.
    expect((job!.params as { rev: number }).rev).toBe(base);
    expect(got.tx.baseRev).toBe(base + 1);
    expect(got.tx.via).toEqual({ plugin: `${ID}@1.0.0`, command: "edit" });
    // Three clips: the base's, the one between and the plugin's, each once.
    expect((await items(r)).length).toBe(3);
    // What the plugin reads after a commit in between: the rev held at submit, not the head.
    const held = await r.head();
    const seen = r.runCommand("look", { waitFor: "again", read: true, out: '{"kind":"data","value":{"rev":$REV}}' });
    let looking: JobView | undefined;
    for (let i = 0; i < 400 && looking === undefined; i++) {
      looking = jobsOf(r).find((j) => j.state === "running" && (j.params as { command: string }).command === "look");
      if (looking === undefined) await new Promise((x) => setTimeout(x, 25));
    }
    await r.commit(tx("t-between-2", held, [insert(0, 2)]));
    writeFileSync(join(keyTmpDir(r.serverData, r.host.warmKeyOf(looking!)), "again"), "");
    expect(value(await seen)).toEqual({ outcome: "data", data: { rev: held } });
    expect(await r.head()).toBe(held + 1);
    expect(r.host.calls.heldSnapshots()).toBe(0);

    // A conflict: the plugin deletes a clip a commit after its base deleted first.
    const now = await r.head();
    const conflicting = r.runCommand("edit", { sleepMs: 1500, out: txOut([{ op: "clip.delete", clip: "c1" }]) });
    for (let i = 0; i < 400 && !jobsOf(r).some((j) => j.state === "running"); i++) await new Promise((x) => setTimeout(x, 25));
    await r.commit(tx("t-del", now, [{ op: "clip.delete", clip: "c1" }]));
    const lost = value(await conflicting);
    expect(lost).toEqual({ outcome: "conflict", rev: now, ops: [{ op: "clip.delete", clip: "c1" }] });
    expect(r.runs.at(-1)).toMatchObject({ outcome: "conflict" });
  });

  test("invalid_ops_are_plugin_tx_invalid (and an undecodable result is PLUGIN_PROTOCOL)", async () => {
    const r = await rig();
    const head = await r.head();
    const bad = errorOf(await r.runCommand("edit", { out: txOut([{ op: "clip.delete", clip: "c99" }]) }));
    expect(bad?.code).toBe("PLUGIN_TX_INVALID");
    expect((bad?.details as { problems: Array<{ code: string }> }).problems[0]?.code).toBeTruthy();
    expect(errorOf(await r.runCommand("edit", { out: '{"kind":"tx","ops":[{"op":"no.such"}]}' }))?.code).toBe("PLUGIN_PROTOCOL");
    expect(errorOf(await r.runCommand("edit", { out: '{"nonsense":true}' }))?.code).toBe("PLUGIN_PROTOCOL");
    expect(await r.head()).toBe(head);
    expect(r.runs.map((x) => x.code)).toEqual(["PLUGIN_TX_INVALID", "PLUGIN_PROTOCOL", "PLUGIN_PROTOCOL"]);
    // A failed run lets its snapshot go.
    expect(r.host.calls.heldSnapshots()).toBe(0);
  });

  test("args_validated_before_job", async () => {
    const r = await rig();
    const e = errorOf(await r.call("plugin.command.run", { projectId: r.projectId, name: `plugin:${ID}/edit`, args: { op: "command", extra: 1, sleepMs: -1 } }));
    expect(e?.code).toBe("PLUGIN_COMMAND_ARGS");
    expect((e?.details as { problems: Array<{ path: string; reason: string }> }).problems.map((p) => p.reason).sort()).toEqual(["additionalProperties", "minimum"]);
    expect(jobsOf(r)).toEqual([]);
  });

  test("dry_run_commits_nothing", async () => {
    const r = await rig();
    const head = await r.head();
    const dry = value(await r.runCommand("edit", { out: txOut([insert(5, 5)]) }, { dryRun: true }));
    expect(dry).toEqual({ outcome: "data", dryRun: true, rev: head, ops: [insert(5, 5)] });
    expect(await r.head()).toBe(head);
    // A dry run is still the invoker's commit to check: a branch limit refuses it.
    r.as(mcp({ branches: ["feature"] }));
    expect(errorOf(await r.runCommand("edit", { out: txOut([insert(5, 5)]) }, { dryRun: true }))?.code).toBe("BRANCH_NOT_ALLOWED");
    // And invalid ops are invalid in a dry run too.
    r.as(LOCAL);
    expect(errorOf(await r.runCommand("edit", { out: txOut([{ op: "clip.delete", clip: "c99" }]) }, { dryRun: true }))?.code).toBe("PLUGIN_TX_INVALID");
    expect(await r.head()).toBe(head);
  });

  test("timeout_at_declared_limit", async () => {
    const r = await rig();
    const t0 = Date.now();
    const e = errorOf(await r.runCommand("slow", { sleepMs: 5000, out: txOut([]) }));
    expect(e).toMatchObject({ code: "PLUGIN_TIMEOUT", details: { timeoutS: 1 } });
    expect(Date.now() - t0).toBeLessThan(4500);
    for (let i = 0; i < 200 && jobsOf(r).some((j) => j.state === "running"); i++) await new Promise((x) => setTimeout(x, 25));
    expect(jobsOf(r)[0]?.state).toBe("cancelled");
    expect(r.host.calls.heldSnapshots()).toBe(0);
    // The job's own ceiling is the manifest maximum (the RPC cancels at the command's).
    expect(r.engine.kinds.kind("plugin.command")).toMatchObject({ timeoutS: 60, maxAttempts: 1, warm: true, defaultPriority: "interactive" });
  });

  test("writes_false_command_returning_tx_refused (and a writing command needs the project:write grant)", async () => {
    const r = await rig();
    const head = await r.head();
    const e = errorOf(await r.runCommand("look", { out: txOut([insert(5, 5)]) }));
    expect(e?.code).toBe("PLUGIN_TX_INVALID");
    expect(JSON.stringify(e?.details)).toContain("readOnlyCommand");
    expect(await r.head()).toBe(head);
    const g = await rig({ grants: [{ perm: "project:read" }] });
    const before = jobsOf(g).length;
    expect(errorOf(await g.runCommand("edit", { out: txOut([]) }))).toMatchObject({ code: "PLUGIN_PERMISSION_DENIED", details: { perm: "project:write", missing: "grant" } });
    expect(jobsOf(g).length).toBe(before);
  });

  test("disabled_plugin_command_not_found", async () => {
    const r = await rig();
    r.tables.setState(ID, false, "disabled", 1);
    expect(errorOf(await r.runCommand("edit", { out: txOut([]) }))?.code).toBe("PLUGIN_NOT_FOUND");
    expect(value(await r.call<PluginCommandList>("plugin.commands", {})).commands).toEqual([]);
    r.tables.setState(ID, true, "active", 2);
    expect(value(await r.call<PluginCommandList>("plugin.commands", {})).commands.length).toBe(3);
    expect(errorOf(await r.runCommand("nope", { out: txOut([]) }))?.code).toBe("PLUGIN_NOT_FOUND");
    expect(errorOf(await r.call("plugin.command.run", { projectId: r.projectId, name: "not-a-command", args: {} }))?.code).toBe("PLUGIN_NOT_FOUND");
    // A developer plugin is never listed to, nor run by, an agent or MCP principal.
    r.tables.replacePlugin({ id: ID, version: "1.0.0", source: "dev", treeHash: "t", dir: "store/@test/probe/1.0.0-t", kind: "code", enabled: true, state: "active" }, 3);
    r.as(mcp());
    expect(value(await r.call<PluginCommandList>("plugin.commands", {})).commands).toEqual([]);
    expect(errorOf(await r.runCommand("look", { out: txOut([]) }))?.code).toBe("PLUGIN_NOT_FOUND");
  });

  test("each_run_holds_its_own_snapshot: a rebuild between runs never serves one run the other's history", async () => {
    const r = await rig();
    const base = await r.head();
    const first = r.runCommand("edit", { waitFor: "go", read: true, out: txOut([insert(5, 5)]) });
    const running = await until(() => jobsOf(r).find((j) => j.state === "running"));
    // A rebuild: the same rev in a new history generation.
    await Effect.runPromise(r.projects.rebuild(r.projectId));
    expect(await r.head()).toBe(base);
    const second = r.runCommand("edit", { read: true, out: txOut([insert(10, 5)]) });
    const queued = await until(() => jobsOf(r).find((j) => j.id !== running.id));
    // Two runs of the same project, branch and rev hold two snapshots, each named by its own job.
    expect(r.host.calls.heldSnapshots()).toBe(2);
    const handle = (j: JobView) => (j.params as { snapshot?: string; rev: number }).snapshot;
    expect(handle(running)).toBeTypeOf("string");
    expect(handle(queued)).toBeTypeOf("string");
    expect(handle(queued)).not.toBe(handle(running));
    expect((queued.params as { rev: number }).rev).toBe(base);
    writeFileSync(join(keyTmpDir(r.serverData, r.host.warmKeyOf(running)), "go"), "");
    // The first read its snapshot before the rebuild: its commit is refused; the second's lands.
    expect(errorOf(await first)?.code).toBe("HISTORY_REWOUND");
    expect(value(await second)).toMatchObject({ outcome: "accepted", rev: base + 1 });
    expect(r.host.calls.heldSnapshots()).toBe(0);
  });

  test("caller_gone_cancels_the_job: an interrupted run cancels its job and lets its snapshot go", async () => {
    const r = await rig();
    const fiber = r.fork("plugin.command.run", { projectId: r.projectId, name: `plugin:${ID}/edit`, args: { op: "command", waitFor: "never", out: txOut([]) } });
    const job = await until(() => jobsOf(r).find((j) => j.state === "running"));
    expect(r.host.calls.heldSnapshots()).toBe(1);
    await Effect.runPromise(Fiber.interrupt(fiber));
    await until(() => (jobsOf(r).find((j) => j.id === job.id)?.state === "cancelled" ? true : undefined));
    expect(r.host.calls.heldSnapshots()).toBe(0);
  });

  test("command_rights_carry_a_net_grant", async () => {
    const net = { perm: "net", origins: ["https://api.example.org"] };
    const r = await rig({ grants: [...GRANTS, net] });
    value(await r.runCommand("look", { out: '{"kind":"data","value":null}' }));
    const rights = jobsOf(r)[0]!.rights as { grants: unknown[] };
    expect(rights.grants).toContainEqual(net);
  });
});

/** Polls `f` until it answers something (5 s at most). */
async function until<A>(f: () => A | undefined): Promise<A> {
  for (let i = 0; i < 200; i++) {
    const got = f();
    if (got !== undefined) return got;
    await new Promise((x) => setTimeout(x, 25));
  }
  throw new Error("timed out waiting");
}
