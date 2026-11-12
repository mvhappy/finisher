// Claude Code's models: discovered from the installed CLI, never hardcoded (plan 7D Decision 11,
// spec §4.7). The cache holds one list per `(version, account hash)`:
// - the probe's unsigned `initialize` list seeds it (read from `claude-probe.json`, which the
//   probe owns and rewrites; this module never writes that file), and
// - every real instance's signed-in `initialize` list replaces it (`recordInstanceModels`, which
//   Task 7's session calls after its `initialize`), because an account may offer other models.
// Instance lists live in memory only: after a restart the probe file seeds the cache again and
// the next instance replaces it. The planner reads `models()` (the newest list for the probed
// version); a model the account lacks fails that session's first turn with the CLI's own error.
import type { ProviderInfo } from "@finisher/contracts";
import type { CliModel } from "../catalogue/cli-models.ts";
import type { Pricing } from "../runtime-deps.ts";
import { probeFlagsHash, readProbeCache } from "./probe.ts";

export class ClaudeModelCache {
  readonly #serverData: string;
  readonly #os: NodeJS.Platform;
  /** `${version}\0${accountHash}` -> list, oldest first (a re-record moves the key to the end). */
  readonly #instances = new Map<string, { version: string; models: readonly CliModel[] }>();

  constructor(serverData: string, os: NodeJS.Platform = process.platform) {
    this.#serverData = serverData;
    this.#os = os;
  }

  /** Replaces the list for `(version, accountHash)` with a signed-in instance's `initialize` list. */
  recordInstanceModels(version: string, accountHash: string, models: readonly CliModel[]): void {
    const key = `${version}\0${accountHash}`;
    this.#instances.delete(key);
    this.#instances.set(key, { version, models });
  }

  /**
   * The models to plan against: the newest instance list of the probed version, else the probe's
   * unsigned list, else the newest instance list of any version; empty when nothing is known yet.
   */
  async models(): Promise<readonly CliModel[]> {
    const hash = probeFlagsHash(this.#os);
    const seed = (await readProbeCache(this.#serverData)).filter((e) => e.flagsHash === hash && e.os === this.#os).at(-1);
    const instances = [...this.#instances.values()];
    const recorded = seed === undefined ? instances.at(-1) : instances.filter((i) => i.version === seed.version).at(-1);
    return recorded?.models ?? seed?.result.models ?? [];
  }
}

/** Whether `model` is one the CLI offers: a discovered `value` (`default`, an alias) or a `resolvedModel`. `default` always is. */
export const claudeModelOffered = (models: readonly CliModel[], model: string): boolean =>
  model === "default" || models.some((m) => m.id === model || m.resolvedModel === model);

/** Sessions may start on Claude Code only when it is found and turned on (`agents.claudeCode.enabled`). */
export const claudeHarnessAvailable = (info: Pick<ProviderInfo, "detected" | "enabled">): boolean => info.detected && info.enabled === true;

/**
 * `base` with Claude Code's `anthropic` provider known: the CLI reports its own cost, so it
 * needs no catalogue price and `PRICING_UNKNOWN` does not apply (Decision 11; `costBasis` per
 * Decision 15). The built-in agent never uses the `anthropic` provider.
 */
export function withClaudePricing(base: Pricing): Pricing {
  return { ...base, known: (provider, model) => provider === "anthropic" || base.known(provider, model) };
}
