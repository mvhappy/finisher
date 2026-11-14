// The hooks as the server wires them (7F plan Task 4): one engine per server, built with the
// agent runtime (`agent/wiring.ts`), which closes it on shutdown. The event sources of Tasks 6–8
// add themselves here: the session side (Task 6: `session`, the engine's `SessionNotices` and
// the runtime's `hooks` and `sessionEvents`), the chain's policy rules and its after stage
// (Task 7, `chain.ts`; the reviews workflow bound by `startAgentReviews`); the plugin layer
// (Task 10: `PluginLayer`, its rows the configuration's plugin port and its runs the engine's,
// bound by `startAgentReviews` to the plugin host's source once the job engine runs, empty
// before). The review and job listeners (Task 8, `review.ts`,
// `jobs.ts`) are subscribed by `startAgentReviews`, which holds the review link and the job
// engine, and reach the session through `notices`. The `fin` hooks' credentials (Task 5):
// leftovers swept here at start, every one held ended at `close`, after the engine (no run is
// under way then).
import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { ChainHooks } from "../chain/hooks.ts";
import type { PolicyRule } from "../chain/policy.ts";
import type { Session } from "@finisher/contracts";
import { chainHooks, type ReviewItems } from "./chain.ts";
import type { HookRequest } from "@finisher/core-types";
import type { ConfiguredHook, HookConfig, PluginHookRow } from "./config.ts";
import { type HookCredentialDeps, HookCredentials } from "./credential.ts";
import { type HookAt, type HookEngineDeps, type HookEngineImpl, makeHookEngine, type PluginHookPort, type SessionNotices } from "./engine.ts";
import type { HookProcessDeps, HookRun } from "./process.ts";
import { SessionHooks } from "./session.ts";

/** The plugin host's handlers (`plugins/host/hooks.ts`): the rows to configure and their runs. */
export interface PluginHookSource extends PluginHookPort {
  /** The enabled, active code plugins' `contributes.hooks`. */
  rows(): readonly PluginHookRow[];
  /** A running hook job's timeout in seconds (`PluginContext.limits`), 60 when unknown. */
  timeoutOf(plugin: string, event: string, handler: string): number;
}

const notBound: HookRun = { notRun: "notReceived", detail: "no plugin host serves hooks" };

/** The plugin layer: empty until a source is bound, then its rows and runs. */
export class PluginLayer implements PluginHookPort {
  #source: PluginHookSource | undefined;

  bind(source: PluginHookSource): void {
    this.#source = source;
  }

  rows(): readonly PluginHookRow[] {
    return this.#source?.rows() ?? [];
  }

  digest(): string {
    return this.#source?.digest() ?? "";
  }

  run(h: ConfiguredHook, req: HookRequest, at: HookAt, signal?: AbortSignal): Promise<HookRun> {
    return this.#source === undefined ? Promise.resolve(notBound) : this.#source.run(h, req, at, signal);
  }
}

/** `serve`'s hook options (tests): the process's sandbox status or launcher. */
export type HookServeOptions = Pick<HookProcessDeps, "status" | "launcher">;

export interface HooksWiring {
  readonly engine: HookEngineImpl;
  /** Stage 5 rules (Task 7: `PreCommand`, then `PreMerge`), appended after the command rules. */
  readonly policies: readonly PolicyRule[];
  /** Stage 7 (Task 7: `PostCommand`, `PostMerge`), wrapped after the review link. */
  readonly wrapAfter: (hooks: ChainHooks) => ChainHooks;
  /** `fin` hooks' credentials (absent without 5A's credentials: such a hook fails `unavailable`). */
  readonly credentials?: HookCredentials;
  /** The session side (Task 6): the runtime's `hooks` and `sessionEvents`, the engine's notices. */
  readonly session: SessionHooks;
  /** The engine's session port, for the review bridge's notices (Task 8, `review.ts`): `session`. */
  readonly notices: SessionNotices;
  /** The reviews workflow `review.merge`'s `PreMerge` reads its item through (`startAgentReviews`). */
  bindReviews(reviews: ReviewItems): void;
  /** The plugin host's handlers (`startAgentReviews`, once the job engine runs). */
  bindPlugins(source: PluginHookSource): void;
  close(): Promise<void>;
}

export interface BuildHooksDeps extends Omit<HookEngineDeps, "process" | "credentials" | "notices" | "plugins"> {
  readonly config: HookConfig;
  /** The plugin layer, whose rows `config`'s plugin port reads (absent: no plugin handlers). */
  readonly plugins?: PluginLayer;
  readonly options?: HookServeOptions;
  readonly process?: HookEngineDeps["process"];
  /** 5A's credentials and revoke path, for `fin` hooks. */
  readonly credentials?: Omit<HookCredentialDeps, "serverData" | "log">;
  /** The agent sessions (a chain event's session: an agent principal's id is its session's). */
  readonly sessions?: { get(id: string): Session | undefined };
}

/** Removes the private temps of hook runs a crash left (`<server-data>/hooks/run/*`). */
export function sweepRunDirs(serverData: string): void {
  const runs = join(serverData, "hooks", "run");
  let names: string[];
  try {
    names = readdirSync(runs);
  } catch {
    return;
  }
  for (const n of names) rmSync(join(runs, n), { recursive: true, force: true, maxRetries: 3 });
}

export function buildHooks(d: BuildHooksDeps): HooksWiring {
  const { options, process, credentials: base, sessions, plugins: _plugins, ...rest } = d;
  // Before any hook runs: no run of this server is under way yet.
  sweepRunDirs(d.serverData);
  const credentials = base === undefined ? undefined : new HookCredentials({ ...base, serverData: d.serverData, ...(d.log === undefined ? {} : { log: d.log }) });
  credentials?.sweep();
  const session = new SessionHooks({ engine: () => engine, ...(d.log === undefined ? {} : { log: d.log }) });
  const engine = makeHookEngine({ ...rest, ...(d.plugins === undefined ? {} : { plugins: d.plugins }), notices: session, ...(credentials === undefined ? {} : { credentials }), process: { ...process, ...options } });
  const close = async () => {
    await engine.close();
    credentials?.close();
  };
  let reviews: ReviewItems | undefined;
  const chain = chainHooks({ engine, sessions: sessions ?? { get: () => undefined }, notices: session, reviews: () => reviews, ...(d.log === undefined ? {} : { log: d.log }) });
  return {
    engine,
    policies: [chain.preCommand, chain.preMerge],
    wrapAfter: chain.withHooksAfter,
    ...(credentials === undefined ? {} : { credentials }),
    session,
    notices: session,
    bindReviews: (r) => void (reviews = r),
    bindPlugins: (source) => d.plugins?.bind(source),
    close,
  };
}
