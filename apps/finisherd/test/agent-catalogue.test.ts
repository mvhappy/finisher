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
import { LISTING_MAX_BYTES, OpenRouterPrices, parseModels, REFRESH_BACKOFF_MS } from "../src/agent/catalogue/prices.ts";
import { cataloguePricing } from "../src/agent/runtime-deps.ts";
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
    expect(await p.refresh(`http://127.0.0.1:${port}/api/v1`)).toBe(4);
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
    expect(models.map((m) => m.id)).toEqual(["acme-closed/big", "acme-closed/weights-on-hf", "openai/test-codex-1", "openrouter/auto"]);
    expect(models[2]).toMatchObject({ maxOutput: 4096, price: { cacheRead: 0, cacheWrite: 0 } });
    expect(code(() => parseModels({}))).toBe("PROVIDER_PROTOCOL");
  });

  it("never reads a missing or negative price as free (H1): `-1` is an unknown price, only a literal 0 is free", async () => {
    const entry = (id: string, pricing: unknown) => ({ id, context_length: 1000, top_provider: {}, supported_parameters: ["tools"], pricing });
    const listed = parseModels({
      data: [
        entry("m/router", { prompt: "-1", completion: "-1" }),
        entry("m/half", { prompt: "0.000001" }),
        entry("m/blank", { prompt: "", completion: "" }),
        entry("m/negative-number", { prompt: -0.000001, completion: 0.000002 }),
        entry("m/nan", { prompt: "cheap", completion: "0.000002" }),
        entry("m/none", undefined),
        entry("m/free", { prompt: "0", completion: "0" }),
        entry("m/paid", { prompt: "0.000001", completion: "0.000002", input_cache_read: "-1" }),
      ],
    });
    const byId = Object.fromEntries(listed.map((m) => [m.id, m]));
    for (const id of ["m/router", "m/half", "m/blank", "m/negative-number", "m/nan", "m/none"]) expect(byId[id], id).toBeDefined();
    for (const id of ["m/router", "m/half", "m/blank", "m/negative-number", "m/nan", "m/none"]) expect(byId[id]!.price, id).toBeUndefined();
    expect(byId["m/free"]!.price).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
    expect(byId["m/paid"]!.price).toEqual({ input: 1_000_000, output: 2_000_000, cacheRead: 0, cacheWrite: 0 });

    // Through the fixture's `openrouter/auto` ("-1"): the catalogue has no price, so a session
    // cannot start on it (`PRICING_UNKNOWN`), and usage is charged `unknown`, never $0 `priced`.
    const p = await prices();
    const cat = Catalogue.load(BUNDLED_CATALOGUE, { prices: p });
    expect(cat.describe("openrouter", "openrouter/auto")).toMatchObject({ source: "openrouter", window: 2_000_000 });
    expect(cat.describe("openrouter", "openrouter/auto").price).toBeUndefined();
    expect(cat.describe("openrouter", "acme-closed/weights-on-hf").price).toEqual({ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 });
    const pricing = cataloguePricing(cat);
    expect(pricing.known("openrouter", "openrouter/auto")).toBe(false);
    expect(pricing.known("openrouter", "acme-closed/big")).toBe(true);
    expect(pricing.known("openrouter", "acme-closed/weights-on-hf")).toBe(true);
    expect(cat.price("openrouter", "openrouter/auto", usage({ input: 10, output: 10 }))).toEqual({ micros: 0, basis: "unknown" });
  });

  it("refreshes one fetch at a time, backs off 15 minutes after a failure, caps the body and drops bad ids (M1)", async () => {
    const dir = mkdtempSync(join(tmpdir(), "fin-prices-"));
    const clock = manualClock();
    const p = new OpenRouterPrices(join(dir, "agents", "openrouter-models.json"), clock);
    const good = JSON.stringify({ data: [{ id: "m/one", context_length: 1000, pricing: { prompt: "0.000001", completion: "0.000002" } }] });

    // Single-flight: concurrent callers share one fetch.
    let calls = 0;
    let release: () => void = () => {};
    const gate = new Promise<void>((r) => (release = r));
    const slow = (async () => {
      calls++;
      await gate;
      return new Response(good);
    }) as unknown as typeof fetch;
    const together = Promise.all([p.refreshIfStale("http://x/api/v1", slow), p.refreshIfStale("http://x/api/v1", slow), p.refresh("http://x/api/v1", slow)]);
    release();
    await together;
    expect(calls).toBe(1);
    expect(p.get("m/one")?.price?.input).toBe(1_000_000);

    // A failure is not retried for 15 minutes, then is.
    clock.advance(25 * 60 * 60_000);
    let attempts = 0;
    const errors: unknown[] = [];
    const down = (async () => {
      attempts++;
      return new Response("no", { status: 503 });
    }) as unknown as typeof fetch;
    await p.refreshIfStale("http://x/api/v1", down, undefined, (e) => errors.push(e));
    await p.refreshIfStale("http://x/api/v1", down, undefined, (e) => errors.push(e));
    expect(attempts).toBe(1);
    expect(errors).toHaveLength(1);
    clock.advance(REFRESH_BACKOFF_MS - 1);
    await p.refreshIfStale("http://x/api/v1", down);
    expect(attempts).toBe(1);
    clock.advance(2);
    await p.refreshIfStale("http://x/api/v1", down);
    expect(attempts).toBe(2);
    // Backed off again after the second failure; a success clears it.
    await p.refreshIfStale("http://x/api/v1", down);
    expect(attempts).toBe(2);
    clock.advance(REFRESH_BACKOFF_MS + 1);
    await p.refreshIfStale("http://x/api/v1", (async () => new Response(good)) as unknown as typeof fetch);
    expect(p.stale()).toBe(false);

    // The body is streamed and capped: a listing over the cap is refused without being read whole.
    const huge = (async () =>
      new Response(
        new ReadableStream({
          pull(c) {
            c.enqueue(new Uint8Array(1024 * 1024));
          },
        }),
      )) as unknown as typeof fetch;
    await expect(p.refresh("http://x/api/v1", huge)).rejects.toMatchObject({ code: "PROVIDER_PROTOCOL" });
    const declared = (async () => new Response("{}", { headers: { "content-length": String(LISTING_MAX_BYTES + 1) } })) as unknown as typeof fetch;
    clock.advance(REFRESH_BACKOFF_MS + 1);
    await expect(p.refresh("http://x/api/v1", declared)).rejects.toMatchObject({ code: "PROVIDER_PROTOCOL" });

    // Ids that are not OpenRouter's shape never enter the catalogue.
    const ids = ["ok/model", "ok/model:free", "no-slash", "bad id/model", "../etc/passwd", "a/b/c", `${"x".repeat(65)}/m`, "ok/with\nnewline"];
    const listed = parseModels({ data: ids.map((id) => ({ id, context_length: 1000, pricing: { prompt: "0", completion: "0" } })) });
    expect(listed.map((m) => m.id)).toEqual(["ok/model", "ok/model:free"]);
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
