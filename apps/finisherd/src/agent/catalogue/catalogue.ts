// The model catalogue (sub-project 7 §4.7; plan 7A Decision 19): bundled data
// (`models.json`) plus the models the owner adds in settings (`agents.models`). It names each
// model's harness, provider, maker and weights class (the provenance the journal records),
// its window and capabilities, and prices a turn.
import { readFileSync } from "node:fs";
import { finError } from "../../errors.ts";

export type CatalogueHarness = "builtin" | "claudeCode" | "codex";
/** `openrouter` and `local` are the built-in agent's two presets; the others are the CLIs'. */
export type CatalogueProvider = "openrouter" | "local" | "anthropic" | "openai";
export type WeightsClass = "open" | "closed" | "unknown";
export type Caching = "explicit" | "automatic" | "none";

/** Micros (USD × 10⁶) per million tokens. */
export interface Price {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
}

export interface ModelEntry {
  readonly id: string;
  readonly harness: CatalogueHarness;
  readonly provider: CatalogueProvider;
  readonly maker: string;
  readonly weights: WeightsClass;
  /** An OpenRouter variant suffix (`free` for `<id>:free`): `:` is not a name character. */
  readonly variant?: string;
  readonly window: number;
  readonly maxOutput: number;
  readonly capabilities: {
    readonly tools: boolean;
    readonly thinking: boolean;
    readonly images: boolean;
    readonly caching: Caching;
  };
  readonly price?: Price;
  /** Added by the owner in settings, not bundled: recorded as `other` plus a keyed hash. */
  readonly extra?: true;
}

/** A model the owner adds in settings (`agents.models`). */
export interface ExtraModel {
  readonly id: string;
  /** `openrouter` (the default) or `local` (an OpenAI-compatible server the user runs). */
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

export type CostBasis = "reported" | "priced" | "unknown";

const HARNESSES = new Set(["builtin", "claudeCode", "codex"]);
const PROVIDERS = new Set(["openrouter", "local", "anthropic", "openai"]);
const CACHING = new Set(["explicit", "automatic", "none"]);
/** As fin-core's `NAME_PATTERN`, so every bundled id can be an `AGENT_MODELS` entry. */
const ID = /^[a-z][A-Za-z0-9_./-]{0,63}$/;
const ZERO_PRICE: Price = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };

function invalid(message: string, details: unknown = {}) {
  return finError("CATALOGUE_INVALID", message, details, "fix the model catalogue or agents.models");
}

const isCount = (n: unknown): n is number => typeof n === "number" && Number.isSafeInteger(n) && n >= 0;

function checkPrice(p: unknown, at: string): Price {
  const price = p as Record<string, unknown>;
  for (const k of ["input", "output", "cacheRead", "cacheWrite"]) {
    if (!isCount(price?.[k])) throw invalid(`${at}: price.${k} must be a whole number of micros per MTok`);
  }
  return price as unknown as Price;
}

function checkEntry(raw: unknown, i: number): ModelEntry {
  const e = raw as Record<string, unknown>;
  const at = `models[${i}]`;
  if (typeof e?.id !== "string" || !ID.test(e.id)) throw invalid(`${at}: id must be a lower-case model name`);
  if (!HARNESSES.has(e.harness as string)) throw invalid(`${at}: unknown harness`, { id: e.id });
  if (!PROVIDERS.has(e.provider as string)) throw invalid(`${at}: unknown provider`, { id: e.id });
  if (typeof e.maker !== "string" || e.maker === "") throw invalid(`${at}: maker missing`, { id: e.id });
  if (e.weights !== "open" && e.weights !== "closed") throw invalid(`${at}: weights must be open or closed`, { id: e.id });
  if (e.variant !== undefined && (typeof e.variant !== "string" || !/^[a-z0-9-]+$/.test(e.variant))) {
    throw invalid(`${at}: bad variant`, { id: e.id });
  }
  if (!isCount(e.window) || e.window === 0 || !isCount(e.maxOutput)) throw invalid(`${at}: window and maxOutput`, { id: e.id });
  const c = e.capabilities as Record<string, unknown> | undefined;
  if (typeof c?.tools !== "boolean" || typeof c.thinking !== "boolean" || typeof c.images !== "boolean" || !CACHING.has(c.caching as string)) {
    throw invalid(`${at}: capabilities`, { id: e.id });
  }
  if (e.price !== undefined) checkPrice(e.price, at);
  return e as unknown as ModelEntry;
}

function fromExtra(x: ExtraModel, i: number): ModelEntry {
  const at = `agents.models[${i}]`;
  if (typeof x?.id !== "string" || x.id.length === 0 || x.id.length > 200 || /[\s\u0000-\u001f]/.test(x.id)) {
    throw invalid(`${at}: id must be a model slug`);
  }
  if (x.provider !== undefined && x.provider !== "openrouter" && x.provider !== "local") {
    throw invalid(`${at}: provider must be openrouter or local`, { id: x.id });
  }
  if (!isCount(x.window) || x.window === 0) throw invalid(`${at}: window must be a token count`, { id: x.id });
  if (x.maxOutput !== undefined && !isCount(x.maxOutput)) throw invalid(`${at}: maxOutput`, { id: x.id });
  const provider = x.provider ?? "openrouter";
  // A local model costs nothing in money; an OpenRouter one is priced from OpenRouter's report.
  const price = x.price !== undefined ? checkPrice(x.price, at) : provider === "local" ? ZERO_PRICE : undefined;
  return {
    id: x.id,
    harness: "builtin",
    provider,
    maker: "other",
    weights: x.weights ?? "unknown",
    window: x.window,
    maxOutput: x.maxOutput ?? Math.min(8192, x.window),
    capabilities: { tools: x.tools ?? true, thinking: false, images: false, caching: "none" },
    ...(price === undefined ? {} : { price }),
    extra: true,
  };
}

export class Catalogue {
  private readonly byId: ReadonlyMap<string, ModelEntry>;

  private constructor(
    readonly asOf: string,
    readonly openrouterBaseUrl: string,
    entries: readonly ModelEntry[],
  ) {
    this.byId = new Map(entries.map((e) => [e.id, e]));
  }

  /** Reads the bundled catalogue at `bundled` (a path) and adds `extra`. */
  static load(bundled: string | URL, extra: readonly ExtraModel[] = []): Catalogue {
    return Catalogue.parse(readFileSync(bundled, "utf8"), extra);
  }

  /** The catalogue from its JSON text plus `extra`; `CATALOGUE_INVALID` when either is malformed. */
  static parse(json: string, extra: readonly ExtraModel[] = []): Catalogue {
    let raw: Record<string, unknown>;
    try {
      raw = JSON.parse(json) as Record<string, unknown>;
    } catch (e) {
      throw invalid(`the catalogue is not JSON: ${(e as Error).message}`);
    }
    if (typeof raw.asOf !== "string" || !/^\d{4}-\d{2}-\d{2}$/.test(raw.asOf)) throw invalid("asOf must be a date");
    const base = raw.openrouterBaseUrl;
    if (typeof base !== "string" || !URL.canParse(base)) throw invalid("openrouterBaseUrl must be a URL");
    if (!Array.isArray(raw.models)) throw invalid("models must be a list");
    const entries = raw.models.map(checkEntry);
    const added = extra.map(fromExtra);
    const seen = new Set<string>();
    for (const e of [...entries, ...added]) {
      if (seen.has(e.id)) throw invalid(`model ${e.id} is listed twice`, { id: e.id });
      seen.add(e.id);
    }
    return new Catalogue(raw.asOf, base.replace(/\/+$/, ""), [...entries, ...added]);
  }

  get(id: string): ModelEntry | undefined {
    return this.byId.get(id);
  }

  entries(): ModelEntry[] {
    return [...this.byId.values()];
  }

  /** The model id as the provider's API takes it (`<id>:<variant>` on OpenRouter). */
  wireModel(e: ModelEntry): string {
    return e.variant === undefined ? e.id : `${e.id}:${e.variant}`;
  }

  /** How much one cache-read token weighs against one input token in token budgets. */
  cacheReadWeight(id: string): number {
    const p = this.get(id)?.price;
    if (p === undefined || p.input === 0) return 1;
    return p.cacheRead / p.input;
  }

  /**
   * The cost of `u` on model `id`: what the provider reported when it did (`reported`), else
   * the catalogue's price (`priced`), else `unknown` (0 micros). Rounded up to a whole micro.
   */
  price(id: string, u: Usage, reportedMicros?: number): { micros: number; basis: CostBasis } {
    if (reportedMicros !== undefined && Number.isFinite(reportedMicros) && reportedMicros >= 0) {
      return { micros: Math.ceil(reportedMicros), basis: "reported" };
    }
    const p = this.get(id)?.price;
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
