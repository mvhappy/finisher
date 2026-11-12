// The RPC handlers of plugin commands (sp8b Task 9): `plugin.commands` and `plugin.command.run`,
// served once the plugin host runs (`PluginCommandsTag`; without it both answer
// `PLUGIN_RUNTIME_UNAVAILABLE`). The commit goes through the chain's `tx.commit` (`commitVia`).
import type { FinError, Principal } from "@finisher/contracts";
import type { PluginCommandRunRequest } from "@finisher/core-types";
import { Context, Effect } from "effect";
import { chainOf } from "../../chain/chain.ts";
import { authorOf } from "../../chain/policy.ts";
import { finError } from "../../errors.ts";
import type { JobEngine } from "../../jobs/api.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import { type CommandRunEvent, type CommitAsInvoker, makeCommandRun } from "./command-run.ts";
import { type CommandRegistry, makeCommandRegistry } from "./commands.ts";
import type { KindSpec } from "../../jobs/kinds.ts";
import { pluginCommandKind } from "../../jobs/kinds/plugin.command.ts";
import type { PluginTables } from "../tables.ts";
import { withHostCommit } from "./commit-channel.ts";
import type { PluginHost } from "./processes.ts";

export interface PluginCommandsService {
  readonly registry: CommandRegistry;
  readonly host: Pick<PluginHost, "rightsOf" | "calls">;
  readonly engine: Pick<JobEngine, "submit" | "wait" | "cancel">;
  /** The journal's `plugin.command` (Task 15). */
  readonly onRun?: (e: CommandRunEvent) => void;
  /** Enabled, active code plugins: `server.status`'s `PLUGINS_UNCONFINED {enabled}` (Decision 17). */
  readonly enabledCode?: () => number;
}

export class PluginCommandsTag extends Context.Service<PluginCommandsTag, PluginCommandsService>()("finisherd/PluginCommands") {}

type Handler = (payload: unknown, options: object) => Effect.Effect<unknown, FinError>;

export interface CommandHandlerDeps {
  readonly service: PluginCommandsService | undefined;
  readonly projects: Pick<ProjectRegistryShape, "resolveBranch" | "snapshot">;
  /** The chained handlers (`withChain`'s result), read when a run commits. */
  readonly chained: () => Record<string, Handler>;
  readonly sessionRootOf?: (p: Principal) => string | undefined;
  readonly authorOf?: (p: Principal) => string | undefined;
  readonly guard: <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;
}

const unavailable = (): FinError => finError("PLUGIN_RUNTIME_UNAVAILABLE", "this server does not run plugin commands", {});

export function commandHandlers(d: CommandHandlerDeps) {
  const principalOf = (options: object): Principal | undefined => chainOf(options)?.principal;
  const runner = (options: object) => {
    const s = d.service;
    if (s === undefined) return undefined;
    // The invoker's own `tx.commit`, on its own connection, through every stage of the chain.
    const commit: CommitAsInvoker = (projectId, tx, generation, opts) =>
      Effect.suspend(() => {
        const h = d.chained()["tx.commit"];
        if (h === undefined) return Effect.fail(finError("INTERNAL", "tx.commit is not served", {}));
        return h({ projectId, tx, ...(generation === undefined ? {} : { generation }) }, withHostCommit(options, opts)) as Effect.Effect<never, FinError>;
      });
    return makeCommandRun({
      registry: s.registry,
      host: s.host,
      engine: s.engine,
      projects: d.projects,
      commit,
      ...(d.sessionRootOf === undefined ? {} : { sessionRootOf: d.sessionRootOf }),
      authorOf: (p) => d.authorOf?.(p) ?? authorOf(p),
      ...(s.onRun === undefined ? {} : { onRun: s.onRun }),
    });
  };
  return {
    "plugin.commands": (_: object, options: object) =>
      d.guard(
        "plugin.commands",
        Effect.suspend(() => {
          const p = principalOf(options);
          if (d.service === undefined || p === undefined) return Effect.fail(unavailable());
          return d.service.registry.list(p);
        }),
      ),
    "plugin.command.run": (req: PluginCommandRunRequest, options: object) =>
      d.guard(
        "plugin.command.run",
        Effect.suspend(() => {
          const p = principalOf(options);
          const r = runner(options);
          if (r === undefined || p === undefined) return Effect.fail(unavailable());
          return r.run(p, req);
        }),
      ),
  };
}

/** The server's plugin commands: the registry, the `plugin.command` kind and, once the engine runs, the service. */
export function serverPluginCommands(d: { readonly serverData: string; readonly tables: PluginTables; readonly host: PluginHost }) {
  const registry = makeCommandRegistry({ serverData: d.serverData, tables: d.tables });
  return {
    registry,
    kind: pluginCommandKind({ contextOf: (job, o) => d.host.contextOf(job, o), timeoutOf: registry.timeoutOf }) as unknown as KindSpec,
    // Every run, refusals included, is journaled as `plugin.command` (Task 15).
    service: (engine: PluginCommandsService["engine"]): PluginCommandsService => ({
      registry, host: d.host, engine, onRun: (e) => d.host.onCommandRun(e),
      enabledCode: () => d.tables.installed().filter((r) => r.kind === "code" && r.enabled && r.state === "active").length,
    }),
  };
}
