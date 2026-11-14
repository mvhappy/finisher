// Codex's models and the planner (plan 7E Task 9, Decisions 10 and 11, spec §4.7): discovered from
// the bundled app-server's `model/list` (the probe seeds the cache, each instance replaces it),
// accepted by `model` or `id`, never hardcoded; the account decides the cost. Synthetic ids.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import type { ProviderInfo } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BUNDLED_CATALOGUE, Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { type CliModel, discoverCodexModels, parseCodexModelPage } from "../src/agent/catalogue/cli-models.ts";
import { type CodexAccess, CodexModelCache, codexAccess, codexUsageCost, withCodexPricing } from "../src/agent/codex/models.ts";
import { codexProbeCachePath, codexProbeFlagsHash } from "../src/agent/codex/probe.ts";
import { sessionPlanner } from "../src/agent/planner.ts";
import { BUNDLED_ROLES, type Role } from "../src/agent/roles.ts";
import { cataloguePricing, type StartRequest } from "../src/agent/runtime-deps.ts";
import { codexWiring } from "../src/agent/wiring.ts";
import { agentEntries, agentJournal, flush, openStores, P1, session, usage } from "./support/agent-journal.ts";
import { PEOPLE } from "./support/runtime-doubles.ts";

const PAGE_1 = {
  data: [
    { id: "synth-a", model: "synth-a", displayName: "Synth A", isDefault: true, hidden: false },
    { id: "synth-hidden", model: "synth-hidden", displayName: "Hidden", isDefault: false, hidden: true },
    { id: "synth-slug", model: "synth-b-2026", displayName: "Synth B", isDefault: false, hidden: false },
  ],
  nextCursor: "c2",
};
const PAGE_2 = { data: [{ id: "synth-c", model: "synth-c", displayName: "Synth C", isDefault: false, hidden: false }], nextCursor: null };
const LIST: readonly CliModel[] = [
  { id: "synth-a", displayName: "Synth A", isDefault: true },
  { id: "synth-b-2026", alias: "synth-slug", displayName: "Synth B", isDefault: false },
  { id: "synth-c", displayName: "Synth C", isDefault: false },
];

/** A priced catalogue: only `openai/synth-a` and `openai/synth-slug`'s model is listed with a price. */
const PRICES = {
  get: (id: string) =>
    ({ "openai/synth-a": { id, window: 400_000, maxOutput: 8000, price: { input: 2_000_000, output: 10_000_000, cacheRead: 200_000, cacheWrite: 0 }, tools: true, thinking: true, images: true, openWeights: false } })[id],
};
const catalogue = (extra?: Parameters<typeof Catalogue.load>[1] extends infer O ? (O extends { extra?: infer E } ? E : never) : never) =>
  Catalogue.load(BUNDLED_CATALOGUE, { prices: PRICES as never, ...(extra === undefined ? {} : { extra }) });

let dir: string;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-codex-models-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const start = (over: Partial<StartRequest> = {}): StartRequest => ({ projectId: P1, prompt: "go", harness: "codex", ...over }) as StartRequest;
const failure = async (p: Promise<unknown>) => (await p.then(() => undefined, (e: unknown) => e)) as { code?: string; details?: Record<string, unknown> } | undefined;

type Account = { authMethod?: string; planType?: string };
function planner(o: { harnesses?: string[]; access?: () => Promise<CodexAccess>; roles?: ReadonlyMap<string, Role>; extra?: Parameters<typeof catalogue>[0] } = {}) {
  return sessionPlanner({
    roles: o.roles ?? BUNDLED_ROLES,
    catalogue: catalogue(o.extra),
    harnesses: () => new Set((o.harnesses ?? ["builtin", "codex"]) as never[]),
    hasKey: () => false,
    local: () => undefined,
    localModels: async () => [],
    ...(o.access === undefined ? {} : { codex: o.access }),
  });
}
const access = (a: Account = { authMethod: "chatgpt", planType: "plus" }, models: readonly CliModel[] = LIST) => async (): Promise<CodexAccess> => ({ models, ...a });
const withDefault = (model: string | undefined): ReadonlyMap<string, Role> =>
  new Map([...BUNDLED_ROLES].map(([k, r]) => [k, { ...r, models: { ...r.models, ...(model === undefined ? { codex: undefined } : { codex: model }) } } as Role]));

/** The probe's cache file as `probeCodex` leaves it. */
function seedProbe(version: string, models: CliModel[], over: { os?: NodeJS.Platform; flagsHash?: string } = {}): void {
  const file = codexProbeCachePath(dir);
  mkdirSync(dirname(file), { recursive: true });
  const result = { version, sandbox: "read-only", approval: "on-request", models };
  writeFileSync(file, JSON.stringify({ entries: [{ version, flagsHash: over.flagsHash ?? codexProbeFlagsHash(over.os ?? process.platform), os: over.os ?? process.platform, result }] }));
}

describe("models from the app-server", () => {
  test("models_from_model_list: paged, hidden skipped, `model` and `id` both kept", async () => {
    const seen: unknown[] = [];
    const pages = [PAGE_1, PAGE_2];
    const models = await discoverCodexModels(async (method, params) => {
      seen.push([method, params]);
      return pages[seen.length - 1];
    });
    expect(seen).toEqual([["model/list", {}], ["model/list", { cursor: "c2" }]]);
    expect(models.map((m) => m.id)).toEqual(["synth-a", "synth-b-2026", "synth-c"]);
    expect(models.find((m) => m.id === "synth-b-2026")?.alias).toBe("synth-slug");
    expect(models.find((m) => m.id === "synth-a")?.alias).toBeUndefined();
    expect(models.some((m) => m.id === "synth-hidden" || m.alias === "synth-hidden")).toBe(false);
    expect(parseCodexModelPage(PAGE_2)).toEqual({ models: [{ id: "synth-c", displayName: "Synth C", isDefault: false }], nextCursor: null });
  });

  test("models_from_model_list: the probe seeds the cache, each instance replaces it per version and account", async () => {
    seedProbe("9.9.9", [{ id: "synth-unsigned", displayName: "U", isDefault: true, alias: "synth-u-slug" }]);
    const cache = new CodexModelCache(dir);
    const ids = async (v: string, a?: string) => (await cache.models(v, a)).map((m) => m.id);
    expect(await ids("9.9.9", "acct-a")).toEqual(["synth-unsigned"]);
    expect((await cache.models("9.9.9"))[0]?.alias).toBe("synth-u-slug");
    cache.recordInstanceModels("9.9.9", "acct-a", LIST);
    cache.recordInstanceModels("9.9.9", "acct-b", [{ id: "synth-b-only", displayName: "B", isDefault: true }]);
    expect(await ids("9.9.9", "acct-a")).toEqual(["synth-a", "synth-b-2026", "synth-c"]);
    expect(await ids("9.9.9", "acct-b")).toEqual(["synth-b-only"]);
    expect(await ids("9.9.9", "acct-c")).toEqual(["synth-unsigned"]);
    expect(await ids("9.9.9")).toEqual(["synth-b-only"]);
    expect(await ids("8.8.8", "acct-a")).toEqual([]);
    expect(await new CodexModelCache(dir).models("9.9.9", "acct-a").then((m) => m.map((x) => x.id))).toEqual(["synth-unsigned"]);
  });

  test("models_from_model_list: the planner sees the detected version's and account's list", async () => {
    const info = (over: Partial<ProviderInfo>): ProviderInfo => ({ harness: "codex", detected: true, enabled: true, signedIn: true, version: "9.9.9", accountHash: "aaaaaaaa", authMethod: "chatgpt", capabilities: {}, ...over }) as never;
    const w = codexWiring(async () => info({}), dir);
    w.cache.recordInstanceModels("9.9.9", "aaaaaaaa", [{ id: "synth-a", displayName: "A", isDefault: true }]);
    w.cache.recordInstanceModels("9.9.9", "bbbbbbbb", [{ id: "synth-b", displayName: "B", isDefault: true }]);
    const p = planner({ access: w.planning });
    expect(await p.plan(start({ model: "synth-a" }), PEOPLE)).toMatchObject({ harness: "codex", provider: "openai", model: "synth-a", weights: "closed" });
    expect(await failure(p.plan(start({ model: "synth-b" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
    for (const over of [{ enabled: false }, { detected: false }]) {
      expect(await codexAccess(info(over), w.cache)).toBeUndefined();
    }
    expect(await failure(sessionPlanner({ roles: BUNDLED_ROLES, catalogue: catalogue(), harnesses: () => new Set(["builtin", "codex"] as never[]), hasKey: () => false, local: () => undefined, localModels: async () => [], codex: async () => codexAccess(info({ enabled: false }), w.cache) }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN", details: { harness: "codex" } });
    expect(await codexAccess(info({ problem: { code: "PROVIDER_ACCOUNT_CHANGED", message: "x" } }), w.cache)).toMatchObject({ accountChanged: { accountHash: "aaaaaaaa" } });
    expect(await failure(planner({ access: async () => codexAccess(info({ problem: { code: "PROVIDER_ACCOUNT_CHANGED", message: "x" } }), w.cache) }).plan(start(), PEOPLE))).toMatchObject({ code: "PROVIDER_ACCOUNT_CHANGED" });
    expect(await failure(planner({ access: async () => codexAccess(info({ version: "9.9.8", problem: { code: "PROVIDER_VERSION", message: "x" } }), w.cache) }).plan(start(), PEOPLE))).toMatchObject({ code: "PROVIDER_VERSION", details: { harness: "codex", version: "9.9.8" } });
  });

  test("unknown_model_refused: MODEL_UNKNOWN {harness: codex}; a listed model or id is accepted", async () => {
    const p = planner({ access: access() });
    for (const model of ["synth-a", "synth-b-2026", "synth-c"]) {
      expect(await p.plan(start({ model }), PEOPLE)).toMatchObject({ harness: "codex", provider: "openai", model, weights: "closed" });
    }
    // The entry's `id` plans as the `model` turn/start takes (pricing, journal and the session use that).
    expect(await p.plan(start({ model: "synth-slug" }), PEOPLE)).toMatchObject({ model: "synth-b-2026" });
    for (const model of ["synth-hidden", "synth-missing", "openai/synth-a", ""]) {
      expect(await failure(p.plan(start({ model }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN", details: { harness: "codex", model, reason: "notOffered" } });
    }
  });

  test("unknown_model_refused: nothing discovered yet accepts a named id; no default then", async () => {
    const p = planner({ access: access({ authMethod: "chatgpt" }, []), roles: withDefault(undefined) });
    expect(await p.plan(start({ model: "synth-new" }), PEOPLE)).toMatchObject({ model: "synth-new" });
    expect(await failure(p.plan(start(), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN", details: { harness: "codex", reason: "noDefault" } });
  });

  test("role_default_or_is_default: models.codex when the role names one, else the list's isDefault", async () => {
    const p = planner({ access: access(), roles: withDefault("synth-c") });
    expect(await p.plan(start(), PEOPLE)).toMatchObject({ model: "synth-c" });
    expect(await planner({ access: access(), roles: withDefault(undefined) }).plan(start(), PEOPLE)).toMatchObject({ model: "synth-a" });
    // The bundled roles name none for Codex: the list decides.
    for (const role of BUNDLED_ROLES.values()) expect(role.models.codex).toBeUndefined();
    expect(await planner({ access: access() }).plan(start(), PEOPLE)).toMatchObject({ harness: "codex", provider: "openai", model: "synth-a" });
    // An explicit model beats both.
    expect(await p.plan(start({ model: "synth-slug" }), PEOPLE)).toMatchObject({ model: "synth-b-2026" });
    // A role default the account does not offer is refused, not replaced.
    expect(await failure(planner({ access: access(), roles: withDefault("synth-gone") }).plan(start(), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN", details: { model: "synth-gone" } });
    // No default flagged and none named.
    expect(await failure(planner({ access: access({ authMethod: "chatgpt" }, [{ id: "x", displayName: "X", isDefault: false }]), roles: withDefault(undefined) }).plan(start(), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN", details: { reason: "noDefault" } });
  });
});

describe("the account decides the cost", () => {
  const tokens = { ...ZERO_USAGE, input: 1_000_000, output: 100_000 };

  test("plan_account_priced_or_shown: a ChatGPT plan is `subscription`: tokens charged, money shown from OpenRouter's price, never charged without chargePlanMoney", () => {
    const cat = catalogue();
    const shown = 2_000_000 + 1_000_000;
    expect(codexUsageCost(cat, { authMethod: "chatgpt", chargePlanMoney: false }, "synth-a", tokens)).toEqual({ costBasis: "subscription", costMicros: shown });
    // A model OpenRouter does not list: nothing to show, still a subscription.
    expect(codexUsageCost(cat, { authMethod: "chatgpt", chargePlanMoney: false }, "synth-c", tokens)).toEqual({ costBasis: "subscription" });
    // The owner chose to charge plan money: reported money, no subscription basis.
    expect(codexUsageCost(cat, { authMethod: "chatgpt", chargePlanMoney: true }, "synth-a", tokens)).toEqual({ costMicros: shown, costBasis: "reported" });
    // Never a usage without a basis for a plan account (the planner refuses this start under chargePlanMoney).
    expect(codexUsageCost(cat, { authMethod: "chatgpt", chargePlanMoney: true }, "synth-c", tokens)).toEqual({ costBasis: "subscription" });
    // And the runtime's price for the same tokens is the shown figure.
    expect(withCodexPricing(cataloguePricing(cat)).price({ provider: "openai", model: "synth-a" } as never, tokens, shown)).toEqual({ micros: shown, basis: "reported" });
  });

  test("plan_account_priced_or_shown: an API key carries no basis; the runtime prices it as `priced`", () => {
    const cat = catalogue();
    expect(codexUsageCost(cat, { authMethod: "apiKey", chargePlanMoney: true }, "synth-a", tokens)).toEqual({});
    const pricing = withCodexPricing(cataloguePricing(cat));
    expect(pricing.price({ provider: "openai", model: "synth-a" } as never, tokens, undefined)).toEqual({ micros: 3_000_000, basis: "priced" });
  });

  test("plan_account_priced_or_shown: bedrock, gcp and anything else fail closed", async () => {
    for (const authMethod of ["amazonBedrock", "gcp", "other"]) {
      expect(() => codexUsageCost(catalogue(), { authMethod, chargePlanMoney: false }, "synth-a", tokens)).toThrow(/Finisher does not support/);
      expect(await failure(planner({ access: access({ authMethod }) }).plan(start({ model: "synth-a" }), PEOPLE))).toMatchObject({ code: "PROVIDER_AUTH", details: { harness: "codex", reason: "unsupportedAccount", authMethod } });
    }
  });

  test("plan_account_priced_or_shown: a plan account starts on an unpriced model (tokens only)", async () => {
    expect(await planner({ access: access({ authMethod: "chatgpt", planType: "pro" }) }).plan(start({ model: "synth-c" }), PEOPLE)).toMatchObject({ model: "synth-c", provider: "openai" });
    expect(withCodexPricing(cataloguePricing(catalogue())).known("openai", "synth-c")).toBe(true);
    expect(withCodexPricing(cataloguePricing(catalogue())).known("openrouter", "synth/unlisted")).toBe(false);
  });

  test("api_key_unpriced_is_pricing_unknown: refused at start unless OpenRouter lists it or the owner prices it in agents.models", async () => {
    const key = { authMethod: "apiKey" };
    expect(await planner({ access: access(key) }).plan(start({ model: "synth-a" }), PEOPLE)).toMatchObject({ model: "synth-a" });
    for (const model of ["synth-c", "synth-slug", "synth-b-2026"]) {
      expect(await failure(planner({ access: access(key) }).plan(start({ model }), PEOPLE))).toMatchObject({ code: "PRICING_UNKNOWN", details: { provider: "openai", model: model === "synth-slug" ? "synth-b-2026" : model } });
    }
    // An owner price on the canonical id covers the alias too.
    expect(await planner({ access: access(key), extra: [{ id: "openai/synth-b-2026", window: 1000, price: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0 } }] }).plan(start({ model: "synth-slug" }), PEOPLE)).toMatchObject({ model: "synth-b-2026" });
    const priced = [{ id: "openai/synth-c", window: 100_000, price: { input: 1_000_000, output: 2_000_000, cacheRead: 0, cacheWrite: 0 } }];
    expect(await planner({ access: access(key), extra: priced }).plan(start({ model: "synth-c" }), PEOPLE)).toMatchObject({ model: "synth-c" });
    // An agents.models entry without a price does not help.
    expect(await failure(planner({ access: access(key), extra: [{ id: "openai/synth-c", window: 100_000 }] }).plan(start({ model: "synth-c" }), PEOPLE))).toMatchObject({ code: "PRICING_UNKNOWN" });
    // The owner's price is the one the runtime charges.
    const cat = catalogue(priced);
    expect(withCodexPricing(cataloguePricing(cat)).price({ provider: "openai", model: "synth-c" } as never, tokens, undefined)).toEqual({ micros: 1_200_000, basis: "priced" });
  });
});

describe("review fixes", () => {
  const info = (over: Partial<ProviderInfo>): ProviderInfo => ({ harness: "codex", detected: true, enabled: true, signedIn: true, version: "9.9.9", accountHash: "aaaaaaaa", authMethod: "apiKey", capabilities: {}, ...over }) as never;
  const viaInfo = (i: ProviderInfo, chargePlanMoney = false) => {
    const w = codexWiring(async () => i, dir, () => chargePlanMoney);
    w.cache.recordInstanceModels("9.9.9", "aaaaaaaa", LIST);
    return planner({ access: w.planning });
  };

  test("signed_out_or_unknown_account_is_refused: no price check can be skipped by an unset authMethod", async () => {
    for (const over of [{ signedIn: false, authMethod: undefined }, { signedIn: null, authMethod: undefined }, { signedIn: true, authMethod: undefined }]) {
      const e = await failure(viaInfo(info(over as never)).plan(start({ model: "synth-c" }), PEOPLE));
      expect(e).toMatchObject({ code: "PROVIDER_AUTH", details: { harness: "codex", reason: "signedOut" } });
    }
    // The planner itself fails closed on an access without a method.
    expect(await failure(planner({ access: access({}) }).plan(start({ model: "synth-c" }), PEOPLE))).toMatchObject({ code: "PROVIDER_AUTH" });
  });

  test("detection_problem_refuses: any other detection problem stops new sessions", async () => {
    for (const code of ["PROVIDER_AUTH", "PROVIDER_PROTOCOL", "PROVIDER_UNAVAILABLE"]) {
      const e = await failure(viaInfo(info({ authMethod: "chatgpt", problem: { code, message: "x", fix: "y" } } as never)).plan(start({ model: "synth-a" }), PEOPLE));
      expect(e).toMatchObject({ code, details: { harness: "codex" } });
    }
  });

  test("api_key_check_runs_for_anything_but_a_plan", async () => {
    expect(await failure(planner({ access: access({ authMethod: "chatgpt" }) }).plan(start({ model: "synth-c" }), PEOPLE))).toBeUndefined();
    expect(await failure(viaInfo(info({})).plan(start({ model: "synth-c" }), PEOPLE))).toMatchObject({ code: "PRICING_UNKNOWN" });
  });

  test("charge_plan_money_unpriced_is_pricing_unknown: refused at start; priced or unset still plans", async () => {
    expect(await failure(viaInfo(info({ authMethod: "chatgpt" }), true).plan(start({ model: "synth-c" }), PEOPLE))).toMatchObject({ code: "PRICING_UNKNOWN", details: { provider: "openai", model: "synth-c" } });
    expect(await viaInfo(info({ authMethod: "chatgpt" }), true).plan(start({ model: "synth-a" }), PEOPLE)).toMatchObject({ model: "synth-a" });
    expect(await viaInfo(info({ authMethod: "chatgpt" }), false).plan(start({ model: "synth-c" }), PEOPLE)).toMatchObject({ model: "synth-c" });
  });

  test("probe_seed_needs_the_flags_hash_and_the_os", async () => {
    const one = [{ id: "synth-unsigned", displayName: "U", isDefault: true }];
    seedProbe("9.9.9", one, { flagsHash: "ffffffffffffffff" });
    expect(await new CodexModelCache(dir).models("9.9.9")).toEqual([]);
    seedProbe("9.9.9", one, { os: "win32" });
    expect(await new CodexModelCache(dir).models("9.9.9")).toEqual([]);
    expect((await new CodexModelCache(dir, "win32").models("9.9.9")).map((m) => m.id)).toEqual(["synth-unsigned"]);
    seedProbe("9.9.9", one);
    expect((await new CodexModelCache(dir).models("9.9.9")).map((m) => m.id)).toEqual(["synth-unsigned"]);
  });

  test("owner_price_beats_the_listing, and the listing prices when the owner's entry has none", () => {
    const owner = { id: "openai/synth-a", window: 1000, price: { input: 5_000_000, output: 0, cacheRead: 0, cacheWrite: 0 } };
    expect(catalogue([owner]).describe("openai", "synth-a")).toMatchObject({ source: "settings", price: { input: 5_000_000 } });
    expect(catalogue([{ id: "openai/synth-a", window: 1000 }]).describe("openai", "synth-a")).toMatchObject({ source: "settings", price: { input: 2_000_000 } });
    expect(catalogue().describe("openai", "synth-a")).toMatchObject({ source: "openrouter", price: { input: 2_000_000 } });
  });
});

describe("the harness is present only when registered", () => {
  test("planner_refuses_codex_until_registered: no adapter is HARNESS_UNKNOWN, with or without models", async () => {
    expect(await failure(planner({ harnesses: ["builtin"], access: access() }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN", details: { harness: "codex" } });
    expect(await failure(planner({ access: async () => undefined }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN", details: { harness: "codex" } });
    expect(await failure(planner().plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
    // The wiring registers no Codex adapter before Task 10: the same answer through it.
    const w = codexWiring(async () => ({ harness: "codex", detected: true, enabled: true, signedIn: true, authMethod: "chatgpt", version: "9.9.9", capabilities: {} }) as never, dir);
    expect(await failure(planner({ harnesses: ["builtin", "claudeCode"], access: w.planning }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
  });
});

describe("diagnostics name the model", () => {
  test("journal_names_model_by_table_or_hash: the ids the pinned app-server lists are named, others are `other` + modelHash", async () => {
    const s = openStores(dir, { diagnostics: "on", training: "on" });
    try {
      const { j } = agentJournal(s);
      const runId = "01JR0000000000000000000001";
      const p = planner({ access: access({ authMethod: "chatgpt" }, []) });
      const of2 = async (id: string, model: string) => {
        const plan = await p.plan(start({ model }), PEOPLE);
        return session({ id, harness: plan.harness, provider: plan.provider, model: plan.model, weights: plan.weights });
      };
      const known = await of2("01JS000000000000000000000A", "gpt-6.1-sol");
      const older = await of2("01JS000000000000000000000C", "gpt-5.5");
      const unknown = await of2("01JS000000000000000000000B", "gpt-synth-unlisted-9");
      for (const ss of [known, older, unknown]) j.turn({ session: ss, runId, usage: usage() });
      await flush(s);
      const turns = agentEntries(dir, "events").filter((e) => e.kind === "agent.turn");
      const of = (id: string) => turns.find((t) => t.ctx?.actor?.session === id)!.payload;
      expect(of(known.id)).toMatchObject({ harness: "codex", provider: "openai", model: "gpt-6.1-sol" });
      expect(of(known.id).modelHash).toBeUndefined();
      expect(of(older.id)).toMatchObject({ harness: "codex", provider: "openai", model: "gpt-5.5" });
      expect(of(older.id).modelHash).toBeUndefined();
      expect(of(unknown.id)).toMatchObject({ harness: "codex", provider: "openai", model: "other" });
      expect(of(unknown.id).modelHash).toMatch(/\S/);
      expect(JSON.stringify(turns)).not.toContain("gpt-synth-unlisted-9");
    } finally {
      await s.close();
    }
  });
});
