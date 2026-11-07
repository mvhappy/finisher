// OpenRouter's public models API as a price and metadata cache (owner decision after review
// 7A-1): `GET <base>/models` (no key) gives every model's window, maximum output, supported
// parameters, prices and, for open weights, a Hugging Face id. The answer is kept in one file
// in server data and refreshed when stale; a model it does not list has an unknown price and
// is charged after the call from OpenRouter's reported `usage.cost`.
import { randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { finError } from "../../errors.ts";
import type { Clock } from "../clock.ts";
import type { Price } from "./catalogue.ts";

export interface ListedModel {
  readonly id: string;
  readonly window: number;
  readonly maxOutput: number;
  readonly price: Price;
  readonly tools: boolean;
  readonly thinking: boolean;
  readonly images: boolean;
  /** OpenRouter names a Hugging Face repository for it. */
  readonly openWeights: boolean;
}

interface CacheFile {
  readonly v: 1;
  readonly fetchedAt: number;
  readonly models: ListedModel[];
}

/** USD per token as OpenRouter writes it ("0.000002") → micros per million tokens. */
function micros(v: unknown): number {
  const n = typeof v === "string" ? Number(v) : typeof v === "number" ? v : 0;
  return Number.isFinite(n) && n > 0 ? Math.round(n * 1e12) : 0;
}

/** The models of a `GET /models` answer; entries that do not parse are skipped. */
export function parseModels(body: unknown): ListedModel[] {
  const data = (body as { data?: unknown })?.data;
  if (!Array.isArray(data)) throw finError("PROVIDER_PROTOCOL", "OpenRouter's models list has no data", {});
  const out: ListedModel[] = [];
  for (const raw of data as Record<string, unknown>[]) {
    if (typeof raw?.id !== "string" || raw.id === "") continue;
    const params = Array.isArray(raw.supported_parameters) ? (raw.supported_parameters as unknown[]) : [];
    const pricing = (raw.pricing ?? {}) as Record<string, unknown>;
    const top = (raw.top_provider ?? {}) as Record<string, unknown>;
    const modalities = ((raw.architecture ?? {}) as Record<string, unknown>).input_modalities;
    const window = typeof raw.context_length === "number" && raw.context_length > 0 ? raw.context_length : 0;
    if (window === 0) continue;
    const maxOutput = typeof top.max_completion_tokens === "number" && top.max_completion_tokens > 0 ? top.max_completion_tokens : Math.min(4096, window);
    out.push({
      id: raw.id,
      window,
      maxOutput,
      price: {
        input: micros(pricing.prompt),
        output: micros(pricing.completion),
        cacheRead: micros(pricing.input_cache_read),
        cacheWrite: micros(pricing.input_cache_write),
      },
      tools: params.includes("tools"),
      thinking: params.includes("reasoning"),
      images: Array.isArray(modalities) && modalities.includes("image"),
      openWeights: typeof raw.hugging_face_id === "string" && raw.hugging_face_id !== "",
    });
  }
  return out;
}

export class OpenRouterPrices {
  private byId = new Map<string, ListedModel>();
  private at: number | null = null;
  /** `<server-data>/agents/openrouter-models.json` */
  readonly file: string;
  private readonly clock: Clock;

  constructor(file: string, clock: Clock) {
    this.file = file;
    this.clock = clock;
    try {
      const doc = JSON.parse(readFileSync(file, "utf8")) as CacheFile;
      if (doc?.v === 1 && Array.isArray(doc.models) && typeof doc.fetchedAt === "number") {
        this.byId = new Map(doc.models.map((m) => [m.id, m]));
        this.at = doc.fetchedAt;
      }
    } catch {
      // No cache yet, or an unreadable one: refreshed on first use.
    }
  }

  get(id: string): ListedModel | undefined {
    return this.byId.get(id);
  }

  get fetchedAt(): number | null {
    return this.at;
  }

  /** Whether the cache is older than `maxAgeMs` (default a day) or missing. */
  stale(maxAgeMs = 24 * 60 * 60_000): boolean {
    return this.at === null || this.clock.now() - this.at > maxAgeMs;
  }

  /** Fetches the list (no key) and replaces the cache; returns the number of models. */
  async refresh(baseUrl: string, fetchImpl: typeof fetch = fetch, signal?: AbortSignal): Promise<number> {
    const res = await fetchImpl(`${baseUrl.replace(/\/+$/, "")}/models`, { headers: { accept: "application/json" }, ...(signal ? { signal } : {}) });
    if (!res.ok) throw finError("PROVIDER_UNAVAILABLE", `OpenRouter's models list answered ${res.status}`, { status: res.status });
    const models = parseModels(await res.json());
    const doc: CacheFile = { v: 1, fetchedAt: this.clock.now(), models };
    mkdirSync(dirname(this.file), { recursive: true, mode: 0o700 });
    const tmp = `${this.file}.${randomBytes(6).toString("hex")}.tmp`;
    writeFileSync(tmp, `${JSON.stringify(doc)}\n`, { mode: 0o600 });
    renameSync(tmp, this.file);
    this.byId = new Map(models.map((m) => [m.id, m]));
    this.at = doc.fetchedAt;
    return models.length;
  }
}
