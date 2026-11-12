// A real plugin command path for tests (sp8b Task 9): one installed, enabled code plugin under
// the real plugin host, job engine, project registry and middleware chain, with the chain's own
// `tx.commit` and the `plugin.commands`/`plugin.command.run` handlers on one connection. Used by
// `plugins-commands.test.ts` (the probe) and `plugins-sdk-e2e.test.ts` (the SDK examples).
import { mkdirSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, Principal } from "@finisher/contracts";
import type { CommittedTx, PluginCommandRunResult } from "@finisher/core-types";
import { Effect, Exit, type Fiber, Layer, Scope } from "effect";
import { chainOf, withChain, writeGuardOf } from "../../src/chain/chain.ts";
import { withoutVia } from "../../src/chain/client-via.ts";
import { Connections } from "../../src/connections.ts";
import { ControlStoreTag, openControlStore } from "../../src/control/db.ts";
import { actorFor } from "../../src/diagnostics.ts";
import type { JobEngine } from "../../src/jobs/engine.ts";
import { JobEventSinkTag, memorySink } from "../../src/jobs/events.ts";
import { pluginCommandKind } from "../../src/jobs/kinds/plugin.command.ts";
import { buildJobEngine } from "../../src/jobs/layer.ts";
import { commandHandlers, type PluginCommandsService } from "../../src/plugins/host/command-rpc.ts";
import type { CommandRunEvent } from "../../src/plugins/host/command-run.ts";
import { makeCommandRegistry } from "../../src/plugins/host/commands.ts";
import { txToCommit } from "../../src/plugins/host/commit-channel.ts";
import type { EgressTestHooks } from "../../src/plugins/host/egress.ts";
import { makePluginHost, type PluginHost } from "../../src/plugins/host/processes.ts";
import { PluginTables } from "../../src/plugins/tables.ts";
import { LOCAL } from "../../src/principal.ts";
import { makeProjectRegistry, type ProjectRegistryShape } from "../../src/projects.ts";
import { run } from "./jobs.ts";

type Handler = (payload: unknown, options: object) => Effect.Effect<unknown, FinError>;

export interface CommandRigOptions {
  /** `@publisher/name`. */
  readonly id: string;
  readonly version?: string;
  /** Fills the plugin's store folder (`plugins/store/<id>/<version>-t`), which exists empty. */
  readonly place: (dir: string) => void;
  /** The granted permissions (as the console would record them). */
  readonly grants: ReadonlyArray<object>;
  /** Transactions committed to the new project before the test (`tx(...)` shapes). */
  readonly setup: ReadonlyArray<(rev: number) => Record<string, unknown>>;
  readonly egressHooks?: EgressTestHooks;
  /** Runs when the test ends (the caller's `afterEach` list). */
  readonly cleanups: Array<() => Promise<void> | void>;
}

export interface CommandRig {
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
  /** `plugin.command.run` of the plugin's `command` with `args`. */
  run(command: string, args: unknown, more?: Record<string, unknown>): Promise<Exit.Exit<PluginCommandRunResult, FinError>>;
  head(): Promise<number>;
  commit(t: Record<string, unknown>): Promise<CommittedTx>;
}

export async function commandRig(opts: CommandRigOptions): Promise<CommandRig> {
  const version = opts.version ?? "1.0.0";
  const serverData = realpathSync(mkdtempSync(join(tmpdir(), "fin-plugin-cmds-")));
  const control = openControlStore(serverData, { serverId: "sid-test", checkpoint: false });
  const tables = new PluginTables(control);
  const host = makePluginHost({ serverData, control, tables, log: () => {}, ...(opts.egressHooks === undefined ? {} : { egressHooks: opts.egressHooks }) });
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
  opts.cleanups.push(async () => {
    await Effect.runPromise(host.close());
    await run(engine.close());
    await Effect.runPromise(Scope.close(scope, Exit.void));
    await control.close();
    rmSync(serverData, { recursive: true, force: true });
  });
  // The plugin as an installed, enabled code plugin.
  const dir = `store/${opts.id}/${version}-t`;
  const full = join(serverData, "plugins", ...dir.split("/"));
  mkdirSync(full, { recursive: true });
  opts.place(full);
  tables.addPlugin({ id: opts.id, version, source: "registry", treeHash: "t", dir, kind: "code", enabled: true, state: "active", installedBy: "console:console", installedAt: 0 });
  tables.setGrants(opts.id, version, opts.grants.map((g) => JSON.stringify(g)), "console:console", 0);

  const { projectId } = await Effect.runPromise(projects.create(join(serverData, "p"), "Commands", "30"));
  for (const [rev, make] of opts.setup.entries()) await Effect.runPromise(projects.commit(projectId, make(rev)));

  // The chain over the real `tx.commit` (as `handlers.ts` serves it) and the command handlers.
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  connections.get(1)!.principal = LOCAL;
  let liveInvoker: Principal | undefined = LOCAL;
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
  const call = <A>(tag: string, payload: unknown) => Effect.runPromiseExit(chained[tag]!(payload, { client: { id: 1 }, headers: {} }) as Effect.Effect<A, FinError>);
  return {
    serverData,
    engine,
    host,
    tables,
    projects,
    projectId,
    runs,
    as(p) {
      liveInvoker = p;
      connections.get(1)!.principal = p;
    },
    call,
    fork: (tag, payload) => Effect.runFork(chained[tag]!(payload, { client: { id: 1 }, headers: {} })),
    run: (command, args, more = {}) => call<PluginCommandRunResult>("plugin.command.run", { projectId, name: `plugin:${opts.id}/${command}`, args, ...more }),
    head: async () => (await Effect.runPromise(projects.snapshot(projectId, "main"))).rev,
    commit: (t) => Effect.runPromise(projects.commit(projectId, t)),
  };
}

/** The value of a successful exit (a test failure naming the error otherwise). */
export const value = <A>(e: Exit.Exit<A, FinError>): A => {
  if (Exit.isSuccess(e)) return e.value;
  throw new Error(`expected success, got ${JSON.stringify(Exit.findErrorOption(e))}`);
};
/** The `FinError` of a failed exit. */
export const errorOf = (e: Exit.Exit<unknown, FinError>): FinError | undefined => (Exit.isSuccess(e) ? undefined : (Exit.findErrorOption(e) as { value?: FinError }).value);
