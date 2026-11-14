// Codex's models and what an account costs (plan 7E Task 9, Decisions 10 and 11, spec §4.7):
// discovered from the bundled app-server's `model/list` (`discoverCodexModels`), never hardcoded.
// - The cache holds one list per `(version, account hash)`: the probe's unsigned list seeds it
//   (read from `codex-probe.json`, which the probe owns; this module never writes that file) and
//   every real instance's signed-in list replaces it (`recordInstanceModels`, Task 10's session
//   after its `initialize`), because an account may offer other models. Instance lists live in
//   memory only.
// - A model is accepted by its `model` (`CliModel.id`) or its entry `id` (`alias`); hidden ones
//   are never listed (`parseCodexModelPage`).
// - Cost (Decision 11): Codex reports tokens only. A ChatGPT plan is `subscription` (tokens
//   charged, the money shown from OpenRouter's `openai/<model>` price, charged only under
//   `agents.codex.chargePlanMoney`); an API key carries no basis and is priced from the same
//   listing (`priced`), refused `PRICING_UNKNOWN` at start without a price (the planner); any
//   other account (Bedrock, GCP) fails closed.
import type { ProviderInfo } from "@finisher/contracts";
import { finError } from "../../errors.ts";
import type { Catalogue, Usage } from "../catalogue/catalogue.ts";
import type { CliModel } from "../catalogue/cli-models.ts";
import type { Pricing } from "../runtime-deps.ts";
import { codexProbeFlagsHash, readCodexProbeCache } from "./probe.ts";

export class CodexModelCache {
  readonly #serverData: string;
  readonly #os: NodeJS.Platform;
  /** `${version}\0${accountHash}` -> list, oldest first (a re-record moves the key to the end). */
  readonly #instances = new Map<string, { version: string; models: readonly CliModel[] }>();

  constructor(serverData: string, os: NodeJS.Platform = process.platform) {
    this.#serverData = serverData;
    this.#os = os;
  }

  /** Replaces the list for `(version, accountHash)` with a signed-in instance's `model/list`. */
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
    const hash = codexProbeFlagsHash(this.#os);
    const seed = (await readCodexProbeCache(this.#serverData)).filter((e) => e.version === version && e.flagsHash === hash && e.os === this.#os).at(-1);
    return seed?.result.models ?? [];
  }
}

/** Whether `model` is one the list names: its `model` (`id`) or its entry `id` (`alias`). */
export const codexModelListed = (models: readonly CliModel[], model: string): boolean => models.some((m) => m.id === model || m.alias === model);

/** The list's default model, if it flags one. */
export const codexListDefault = (models: readonly CliModel[]): string | undefined => models.find((m) => m.isDefault)?.id;

/**
 * What planning knows of Codex: `undefined` when it is not usable here, else its models and the
 * account's method, a version that is not the pin, or an account change waiting for a person.
 */
export type CodexAccess =
  | { readonly models: readonly CliModel[]; readonly authMethod?: string; readonly planType?: string; readonly chargePlanMoney?: boolean }
  | { readonly refused: { readonly code: string; readonly message: string; readonly fix?: string } }
  | { readonly signedOut: { readonly signedIn: boolean | null } }
  | { readonly versionMismatch: { readonly version?: string } }
  | { readonly accountChanged: { readonly accountHash?: string } }
  | undefined;

/** Sessions may start on Codex only when it is found and turned on (`agents.codex.enabled`). */
export const codexHarnessAvailable = (info: Pick<ProviderInfo, "detected" | "enabled">): boolean => info.detected && info.enabled === true;

/**
 * Planning's view of `info`: absent unless detected and enabled; a version other than the pin
 * and an unconfirmed account change (`PROVIDER_ACCOUNT_CHANGED`) stop new sessions, as does any
 * detection problem and anything but a signed-in account of a known method; else the
 * models for its version and account, with the account's method.
 */
export async function codexAccess(info: Pick<ProviderInfo, "detected" | "enabled" | "signedIn" | "version" | "accountHash" | "authMethod" | "planType" | "problem">, cache: CodexModelCache, chargePlanMoney = false): Promise<CodexAccess> {
  if (!codexHarnessAvailable(info)) return undefined;
  if (info.problem?.code === "PROVIDER_VERSION") return { versionMismatch: info.version === undefined ? {} : { version: info.version } };
  if (info.problem?.code === "PROVIDER_ACCOUNT_CHANGED") return { accountChanged: info.accountHash === undefined ? {} : { accountHash: info.accountHash } };
  // Any other detection problem (noAccountLabel, unsupportedAccount, a protocol error) refuses.
  if (info.problem !== undefined) return { refused: info.problem };
  // Without a signed-in account of a known method no price check could be made: never plan.
  if (info.signedIn !== true || info.authMethod === undefined) return { signedOut: { signedIn: info.signedIn ?? null } };
  return {
    ...(chargePlanMoney ? { chargePlanMoney: true } : {}),
    models: info.version === undefined ? [] : await cache.models(info.version, info.accountHash),
    ...(info.authMethod === undefined ? {} : { authMethod: info.authMethod }),
    ...(info.planType === undefined ? {} : { planType: info.planType }),
  };
}

/** `PROVIDER_AUTH {reason: unsupportedAccount}`: Bedrock, GCP or anything but a plan or an API key. */
export const unsupportedAccount = (authMethod: string): ReturnType<typeof finError> =>
  finError(
    "PROVIDER_AUTH",
    "Codex is signed in with an account type Finisher does not support (only a ChatGPT plan or an OpenAI API key)",
    { harness: "codex", reason: "unsupportedAccount", authMethod },
    "sign it in again with the commands `finisherd provider signin codex` prints",
  );

/**
 * `base` with Codex's `openai` provider known: a plan account needs no price (its tokens are
 * charged, its money only shown), and an API-key account without one never gets this far (the
 * planner refuses it `PRICING_UNKNOWN` first). The built-in agent never uses `openai`.
 */
export function withCodexPricing(base: Pricing): Pricing {
  return { ...base, known: (provider, model) => provider === "openai" || base.known(provider, model) };
}

/**
 * What a Codex usage event says about money (Decision 11), for the adapter (Task 13):
 * - ChatGPT plan: `costBasis: "subscription"` with `costMicros` from OpenRouter's `openai/<model>`
 *   price where it lists one (shown, never charged); under `chargePlanMoney` the same money is
 *   `reported` and charged;
 * - API key: nothing, so the runtime prices the tokens from the same listing (`priced`);
 * - any other account fails closed (`PROVIDER_AUTH`).
 */
export function codexUsageCost(
  catalogue: Pick<Catalogue, "projectedMicros">,
  account: { readonly authMethod: string | undefined; readonly chargePlanMoney: boolean },
  model: string,
  tokens: Usage,
): { costMicros?: number; costBasis?: "subscription" | "reported" } {
  if (account.authMethod === "apiKey") return {};
  if (account.authMethod !== "chatgpt") throw unsupportedAccount(String(account.authMethod));
  const shown = catalogue.projectedMicros("openai", model, tokens);
  // Never a usage without a basis for a plan account: with no price there is no money to charge.
  if (account.chargePlanMoney && shown !== null) return { costMicros: shown, costBasis: "reported" };
  return { costBasis: "subscription", ...(shown === null ? {} : { costMicros: shown }) };
}
