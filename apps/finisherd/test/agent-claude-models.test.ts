// Claude Code's models and the planner (plan 7D Task 6, Decision 11, spec §4.7): discovered from the
// installed CLI's `initialize` list, never hardcoded; a model is accepted by `value` or
// `resolvedModel`; no price needed (the CLI reports its cost); the harness is absent unless
// detected and enabled. Synthetic ids throughout.
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { BUNDLED_CATALOGUE, Catalogue, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { type CliModel, parseClaudeInitialize } from "../src/agent/catalogue/cli-models.ts";
import type { ProviderInfo } from "@finisher/contracts";
import { ClaudeModelCache, claudeHarnessAvailable, claudeModelOffered, withClaudePricing } from "../src/agent/claude/models.ts";
import { probeCachePath, probeFlagsHash } from "../src/agent/claude/probe.ts";
import { sessionPlanner } from "../src/agent/planner.ts";
import { BUNDLED_ROLES } from "../src/agent/roles.ts";
import { cataloguePricing, type StartRequest } from "../src/agent/runtime-deps.ts";
import { claudeWiring } from "../src/agent/wiring.ts";
import { PEOPLE } from "./support/runtime-doubles.ts";
import { agentEntries, agentJournal, flush, openStores, P1, session, usage } from "./support/agent-journal.ts";

const INIT = {
  type: "control_response",
  response: {
    subtype: "success",
    request_id: "r1",
    response: {
      models: [
        { value: "default", displayName: "Default (recommended)", description: "x", resolvedModel: "claude-synth-main-1" },
        { value: "synth-big", displayName: "Big", resolvedModel: "claude-synth-big-2" },
        { value: "synth-small", displayName: "Small" },
      ],
    },
  },
};

let dir: string;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "agent-claude-models-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const start = (over: Partial<StartRequest> = {}): StartRequest => ({ projectId: P1, prompt: "go", harness: "claudeCode", ...over }) as StartRequest;

function planner(o: { harnesses?: string[]; models?: () => Promise<readonly CliModel[] | undefined> } = {}) {
  return sessionPlanner({
    roles: BUNDLED_ROLES,
    catalogue: Catalogue.load(BUNDLED_CATALOGUE),
    harnesses: () => new Set((o.harnesses ?? ["builtin", "claudeCode"]) as never[]),
    hasKey: () => false,
    local: () => undefined,
    localModels: async () => [],
    ...(o.models === undefined ? {} : { claude: async () => ((await o.models!()) === undefined ? undefined : { models: (await o.models!())! }) }),
  });
}
function sessionPlannerOver(claude: () => Promise<{ models: readonly CliModel[] }>) {
  return sessionPlanner({ roles: BUNDLED_ROLES, catalogue: Catalogue.load(BUNDLED_CATALOGUE), harnesses: () => new Set(["builtin", "claudeCode"] as never[]), hasKey: () => false, local: () => undefined, localModels: async () => [], claude });
}
const discovered = () => async () => parseClaudeInitialize(INIT);
const failure = async (p: Promise<unknown>) => (await p.then(() => undefined, (e: unknown) => e)) as { code?: string; details?: Record<string, unknown> } | undefined;

/** The probe's cache file as `probe()` leaves it, with `models` in the unsigned list. */
function seedProbe(version: string, models: CliModel[]): void {
  const hash = probeFlagsHash();
  const file = probeCachePath(dir);
  mkdirSync(dirname(file), { recursive: true });
  const result = { version, permissionPath: "stdio", tools: [], models, capabilities: [] };
  writeFileSync(file, JSON.stringify({ entries: [{ version, flagsHash: hash, os: process.platform, result }] }));
}

describe("models from the installed CLI", () => {
  test("models_from_initialize: default, aliases and resolvedModel are accepted", async () => {
    const models = parseClaudeInitialize(INIT);
    expect(models.map((m) => [m.id, m.resolvedModel])).toEqual([
      ["default", "claude-synth-main-1"],
      ["synth-big", "claude-synth-big-2"],
      ["synth-small", undefined],
    ]);
    const p = planner({ models: discovered() });
    for (const model of ["default", "synth-big", "synth-small", "claude-synth-big-2", "claude-synth-main-1"]) {
      expect(await p.plan(start({ model }), PEOPLE)).toMatchObject({ harness: "claudeCode", provider: "anthropic", model, weights: "closed" });
    }
    expect(claudeModelOffered(models, "claude-synth-small-3")).toBe(false);
  });

  test("models_from_initialize: the cache is seeded by the probe and replaced by each instance's signed-in list", async () => {
    seedProbe("9.9.9", [{ id: "default", displayName: "Default", isDefault: true }, { id: "synth-unsigned", displayName: "U", isDefault: false }]);
    const cache = new ClaudeModelCache(dir);
    const ids = async (v: string, a?: string) => (await cache.models(v, a)).map((m) => m.id);
    expect(await ids("9.9.9", "acct-a")).toEqual(["default", "synth-unsigned"]);
    cache.recordInstanceModels("9.9.9", "acct-a", parseClaudeInitialize(INIT));
    expect(await ids("9.9.9", "acct-a")).toEqual(["default", "synth-big", "synth-small"]);
    // Another account of the same version has its own list; another version does not touch it.
    cache.recordInstanceModels("9.9.9", "acct-b", [{ id: "synth-b", displayName: "B", isDefault: false }]);
    cache.recordInstanceModels("1.0.0", "acct-a", [{ id: "synth-old", displayName: "O", isDefault: false }]);
    expect(await ids("9.9.9", "acct-b")).toEqual(["synth-b"]);
    // An unknown account gets the version's newest list; a known account without one, the probe's.
    expect(await ids("9.9.9")).toEqual(["synth-b"]);
    expect(await ids("9.9.9", "acct-c")).toEqual(["default", "synth-unsigned"]);
    // A version the probe never saw and no instance ran: nothing known.
    expect(await ids("8.8.8", "acct-a")).toEqual([]);
    expect(await ids("1.0.0", "acct-a")).toEqual(["synth-old"]);
    expect(await new ClaudeModelCache(dir).models("9.9.9", "acct-a").then((m) => m.map((x) => x.id))).toEqual(["default", "synth-unsigned"]);
  });

  test("models_from_initialize: switching back to account A accepts A's model after B recorded a list without it", async () => {
    const cache = new ClaudeModelCache(dir);
    cache.recordInstanceModels("9.9.9", "acct-a", [{ id: "default", displayName: "D", isDefault: true }, { id: "opus", displayName: "Opus", isDefault: false }]);
    cache.recordInstanceModels("9.9.9", "acct-b", [{ id: "default", displayName: "D", isDefault: true }, { id: "sonnet", displayName: "Sonnet", isDefault: false }]);
    for (const [account, ok, refused] of [["acct-a", "opus", "sonnet"], ["acct-b", "sonnet", "opus"]] as const) {
      const p = sessionPlannerOver(async () => ({ models: await cache.models("9.9.9", account) }));
      expect(await p.plan(start({ model: ok }), PEOPLE)).toMatchObject({ model: ok });
      expect(await failure(p.plan(start({ model: refused }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
    }
  });

  test("models_from_initialize: nothing discovered yet accepts any non-empty id; a list refuses the rest", async () => {
    const cache = new ClaudeModelCache(dir);
    expect(await cache.models("9.9.9")).toEqual([]);
    const p = planner({ models: () => cache.models("9.9.9") });
    expect(await p.plan(start(), PEOPLE)).toMatchObject({ model: "default" });
    expect(await p.plan(start({ model: "synth-big" }), PEOPLE)).toMatchObject({ model: "synth-big" });
    expect(await failure(p.plan(start({ model: "" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
    cache.recordInstanceModels("9.9.9", "acct-a", parseClaudeInitialize(INIT));
    expect(await failure(planner({ models: () => cache.models("9.9.9", "acct-a") }).plan(start({ model: "synth-other" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
  });

  test("unknown_model_refused: MODEL_UNKNOWN {harness: claudeCode}", async () => {
    const e = await failure(planner({ models: discovered() }).plan(start({ model: "synth-missing" }), PEOPLE));
    expect(e).toMatchObject({ code: "MODEL_UNKNOWN", details: { harness: "claudeCode", model: "synth-missing", reason: "notOffered" } });
    // An OpenRouter-shaped id is not a Claude Code model either.
    expect(await failure(planner({ models: discovered() }).plan(start({ model: "anthropic/claude-sonnet-5.5" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
  });

  test("role_default_is_default: every bundled role plans `default`", async () => {
    for (const role of BUNDLED_ROLES.values()) expect(role.models.claudeCode).toBe("default");
    const plan = await planner({ models: discovered() }).plan(start(), PEOPLE);
    expect(plan).toMatchObject({ harness: "claudeCode", role: "editor", model: "default", provider: "anthropic" });
  });

  test("no_pricing_unknown_for_claude: the CLI reports its cost, so no catalogue price is needed", () => {
    const base = cataloguePricing(Catalogue.load(BUNDLED_CATALOGUE));
    expect(base.known("anthropic", "default")).toBe(false);
    const pricing = withClaudePricing(base);
    expect(pricing.known("anthropic", "default")).toBe(true);
    expect(pricing.known("anthropic", "synth-big")).toBe(true);
    expect(pricing.known("openrouter", "synth/unlisted")).toBe(false);
    const s = { provider: "anthropic", model: "default" } as never;
    expect(pricing.price(s, ZERO_USAGE, 1234)).toEqual({ micros: 1234, basis: "reported" });
  });
});

describe("the harness is present only when usable", () => {
  test("harness_absent_when_not_installed: HARNESS_UNKNOWN", async () => {
    // No adapter registered (Task 7), or the CLI not found/enabled: the models read says so.
    expect(await failure(planner({ harnesses: ["builtin"], models: discovered() }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN", details: { harness: "claudeCode" } });
    expect(await failure(planner({ models: async () => undefined }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN", details: { harness: "claudeCode" } });
    expect(await failure(planner().plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
    expect(claudeHarnessAvailable({ detected: false, enabled: true })).toBe(false);
  });

  test("harness_absent_when_not_installed: installed but disabled is absent too", async () => {
    expect(claudeHarnessAvailable({ detected: true, enabled: false })).toBe(false);
    expect(claudeHarnessAvailable({ detected: true })).toBe(false);
    expect(claudeHarnessAvailable({ detected: true, enabled: true })).toBe(true);
    expect(await failure(planner({ models: async () => (claudeHarnessAvailable({ detected: true, enabled: false }) ? [] : undefined) }).plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
  });
});

const INFO: ProviderInfo = { harness: "claudeCode", detected: true, enabled: true, supported: true, version: "9.9.9", floor: "2.1.314", capabilities: {} } as never;

describe("the wiring plans Claude Code", () => {
  const wired = (info: Partial<ProviderInfo>) => {
    const w = claudeWiring(async () => ({ ...INFO, ...info }) as ProviderInfo, dir);
    const p = sessionPlanner({ roles: BUNDLED_ROLES, catalogue: Catalogue.load(BUNDLED_CATALOGUE), harnesses: () => new Set(["builtin", "claudeCode"] as never[]), hasKey: () => false, local: () => undefined, localModels: async () => [], claude: w.planning });
    return { w, p };
  };

  test("wiring_enabled_disabled_not_installed: only a detected and enabled CLI plans", async () => {
    const on = wired({});
    on.w.cache.recordInstanceModels("9.9.9", "acct-a", parseClaudeInitialize(INIT));
    expect(await on.p.plan(start({ model: "synth-big" }), PEOPLE)).toMatchObject({ harness: "claudeCode", model: "synth-big" });
    expect(await failure(on.p.plan(start({ model: "synth-missing" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
    expect(await failure(wired({ enabled: false }).p.plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
    expect(await failure(wired({ detected: false }).p.plan(start(), PEOPLE))).toMatchObject({ code: "HARNESS_UNKNOWN" });
  });

  test("wiring_models_follow_the_detected_account: the version and account hash select the list", async () => {
    const a = wired({ accountHash: "aaaaaaaa" });
    a.w.cache.recordInstanceModels("9.9.9", "aaaaaaaa", [{ id: "opus", displayName: "O", isDefault: false }]);
    a.w.cache.recordInstanceModels("9.9.9", "bbbbbbbb", [{ id: "sonnet", displayName: "S", isDefault: false }]);
    expect(await a.p.plan(start({ model: "opus" }), PEOPLE)).toMatchObject({ model: "opus" });
    expect(await failure(a.p.plan(start({ model: "sonnet" }), PEOPLE))).toMatchObject({ code: "MODEL_UNKNOWN" });
  });

  test("wiring_below_the_floor: PROVIDER_VERSION with the floor", async () => {
    const old = wired({ version: "2.1.300", supported: false });
    expect(await failure(old.p.plan(start(), PEOPLE))).toMatchObject({ code: "PROVIDER_VERSION", details: { harness: "claudeCode", version: "2.1.300", floor: "2.1.314" } });
  });

  test("wiring_pricing: anthropic is known for Claude Code, OpenRouter is unchanged", () => {
    const base = cataloguePricing(Catalogue.load(BUNDLED_CATALOGUE));
    const pricing = claudeWiring(async () => INFO, dir).pricing(base);
    expect(base.known("anthropic", "default")).toBe(false);
    expect(pricing.known("anthropic", "default")).toBe(true);
    for (const model of ["anthropic/claude-sonnet-5.5", "synth/unlisted"]) expect(pricing.known("openrouter", model)).toBe(base.known("openrouter", model));
    expect(pricing.known("openrouter", "synth/unlisted")).toBe(false);
  });
});

describe("diagnostics name the model", () => {
  test("journal_names_model_by_table_or_hash: the discovered ids are named, an id AGENT_MODELS lacks is `other` + modelHash", async () => {
    const s = openStores(dir, { diagnostics: "on", training: "on" });
    try {
      const { j } = agentJournal(s);
      const runId = "01JR0000000000000000000001";
      // The plans a Claude Code start makes (nothing discovered yet, so any id plans).
      const p = planner({ models: async () => [] });
      const of2 = async (id: string, model: string) => {
        const plan = await p.plan(start({ model }), PEOPLE);
        return session({ id, harness: plan.harness, provider: plan.provider, model: plan.model, weights: plan.weights });
      };
      const known = await of2("01JS000000000000000000000A", "default");
      const unknown = await of2("01JS000000000000000000000B", "claude-synth-unlisted-9");
      for (const ss of [known, unknown]) j.turn({ session: ss, runId, usage: usage() });
      await flush(s);
      const turns = agentEntries(dir, "events").filter((e) => e.kind === "agent.turn");
      const of = (id: string) => turns.find((t) => t.ctx?.actor?.session === id)!.payload;
      expect(of(known.id)).toMatchObject({ harness: "claudeCode", provider: "anthropic", model: "default" });
      expect(of(known.id).modelHash).toBeUndefined();
      expect(of(unknown.id)).toMatchObject({ harness: "claudeCode", provider: "anthropic", model: "other" });
      expect(of(unknown.id).modelHash).toMatch(/\S/);
      expect(JSON.stringify(turns)).not.toContain("claude-synth-unlisted-9");
    } finally {
      await s.close();
    }
  });
});
