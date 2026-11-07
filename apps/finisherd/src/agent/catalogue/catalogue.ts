// The model catalogue (sub-project 7 §4.7, owner decisions after review 7A-1): a pricing and
// metadata cache, not a list of models to offer.
//
// - OpenRouter: the user types or pastes any model id. Its window, capabilities and price come
//   from OpenRouter's public models API (`prices.ts`, cached on disk), else from the reported
//   `usage.cost` after the call: an unknown price means post-hoc charging only.
// - Local: any model the user's server lists or the owner types; priced at zero.
// - Claude Code and Codex: the CLI lists its own models (`cli-models.ts`); never hardcoded.
//
// The bundled `models.json` holds only structure: the presets, defaults, and well-known
// families (by id prefix) that give a model's maker, weights class and caching when nothing
// better is known. The weights class is the provenance §12.7 filters on.
import { readFileSync } from "node:fs";
import { finError } from "../../errors.ts";
import type { OpenRouterPrices } from "./prices.ts";

/** The session's provider: the built-in agent's two presets, and the CLIs'. */
export type Provider = "openrouter" | "local" | "anthropic" | "openai";
export type WeightsClass = "open" | "closed" | "unknown";
export type Caching = "explicit" | "automatic" | "none";

/** Micros (USD × 10⁶) per million tokens. */
export interface Price {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
}

export interface ModelInfo {
  readonly id: string;
  readonly provider: Provider;
  readonly maker: string;
  readonly weights: WeightsClass;
  readonly window: number;
  readonly maxOutput: number;
  readonly capabilities: {
    readonly tools: boolean;
    readonly thinking: boolean;
    readonly images: boolean;
    readonly caching: Caching;
  };
  /** Absent: unknown, charged after the call from what the provider reports. */
  readonly price?: Price;
  /** Where the facts came from. */
  readonly source: "settings" | "openrouter" | "family" | "default";
}

/** A model the owner describes in settings (`agents.models`); never written by an agent. */
export interface ExtraModel {
  readonly id: string;
  readonly provider?: "openrouter" | "local";
  readonly weights?: "open" | "closed";
  readonly window: number;
  readonly maxOutput?: number;
  readonly tools?: boolean;
  readonly price?: Price;
}

/**
 * Token usage of one model call. `input` excludes the cache reads and writes, which are
 * counted on their own; `reasoning` is part of `output` (as the providers report it).
 */
export interface Usage {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly reasoning: number;
}

export const ZERO_USAGE: Usage = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, reasoning: 0 };
export const ZERO_PRICE: Price = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

export type CostBasis = "reported" | "priced" | "unknown";

interface Family {
  readonly provider: "openrouter" | "local";
  readonly prefix: string;
  readonly maker: string;
  readonly weights: "open" | "closed";
  readonly caching?: Caching;
  readonly window?: number;
}

/** OpenRouter's API host: the openrouter preset talks to it and nothing else (review M1). */
export const OPENROUTER_HOST = "openrouter.ai";

const CACHING = new Set(["explicit", "automatic", "none"]);
const isCount = (n: unknown): n is number => typeof n === "number" && Number.isSafeInteger(n) && n >= 0;

function invalid(message: string, details: unknown = {}) {
  return finError("CATALOGUE_INVALID", message, details, "fix the model catalogue or agents.models");
}

function checkPrice(p: unknown, at: string): Price {
  const price = p as Record<string, unknown>;
  for (const k of ["input", "output", "cacheRead", "cacheWrite"]) {
    if (!isCount(price?.[k])) throw invalid(`${at}: price.${k} must be a whole number of micros per MTok`);
  }
  return price as unknown as Price;
}

function checkUrl(raw: unknown, at: string): string {
  if (typeof raw !== "string" || !URL.canParse(raw)) throw invalid(`${at} must be a URL`);
  return raw.replace(/\/+$/, "");
}

function checkFamily(raw: unknown, i: number): Family {
  const f = raw as Record<string, unknown>;
  const at = `families[${i}]`;
  if (f?.provider !== "openrouter" && f?.provider !== "local") throw invalid(`${at}: provider`);
  if (typeof f.prefix !== "string" || f.prefix === "") throw invalid(`${at}: prefix`);
  if (typeof f.maker !== "string" || f.maker === "") throw invalid(`${at}: maker`);
  if (f.weights !== "open" && f.weights !== "closed") throw invalid(`${at}: weights must be open or closed`);
  if (f.caching !== undefined && !CACHING.has(f.caching as string)) throw invalid(`${at}: caching`);
  if (f.window !== undefined && (!isCount(f.window) || f.window === 0)) throw invalid(`${at}: window`);
  return f as unknown as Family;
}

function checkExtra(x: ExtraModel, i: number): ExtraModel {
  const at = `agents.models[${i}]`;
  if (typeof x?.id !== "string" || x.id.length === 0 || x.id.length > 200 || /[\s\u0000-\u001f]/.test(x.id)) {
    throw invalid(`${at}: id must be a model id`);
  }
  if (x.provider !== undefined && x.provider !== "openrouter" && x.provider !== "local") throw invalid(`${at}: provider`, { id: x.id });
  if (!isCount(x.window) || x.window === 0) throw invalid(`${at}: window must be a token count`, { id: x.id });
  if (x.maxOutput !== undefined && !isCount(x.maxOutput)) throw invalid(`${at}: maxOutput`, { id: x.id });
  if (x.price !== undefined) checkPrice(x.price, at);
  return x;
}

export interface CatalogueOptions {
  /** The owner's `agents.models`. */
  readonly extra?: readonly ExtraModel[];
  /** OpenRouter's models API, cached (absent: families and defaults only). */
  readonly prices?: OpenRouterPrices;
}

export class Catalogue {
  private readonly extra: ReadonlyMap<string, ExtraModel>;

  private constructor(
    readonly asOf: string,
    readonly openrouterBaseUrl: string,
    readonly localPresets: Readonly<Record<string, string>>,
    private readonly defaults: { window: number; maxOutput: number },
    private readonly families: readonly Family[],
    /** Model ids the bundled file names (diagnostics names them; ⊆ `AGENT_MODELS`). */
    readonly bundledIds: readonly string[],
    extra: readonly ExtraModel[],
    private readonly prices: OpenRouterPrices | undefined,
  ) {
    this.extra = new Map(extra.map((x) => [`${x.provider ?? "openrouter"}/${x.id}`, x]));
  }

  static load(bundled: string | URL, opts: CatalogueOptions = {}): Catalogue {
    return Catalogue.parse(readFileSync(bundled, "utf8"), opts);
  }

  /** `CATALOGUE_INVALID` when the file or the owner's entries are malformed. */
  static parse(json: string, opts: CatalogueOptions = {}): Catalogue {
    let raw: Record<string, unknown>;
    try {
      raw = JSON.parse(json) as Record<string, unknown>;
    } catch (e) {
      throw invalid(`the catalogue is not JSON: ${(e as Error).message}`);
    }
    if (typeof raw.asOf !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(raw.asOf)) throw invalid("asOf must be a date");
    const presets = (raw.presets ?? {}) as Record<string, Record<string, unknown>>;
    const openrouter = checkUrl(presets.openrouter?.baseUrl, "presets.openrouter.baseUrl");
    const url = new URL(openrouter);
    if (url.protocol !== "https:" || url.hostname !== OPENROUTER_HOST) {
      throw invalid(`presets.openrouter.baseUrl must be https://${OPENROUTER_HOST}/…`);
    }
    const local: Record<string, string> = {};
    for (const [k, v] of Object.entries(presets.local ?? {})) local[k] = checkUrl(v, `presets.local.${k}`);
    const d = (raw.defaults ?? {}) as Record<string, unknown>;
    if (!isCount(d.window) || d.window === 0 || !isCount(d.maxOutput)) throw invalid("defaults.window and defaults.maxOutput");
    if (!Array.isArray(raw.families) || !Array.isArray(raw.models)) throw invalid("families and models must be lists");
    const families = raw.families.map(checkFamily);
    const ids = raw.models.map((m, i) => {
      const id = (m as { id?: unknown })?.id;
      if (typeof id !== "string" || id === "") throw invalid(`models[${i}]: id`);
      return id;
    });
    const extra = (opts.extra ?? []).map(checkExtra);
    const seen = new Set<string>();
    for (const x of extra) {
      const k = `${x.provider ?? "openrouter"}/${x.id}`;
      if (seen.has(k)) throw invalid(`agents.models lists ${x.id} twice`, { id: x.id });
      seen.add(k);
    }
    return new Catalogue(raw.asOf, openrouter, local, { window: d.window, maxOutput: d.maxOutput }, families, ids, extra, opts.prices);
  }

  /** What is known about `id` on `provider`: the owner's entry, else OpenRouter's, else the family's, else defaults. */
  describe(provider: Provider, id: string): ModelInfo {
    const builtin = provider === "openrouter" || provider === "local";
    const family = builtin ? this.families.find((f) => f.provider === provider && id.startsWith(f.prefix)) : undefined;
    const base = {
      id,
      provider,
      maker: family?.maker ?? (provider === "anthropic" || provider === "openai" ? provider : "other"),
      // CLI harnesses serve their maker's closed models.
      weights: (family?.weights ?? (builtin ? "unknown" : "closed")) as WeightsClass,
    };
    const caps = (tools: boolean, thinking = false, images = false) => ({ tools, thinking, images, caching: family?.caching ?? ("none" as Caching) });
    const x = builtin ? this.extra.get(`${provider}/${id}`) : undefined;
    if (x) {
      const price = x.price ?? (provider === "local" ? ZERO_PRICE : this.prices?.get(id)?.price);
      return {
        ...base,
        weights: x.weights ?? base.weights,
        window: x.window,
        maxOutput: x.maxOutput ?? Math.min(this.defaults.maxOutput, x.window),
        capabilities: caps(x.tools ?? true),
        ...(price === undefined ? {} : { price }),
        source: "settings",
      };
    }
    const listed = provider === "openrouter" ? this.prices?.get(id) : provider === "openai" ? this.prices?.get(`openai/${id}`) : undefined;
    if (listed) {
      return {
        ...base,
        weights: listed.openWeights ? "open" : base.weights,
        window: listed.window,
        maxOutput: listed.maxOutput,
        capabilities: caps(listed.tools, listed.thinking, listed.images),
        price: listed.price,
        source: "openrouter",
      };
    }
    return {
      ...base,
      window: family?.window ?? this.defaults.window,
      maxOutput: this.defaults.maxOutput,
      capabilities: caps(true),
      ...(provider === "local" ? { price: ZERO_PRICE } : {}),
      source: family ? "family" : "default",
    };
  }

  /** How much one cache-read token weighs against one input token in token budgets. */
  cacheReadWeight(provider: Provider, id: string): number {
    const p = this.describe(provider, id).price;
    if (p === undefined || p.input === 0) return 1;
    return p.cacheRead / p.input;
  }

  /** The projected cost of `u` in micros, or null when the price is unknown (post-hoc only). */
  projectedMicros(provider: Provider, id: string, u: Usage): number | null {
    const p = this.describe(provider, id).price;
    return p === undefined ? null : priceOf(p, u);
  }

  /**
   * The cost of `u`: what the provider reported (`reported`), else the known price (`priced`;
   * a local model at zero), else `unknown` (0 micros now; settled later where possible).
   */
  price(provider: Provider, id: string, u: Usage, reportedMicros?: number): { micros: number; basis: CostBasis } {
    if (reportedMicros !== undefined && Number.isFinite(reportedMicros) && reportedMicros >= 0) {
      return { micros: Math.ceil(reportedMicros), basis: "reported" };
    }
    const p = this.describe(provider, id).price;
    if (p === undefined) return { micros: 0, basis: "unknown" };
    return { micros: priceOf(p, u), basis: "priced" };
  }
}

/** `u` at `p`, in micros, rounded up. */
export function priceOf(p: Price, u: Usage): number {
  const perMillion = u.input * p.input + u.output * p.output + u.cacheRead * p.cacheRead + u.cacheWrite * p.cacheWrite;
  return Math.ceil(perMillion / 1_000_000);
}

/** The bundled catalogue's path. */
export const BUNDLED_CATALOGUE = new URL("./models.json", import.meta.url);
