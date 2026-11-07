// The model catalogue as a pricing and metadata cache, OpenRouter's models API, and the CLIs'
// own model lists (owner decisions after review 7A-1). Synthetic model ids throughout: adding a
// real model never needs a test change.
import { mkdtempSync, readFileSync, statSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { BUNDLED_CATALOGUE, Catalogue, type Usage, ZERO_USAGE } from "../src/agent/catalogue/catalogue.ts";
import { discoverCodexModels, parseClaudeInitialize, parseCodexModelPage } from "../src/agent/catalogue/cli-models.ts";
import { OpenRouterPrices, parseModels } from "../src/agent/catalogue/prices.ts";
import { manualClock } from "../src/agent/clock.ts";

const FIXTURE = fileURLToPath(new URL("./fixtures/catalogue.test.json", import.meta.url));
const json = (name: string) => JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), "utf8")) as unknown;
const usage = (u: Partial<Usage>): Usage => ({ ...ZERO_USAGE, ...u });

function code(f: () => unknown): string {
  try {
    f();
  } catch (e) {
    return (e as { code?: string }).code ?? "no code";
  }
  return "none";
}

/** A prices cache loaded from the fixture through a loopback models endpoint. */
async function prices(): Promise<OpenRouterPrices> {
  const body = JSON.stringify(json("openrouter-models.json"));
  const server = createServer((req, res) => {
    if (req.url === "/api/v1/models" && req.headers.authorization === undefined) res.writeHead(200, { "content-type": "application/json" }).end(body);
    else res.writeHead(404).end();
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  const { port } = server.address() as AddressInfo;
  const p = new OpenRouterPrices(join(mkdtempSync(join(tmpdir(), "fin-prices-")), "agents", "openrouter-models.json"), manualClock());
  try {
    expect(await p.refresh(`http://127.0.0.1:${port}/api/v1`)).toBe(3);
  } finally {
    server.close();
  }
  return p;
}

describe("the bundled catalogue", () => {
  it("holds structure only: presets, defaults and families, no fixed model list", () => {
    const raw = JSON.parse(readFileSync(BUNDLED_CATALOGUE, "utf8")) as { models: unknown[]; families: { weights: string; maker: string }[] };
    expect(raw.models).toEqual([]);
    for (const f of raw.families) {
      expect(["open", "closed"]).toContain(f.weights);
      expect(f.maker).not.toBe("");
    }
    const cat = Catalogue.load(BUNDLED_CATALOGUE);
    expect(cat.openrouterBaseUrl).toBe("https://openrouter.ai/api/v1");
    expect(Object.keys(cat.localPresets).sort()).toEqual(["lmstudio", "ollama"]);
    expect(cat.bundledIds).toEqual([]);
    // Families give provenance when nothing else is known.
    expect(cat.describe("openrouter", "anthropic/any-new-claude")).toMatchObject({ weights: "closed", maker: "anthropic", capabilities: { caching: "explicit" } });
    expect(cat.describe("openrouter", "openai/gpt-oss-999b").weights).toBe("open");
    expect(cat.describe("openrouter", "openai/any-gpt").weights).toBe("closed");
    expect(cat.describe("openrouter", "nobody/unheard-of")).toMatchObject({ weights: "unknown", source: "default", window: 8192 });
    expect(cat.describe("local", "llama3.3:70b")).toMatchObject({ weights: "open", maker: "meta", price: { input: 0, output: 0 } });
  });

  it("pins the openrouter preset to openrouter.ai and refuses a malformed file", () => {
    const doc = JSON.parse(readFileSync(FIXTURE, "utf8")) as Record<string, any>;
    const with_ = (f: (d: Record<string, any>) => void) => {
      const d = structuredClone(doc);
      f(d);
      return JSON.stringify(d);
    };
    expect(code(() => Catalogue.parse(with_((d) => (d.presets.openrouter.baseUrl = "https://models.example/api/v1"))))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(with_((d) => (d.presets.openrouter.baseUrl = "http://openrouter.ai/api/v1"))))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(with_((d) => (d.families[0].weights = "unknown"))))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(with_((d) => (d.families[0].maker = "")))))
      .toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse("{"))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(JSON.stringify(doc), { extra: [{ id: "x", window: 1 }, { id: "x", window: 2 }] }))).toBe("CATALOGUE_INVALID");
    expect(code(() => Catalogue.parse(JSON.stringify(doc), { extra: [{ id: "x", window: 0 }] }))).toBe("CATALOGUE_INVALID");
  });
});

describe("describing and pricing any model id", () => {
  it("takes OpenRouter's listing for an id the user typed: window, capabilities, price, weights", async () => {
    const cat = Catalogue.load(FIXTURE, { prices: await prices() });
    const big = cat.describe("openrouter", "acme-closed/big");
    expect(big).toMatchObject({
      source: "openrouter",
      weights: "closed",
      window: 200_000,
      maxOutput: 32_000,
      capabilities: { tools: true, thinking: true, images: true, caching: "explicit" },
      price: { input: 3_000_000, output: 15_000_000, cacheRead: 300_000, cacheWrite: 3_750_000 },
    });
    // A Hugging Face id means open weights, whatever the family says.
    expect(cat.describe("openrouter", "acme-closed/weights-on-hf").weights).toBe("open");
    // 1000 in × 3 + 200 out × 15 + 5000 cache read × 0.3 + 100 cache write × 3.75 (USD per MTok).
    const u = usage({ input: 1000, output: 200, cacheRead: 5000, cacheWrite: 100 });
    expect(cat.price("openrouter", "acme-closed/big", u)).toEqual({ micros: 7875, basis: "priced" });
    expect(cat.projectedMicros("openrouter", "acme-closed/big", u)).toBe(7875);
    expect(cat.cacheReadWeight("openrouter", "acme-closed/big")).toBeCloseTo(0.1);
  });

  it("an unlisted id has an unknown price: post-hoc charging only", async () => {
    const cat = Catalogue.load(FIXTURE, { prices: await prices() });
    const u = usage({ input: 1000 });
    expect(cat.describe("openrouter", "acme-open/brand-new")).toMatchObject({ source: "family", weights: "open", window: 8192 });
    expect(cat.describe("openrouter", "acme-open/brand-new").price).toBeUndefined();
    expect(cat.projectedMicros("openrouter", "acme-open/brand-new", u)).toBeNull();
    expect(cat.price("openrouter", "acme-open/brand-new", u)).toEqual({ micros: 0, basis: "unknown" });
    expect(cat.price("openrouter", "acme-open/brand-new", u, 1234.2)).toEqual({ micros: 1235, basis: "reported" });
  });

  it("prices local models at zero, any id", () => {
    const cat = Catalogue.load(FIXTURE);
    expect(cat.price("local", "testllama-70b", usage({ input: 10_000, output: 999 }))).toEqual({ micros: 0, basis: "priced" });
    expect(cat.price("local", "whatever:7b", usage({ input: 1 }))).toEqual({ micros: 0, basis: "priced" });
    expect(cat.describe("local", "testllama-70b")).toMatchObject({ weights: "open", window: 65_536 });
    expect(cat.describe("local", "whatever:7b").weights).toBe("unknown");
  });

  it("the owner's entries win; CLI models are closed, Codex's priced from OpenRouter's openai listing", async () => {
    const cat = Catalogue.load(FIXTURE, {
      prices: await prices(),
      extra: [
        { id: "acme-closed/big", window: 1000, weights: "open" },
        { id: "mine:7b", provider: "local", window: 4096, weights: "open" },
      ],
    });
    expect(cat.describe("openrouter", "acme-closed/big")).toMatchObject({ source: "settings", window: 1000, weights: "open", price: { input: 3_000_000 } });
    expect(cat.describe("local", "mine:7b")).toMatchObject({ source: "settings", window: 4096, price: { input: 0 } });
    expect(cat.describe("openai", "test-codex-1")).toMatchObject({ weights: "closed", window: 400_000, price: { input: 1_000_000 } });
    expect(cat.describe("anthropic", "test-claude-big")).toMatchObject({ weights: "closed", maker: "anthropic" });
    expect(cat.price("anthropic", "test-claude-big", usage({ input: 5 }))).toEqual({ micros: 0, basis: "unknown" });
  });
});

describe("OpenRouter's models API cache", () => {
  it("parses the listing, skipping entries without an id or a window", () => {
    const models = parseModels(json("openrouter-models.json"));
    expect(models.map((m) => m.id)).toEqual(["acme-closed/big", "acme-closed/weights-on-hf", "openai/test-codex-1"]);
    expect(models[2]).toMatchObject({ maxOutput: 4096, price: { cacheRead: 0, cacheWrite: 0 } });
    expect(code(() => parseModels({}))).toBe("PROVIDER_PROTOCOL");
  });

  it("is written to server data, read back on start, and goes stale after a day", async () => {
    const p = await prices();
    expect(statSync(p.file).isFile()).toBe(true);
    const clock = manualClock();
    const again = new OpenRouterPrices(p.file, clock);
    expect(again.get("acme-closed/big")?.window).toBe(200_000);
    expect(again.stale()).toBe(false);
    clock.advance(25 * 60 * 60_000);
    expect(again.stale()).toBe(true);
    expect(new OpenRouterPrices(join(tmpdir(), "nope", "x.json"), clock).stale()).toBe(true);
  });
});

describe("the CLIs' own model lists", () => {
  it("pages through Codex's model/list, skipping hidden models", async () => {
    const pages = json("codex-model-list.json") as unknown[];
    const calls: unknown[] = [];
    const models = await discoverCodexModels(async (method, params) => {
      calls.push([method, params]);
      return pages[calls.length - 1];
    });
    expect(calls).toEqual([
      ["model/list", {}],
      ["model/list", { cursor: "page-2" }],
    ]);
    expect(models).toEqual([
      { id: "test-codex-1", displayName: "Test Codex 1", isDefault: true },
      { id: "test-codex-2", displayName: "Test Codex 2", isDefault: false },
    ]);
    expect(code(() => parseCodexModelPage({ nope: 1 }))).toBe("PROVIDER_PROTOCOL");
  });

  it("reads Claude Code's initialize response", () => {
    expect(parseClaudeInitialize(json("claude-initialize.json"))).toEqual([
      { id: "default", displayName: "Default (recommended)", isDefault: true },
      { id: "test-claude-big", displayName: "Big", isDefault: false },
      { id: "test-claude-small", displayName: "Small", isDefault: false },
    ]);
    expect(code(() => parseClaudeInitialize({ type: "control_response", response: {} }))).toBe("PROVIDER_PROTOCOL");
  });
});
