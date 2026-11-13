// The hooks as the server wires them (7F plan Task 4): one engine per server, built with the
// agent runtime (`agent/wiring.ts`), which closes it on shutdown. The event sources of Tasks 6–8
// add themselves here: the chain's policy rules and its after stage (Task 7), the session port
// (Task 6), the review and job listeners (Task 8); the plugin layer's port (Task 10).
import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { ChainHooks } from "../chain/hooks.ts";
import type { PolicyRule } from "../chain/policy.ts";
import type { HookConfig } from "./config.ts";
import { type HookEngineDeps, type HookEngineImpl, makeHookEngine } from "./engine.ts";
import type { HookProcessDeps } from "./process.ts";

/** `serve`'s hook options (tests): the process's sandbox status or launcher. */
export type HookServeOptions = Pick<HookProcessDeps, "status" | "launcher">;

export interface HooksWiring {
  readonly engine: HookEngineImpl;
  /** Stage 5 rules (Task 7: `PreCommand`, `PreMerge`), appended after the command rules. */
  readonly policies: readonly PolicyRule[];
  /** Stage 7 (Task 7: `PostCommand`, `PostMerge`), wrapped after the review link. */
  readonly wrapAfter: (hooks: ChainHooks) => ChainHooks;
  close(): Promise<void>;
}

export interface BuildHooksDeps extends Omit<HookEngineDeps, "process"> {
  readonly config: HookConfig;
  readonly options?: HookServeOptions;
  readonly process?: HookEngineDeps["process"];
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
  const { options, process, ...rest } = d;
  // Before any hook runs: no run of this server is under way yet.
  sweepRunDirs(d.serverData);
  const engine = makeHookEngine({ ...rest, process: { ...process, ...options } });
  return { engine, policies: [], wrapAfter: (hooks) => hooks, close: () => engine.close() };
}
