// Developer mode (sp8b plan Task 11, Decision 19; spec §5.5): `finisherd --plugin-dir <dir>`
// validates the folder as `@dev/<name>`, shows its permissions and the unconfined warning on the
// console, starts nothing without the person's `y`, then serves the plugin from its own folder for
// that run only (removed when the server stops, and at the next start after a crash), never to an
// agent or MCP principal. The plugin is the test-only probe (`fixtures/plugins/probe`) under the
// real plugin host, job engine, project registry and middleware chain.
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError, Principal } from "@finisher/contracts";
import { defaultScopes } from "@finisher/contracts";
import type { PluginCommandList, PluginCommandRunResult } from "@finisher/core-types";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect, Exit, Layer, Scope } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { withChain } from "../src/chain/chain.ts";
import { Connections } from "../src/connections.ts";
import { ConsoleError } from "../src/console/client.ts";
import { approveDevPlugin, devRefused } from "../src/console/dev-plugin.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { ControlStoreTag, openControlStore } from "../src/control/db.ts";
import type { JobEngine } from "../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../src/jobs/events.ts";
import { pluginCommandKind } from "../src/jobs/kinds/plugin.command.ts";
import { buildJobEngine } from "../src/jobs/layer.ts";
import { commandHandlers, type PluginCommandsService } from "../src/plugins/host/command-rpc.ts";
import { makeCommandRegistry } from "../src/plugins/host/commands.ts";
import { type DevPlugin, devPluginForRun, installDevPlugin, loadDevPlugin } from "../src/plugins/host/dev.ts";
import { makePluginHost } from "../src/plugins/host/processes.ts";
import { reconcileStore } from "../src/plugins/store-sync.ts";
import { PluginTables } from "../src/plugins/tables.ts";
import { LOCAL } from "../src/principal.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { makeProjectRegistry } from "../src/projects.ts";
import { addAsset, insert, tx } from "./support/ndjson-client.ts";
import { run } from "./support/jobs.ts";

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "plugins", "probe");
const ID = "@dev/probe";
const LOOK = `plugin:${ID}/look`;
const ARGS = { type: "object", properties: { op: { type: "string", enum: ["command"] }, out: { type: "string", maxLength: 100000 } }, required: ["op"], additionalProperties: false };
const PERMISSIONS = [{ perm: "project:read" }];
const DATA = '{"kind":"data","value":{"n":1}}';
const restricted = (kind: "mcp" | "agent"): Principal => ({
  kind,
  id: `01${kind}0000000000000000000000a`.slice(0, 26),
  scopes: defaultScopes("local").filter((s) => s.startsWith("project.") || s.startsWith("plugin.read")),
  projects: "*",
  branches: "*",
  human: false,
});

const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

/** The probe unpacked in a folder of its own, outside server data, as `id` with `more` merged in. */
function devFolder(id = ID, more: Record<string, unknown> = {}): string {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "fin-dev-plugin-")));
  cleanups.push(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(FIXTURE, dir, { recursive: true });
  const manifest = JSON.parse(readFileSync(join(dir, "finisher-plugin.json"), "utf8")) as Record<string, unknown>;
  const commands = [{ name: "look", title: "cmd.look", args: ARGS }];
  writeFileSync(join(dir, "finisher-plugin.json"), JSON.stringify({ ...manifest, id, messages: "messages", locales: ["en"], contributes: { commands }, permissions: PERMISSIONS, ...more }));
  mkdirSync(join(dir, "messages"));
  writeFileSync(join(dir, "messages", "en.json"), JSON.stringify({ "cmd.look": "Look" }));
  return dir;
}

/** A prompt that answers `answers` in turn and records what it showed. */
function fakePrompt(answers: string[], interactive = true): Prompt & { readonly shown: string[] } {
  const shown: string[] = [];
  return { shown, isInteractive: () => interactive, show: (t) => void shown.push(t), ask: async (q) => (shown.push(q), answers.shift() ?? "") };
}

const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), plugin: { kind: "disabled", reason: "profileNotReady" } });
const ENFORCED = (): SandboxStatus => ({ ...sandboxStatus(), plugin: { kind: "enforced" } as SandboxStatus["plugin"] });
function withStatus(s: () => SandboxStatus): void {
  const prev = setSandboxStatusSource(s);
  cleanups.push(() => void setSandboxStatusSource(prev));
}

interface Rig {
  readonly serverData: string;
  readonly tables: PluginTables;
  readonly engine: JobEngine;
  readonly projectId: string;
  as(p: Principal): void;
  call<A>(tag: string, payload: unknown): Promise<Exit.Exit<A, FinError>>;
  look(): Promise<Exit.Exit<PluginCommandRunResult, FinError>>;
}

async function rig(): Promise<Rig> {
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-dev-")));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const tables = new PluginTables(control);
  const host = makePluginHost({ serverData, control, tables, log: () => {} });
  const registry = makeCommandRegistry({ serverData, tables });
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
    }).pipe(Effect.provide(Layer.mergeAll(Layer.succeed(ControlStoreTag, control), Layer.succeed(JobEventSinkTag, memorySink())))),
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
  const { projectId } = await Effect.runPromise(projects.create(join(serverData, "p"), "Dev", "30"));
  await Effect.runPromise(projects.commit(projectId, tx("t-asset", 0, [addAsset()])));
  await Effect.runPromise(projects.commit(projectId, tx("t-base", 1, [insert(0, 5)])));

  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  let live: Principal = LOCAL;
  connections.get(1)!.principal = live;
  host.serve({
    live: (o) => (o.kind === live.kind && o.id === live.id ? live : undefined),
    reach: { subtree: (_p, _root, _project, branch) => Effect.succeed(branch) },
    snapshot: (id, branch) => projects.snapshot(id, branch),
  });
  const service: PluginCommandsService = { registry, host, engine, onRun: () => {} };
  type Handler = (payload: unknown, options: object) => Effect.Effect<unknown, FinError>;
  let chained: Record<string, Handler> = {};
  const commands = commandHandlers({ service, projects, chained: () => chained, guard: (_t, self) => self });
  const chainProjects = {
    isOpen: projects.isOpen,
    dirOf: projects.dirOf,
    indexedDirOf: projects.indexedDirOf,
    resolveBranch: projects.resolveBranch,
    branches: (id: string) => Effect.map(projects.branches(id, true), (r) => r.branches),
  };
  chained = withChain({ "plugin.commands": commands["plugin.commands"] as Handler, "plugin.command.run": commands["plugin.command.run"] as Handler }, {
    connections,
    projects: chainProjects,
    pluginCommands: { writes: registry.writes },
  }) as unknown as Record<string, Handler>;
  const call = <A,>(tag: string, payload: unknown) => Effect.runPromiseExit(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Effect.Effect<A, FinError>);
  return {
    serverData,
    tables,
    engine,
    projectId,
    as(p) {
      live = p;
      connections.get(1)!.principal = p;
    },
    call,
    look: () => call<PluginCommandRunResult>("plugin.command.run", { projectId, name: LOOK, args: { op: "command", out: DATA } }),
  };
}

const value = <A,>(e: Exit.Exit<A, FinError>): A => {
  if (Exit.isSuccess(e)) return e.value;
  throw new Error(`expected success, got ${JSON.stringify(Exit.findErrorOption(e))}`);
};
const errorOf = (e: Exit.Exit<unknown, FinError>): FinError | undefined => (Exit.isSuccess(e) ? undefined : (Exit.findErrorOption(e) as { value?: FinError }).value);
const names = async (r: Rig): Promise<string[]> => value(await r.call<PluginCommandList>("plugin.commands", {})).commands.map((c) => c.name);
const errorCode = async (p: Promise<unknown>): Promise<string | undefined> =>
  p.then(
    () => undefined,
    (e: unknown) => (e instanceof ConsoleError ? e.error.code : (e as { code?: string }).code),
  );

/** A server run's developer-mode step (`serve`'s), held open until the returned close. */
async function startRun(r: Rig, dev: DevPlugin | undefined, log: string[] = []): Promise<() => Promise<void>> {
  const scope = Effect.runSync(Scope.make());
  await Effect.runPromise(Scope.provide(scope)(devPluginForRun(r.tables, dev, (m) => void log.push(m))));
  return () => Effect.runPromise(Scope.close(scope, Exit.void));
}

describe("developer mode", () => {
  test("dev_plugin_runs_for_this_run_only: served from its own folder while the run lasts, gone after it and after a crash", async () => {
    const r = await rig();
    const folder = devFolder();
    const dev = await loadDevPlugin(folder);
    expect(dev).toMatchObject({ dir: folder, manifest: { id: ID, kind: "code" } });

    const stop = await startRun(r, dev);
    const row = r.tables.installedFull(ID)!;
    expect(row).toMatchObject({ source: "dev", dir: folder, kind: "code", enabled: true, state: "active", treeHash: "dev", installedBy: "console:console" });
    expect(r.tables.grants(ID).map((g) => JSON.parse(g) as unknown)).toEqual(PERMISSIONS);
    expect(await names(r)).toEqual([LOOK]);
    // It runs from its folder: nothing was copied into the store.
    expect(value(await r.look())).toEqual({ outcome: "data", data: { n: 1 } });
    const job = r.engine.list({ limit: 10 }).jobs.find((j) => j.kind === "plugin.command");
    expect(job?.via).toEqual({ plugin: `${ID}@1.0.0` });
    expect(() => readFileSync(join(r.serverData, "plugins", "store", "@dev", "probe"))).toThrow();
    // The start's store check leaves it alone: it is not in the store, and not meant to be.
    const store = { installedDirs: async () => "[]", removePlugin: async () => "{}" } as unknown as Parameters<typeof reconcileStore>[0];
    expect((await Effect.runPromise(reconcileStore(store, r.tables, () => 1))).problems).toEqual([]);
    expect(r.tables.installedOne(ID)).toMatchObject({ enabled: true, state: "active" });
    // The folder changes while the run lasts: nothing re-hashes it, the command still runs.
    writeFileSync(join(folder, "notes.txt"), "edited");
    expect(value(await r.look())).toEqual({ outcome: "data", data: { n: 1 } });

    // The server stops: the row and its grants go.
    await stop();
    expect(r.tables.installedOne(ID)).toBeUndefined();
    expect(r.tables.grants(ID)).toEqual([]);
    expect(await names(r)).toEqual([]);
    expect(errorOf(await r.look())?.code).toBe("PLUGIN_NOT_FOUND");

    // A run that never stopped (a crash) leaves its row; the next start without the flag removes it.
    installDevPlugin(r.tables, dev, 5);
    expect(await names(r)).toEqual([LOOK]);
    const next = await startRun(r, undefined);
    expect(r.tables.installedOne(ID)).toBeUndefined();
    expect(r.tables.grants(ID)).toEqual([]);
    expect(errorOf(await r.look())?.code).toBe("PLUGIN_NOT_FOUND");
    await next();
  });

  test("dev_plugin_invisible_to_agents: absent from an agent's or MCP client's list and refused PLUGIN_NOT_FOUND", async () => {
    const r = await rig();
    const stop = await startRun(r, await loadDevPlugin(devFolder()));
    cleanups.push(stop);
    expect(await names(r)).toEqual([LOOK]);
    for (const kind of ["mcp", "agent"] as const) {
      r.as(restricted(kind));
      expect(await names(r)).toEqual([]);
      expect(errorOf(await r.look())?.code).toBe("PLUGIN_NOT_FOUND");
    }
    // A person still runs it.
    r.as(LOCAL);
    expect(value(await r.look())).toEqual({ outcome: "data", data: { n: 1 } });
  });

  test("dev_permissions_need_console_approval: no terminal, no answer or a no installs nothing; y approves what was shown", async () => {
    withStatus(ENFORCED);
    const folder = devFolder(ID, { publisher: { name: "Evil\u001b[2J‮Corp" }, permissions: [{ perm: "project:read" }, { perm: "net", origins: ["https://api.example.org"] }] });
    // Without a terminal nothing is read or asked.
    const pipe = fakePrompt(["y"], false);
    expect(await errorCode(approveDevPlugin(folder, pipe))).toBe("CONSOLE_NEEDS_TTY");
    expect(pipe.shown).toEqual([]);
    // Declined: no answer (end of input or Ctrl-C), or anything but y.
    for (const answer of ["", "n", "no", "yy"]) expect(await errorCode(approveDevPlugin(folder, fakePrompt([answer])))).toBe("CONFIRMATION_REQUIRED");
    // A folder that is not a developer plugin is refused before anything is asked.
    const other = fakePrompt(["y"]);
    expect(await errorCode(approveDevPlugin(devFolder("@test/probe"), other))).toBe("PLUGIN_MANIFEST_INVALID");
    expect(await errorCode(approveDevPlugin(join(folder, "missing"), other))).toBe("PLUGIN_NOT_FOUND");
    expect(other.shown).toEqual([]);
    // A refusal is reported as the start's error and exit code.
    const written: string[] = [];
    const code = devRefused(await approveDevPlugin(folder, fakePrompt(["n"])).catch((e: unknown) => e), (t) => void written.push(t));
    expect(written[0]).toMatch(/^finisherd: CONFIRMATION_REQUIRED: /);
    expect(code).toBeGreaterThan(0);

    // Approved: the screen named the plugin and every permission, with the publisher's text made plain.
    const prompt = fakePrompt(["y"]);
    const dev = await approveDevPlugin(folder, prompt);
    expect(dev?.dir).toBe(folder);
    const screen = prompt.shown.join("\n");
    expect(screen).toContain(`${ID} 1.0.0`);
    expect(screen).toContain("Permissions (2), granted until this server stops:");
    expect(screen).toContain("  project:read");
    expect(screen).toContain("  net (origins: https://api.example.org)");
    expect(screen).toContain("by Evil[2JCorp");
    expect(screen).not.toMatch(/[\u001b‮]/);
    expect(screen).toContain("Proceed? [y/N]");
    expect(screen).not.toContain("PLUGINS_UNCONFINED");
    // Without the flag nothing is asked.
    expect(await approveDevPlugin(undefined, fakePrompt([], false))).toBeUndefined();
  });

  test("the unconfined warning is printed on the approval screen and at start", async () => {
    withStatus(NOT_READY);
    const folder = devFolder();
    const prompt = fakePrompt(["y"]);
    const dev = await approveDevPlugin(folder, prompt);
    const screen = prompt.shown.join("\n");
    expect(screen).toContain("!! DEVELOPER PLUGIN");
    expect(screen).toContain("!! PLUGINS_UNCONFINED");
    expect(screen.indexOf("PLUGINS_UNCONFINED")).toBeLessThan(screen.indexOf("Permissions"));

    const r = await rig();
    const log: string[] = [];
    cleanups.push(await startRun(r, dev, log));
    expect(log.some((l) => l.includes(`developer plugin ${ID} 1.0.0`) && l.includes("this run only"))).toBe(true);
    expect(log.some((l) => l.startsWith(`finisherd: PLUGINS_UNCONFINED (${ID}): `))).toBe(true);
  });
});
