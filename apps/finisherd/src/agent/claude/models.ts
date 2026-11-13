// Claude Code's models: discovered from the installed CLI, never hardcoded (plan 7D Decision 11,
// spec §4.7). The cache holds one list per `(version, account hash)`:
// - the probe's unsigned `initialize` list seeds it (read from `claude-probe.json`, which the
//   probe owns and rewrites; this module never writes that file), and
// - every real instance's signed-in `initialize` list replaces it (`recordInstanceModels`, which
//   Task 7's session calls after its `initialize`), because an account may offer other models.
// Instance lists live in memory only: after a restart the probe file seeds the cache again and
// the next instance replaces it. The planner reads `models(version, accountHash)` (that account's
// list, or the version's newest while the account is unknown); a model the account lacks fails
// that session's first turn with the CLI's own error.
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
   * The models to plan against for `version` and the signed-in account `accountHash`: that
   * account's own instance list; else, only when the account is not known, the newest instance
   * list of the version (another account's list never answers for a known one); else the
   * probe's unsigned list for exactly that version; else empty (nothing discovered yet).
   */
  async models(version: string, accountHash?: string): Promise<readonly CliModel[]> {
    const exact = accountHash === undefined ? undefined : this.#instances.get(`${version}\0${accountHash}`);
    if (exact !== undefined) return exact.models;
    if (accountHash === undefined) {
      const newest = [...this.#instances.values()].filter((i) => i.version === version).at(-1);
      if (newest !== undefined) return newest.models;
    }
    const hash = probeFlagsHash(this.#os);
    const seed = (await readProbeCache(this.#serverData)).filter((e) => e.version === version && e.flagsHash === hash && e.os === this.#os).at(-1);
    return seed?.result.models ?? [];
  }
}

/**
 * Whether `model` is one the CLI offers: a discovered `value` (`default`, an alias) or a
 * `resolvedModel`. `default` always is. Before anything is discovered any non-empty id passes:
 * the CLI's own first-turn error is mapped to `MODEL_UNKNOWN` (Decision 11).
 */
export const claudeModelOffered = (models: readonly CliModel[], model: string): boolean =>
  model !== "" && (models.length === 0 || model === "default" || models.some((m) => m.id === model || m.resolvedModel === model));

/** What planning knows of Claude Code: `undefined` when it is not usable here, else its models or the version it is too old at. */
export type ClaudeAccess = { readonly models: readonly CliModel[] } | { readonly tooOld: { readonly version?: string; readonly floor: string } } | undefined;

/** Planning's view of `info`: absent unless detected and enabled; too old below the floor; else the models for its version and account. */
export async function claudeAccess(info: Pick<ProviderInfo, "detected" | "enabled" | "supported" | "version" | "floor" | "accountHash">, cache: ClaudeModelCache): Promise<ClaudeAccess> {
  if (!claudeHarnessAvailable(info)) return undefined;
  if (info.supported === false) return { tooOld: { ...(info.version === undefined ? {} : { version: info.version }), floor: info.floor ?? "" } };
  return { models: info.version === undefined ? [] : await cache.models(info.version, info.accountHash) };
}

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
