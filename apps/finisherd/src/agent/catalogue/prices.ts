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
  /** Absent when OpenRouter gives no usable price (missing, or `-1` for a router such as `openrouter/auto`): the model's price is unknown, never free. */
  readonly price?: Price;
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

/** `<maker>/<model>`, with an optional `:variant` (OpenRouter's id shape). */
export const OPENROUTER_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}\/[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/;

/** The most a models listing may weigh (streamed: a larger one is refused, not buffered). */
export const LISTING_MAX_BYTES = 16 * 1024 * 1024;
/** After a failed refresh, none is tried for this long. */
export const REFRESH_BACKOFF_MS = 15 * 60_000;

/**
 * USD per token as OpenRouter writes it ("0.000002") → micros per million tokens. A missing,
 * blank, non-numeric or negative value (`"-1"` marks a router with a variable price) is
 * `undefined`: only a literal zero is free.
 */
function micros(v: unknown): number | undefined {
  if (typeof v === "string" ? v.trim() === "" : typeof v !== "number") return undefined;
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? Math.round(n * 1e12) : undefined;
}

/** The body as JSON, never holding more than `max` bytes. */
async function readJson(res: Response, max: number): Promise<unknown> {
  const declared = Number(res.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > max) throw finError("PROVIDER_PROTOCOL", "OpenRouter's models list is too large", { max });
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (res.body !== null) {
    const reader = res.body.getReader();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > max) {
        await reader.cancel().catch(() => undefined);
        throw finError("PROVIDER_PROTOCOL", "OpenRouter's models list is too large", { max });
      }
      chunks.push(value);
    }
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    throw finError("PROVIDER_PROTOCOL", "OpenRouter's models list is not JSON", {});
  }
}

/** The models of a `GET /models` answer; entries that do not parse are skipped. */
export function parseModels(body: unknown): ListedModel[] {
  const data = (body as { data?: unknown })?.data;
  if (!Array.isArray(data)) throw finError("PROVIDER_PROTOCOL", "OpenRouter's models list has no data", {});
  const out: ListedModel[] = [];
  for (const raw of data as Record<string, unknown>[]) {
    if (typeof raw?.id !== "string" || !OPENROUTER_ID.test(raw.id)) continue;
    const params = Array.isArray(raw.supported_parameters) ? (raw.supported_parameters as unknown[]) : [];
    const pricing = (raw.pricing ?? {}) as Record<string, unknown>;
    const top = (raw.top_provider ?? {}) as Record<string, unknown>;
    const modalities = ((raw.architecture ?? {}) as Record<string, unknown>).input_modalities;
    const window = typeof raw.context_length === "number" && raw.context_length > 0 ? raw.context_length : 0;
    if (window === 0) continue;
    const maxOutput = typeof top.max_completion_tokens === "number" && top.max_completion_tokens > 0 ? top.max_completion_tokens : Math.min(4096, window);
    const input = micros(pricing.prompt);
    const output = micros(pricing.completion);
    out.push({
      id: raw.id,
      window,
      maxOutput,
      // Both rates, or no price: a router's "-1" or an absent rate is never read as free. The
      // cache rates default to zero (no cache discount is promised without them).
      ...(input === undefined || output === undefined
        ? {}
        : { price: { input, output, cacheRead: micros(pricing.input_cache_read) ?? 0, cacheWrite: micros(pricing.input_cache_write) ?? 0 } }),
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
  private inflight: Promise<number> | undefined;
  private failedAt: number | null = null;
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

  /**
   * Refreshes the list when it is stale, unless a refresh is under way (this one waits for it) or
   * the last attempt failed less than [`REFRESH_BACKOFF_MS`] ago. Never rejects.
   */
  async refreshIfStale(baseUrl: string, fetchImpl: typeof fetch = fetch, signal?: AbortSignal, onError?: (e: unknown) => void): Promise<void> {
    if (!this.stale()) return;
    if (this.inflight === undefined && this.failedAt !== null && this.clock.now() - this.failedAt < REFRESH_BACKOFF_MS) return;
    await this.refresh(baseUrl, fetchImpl, signal).catch((e: unknown) => onError?.(e));
  }

  /** Fetches the list (no key) and replaces the cache; returns the number of models. One fetch at a time: callers share it. */
  refresh(baseUrl: string, fetchImpl: typeof fetch = fetch, signal?: AbortSignal): Promise<number> {
    this.inflight ??= this.fetchList(baseUrl, fetchImpl, signal).then(
      (n) => {
        this.failedAt = null;
        this.inflight = undefined;
        return n;
      },
      (e: unknown) => {
        this.failedAt = this.clock.now();
        this.inflight = undefined;
        throw e;
      },
    );
    return this.inflight;
  }

  private async fetchList(baseUrl: string, fetchImpl: typeof fetch, signal?: AbortSignal): Promise<number> {
    const res = await fetchImpl(`${baseUrl.replace(/\/+$/, "")}/models`, { headers: { accept: "application/json" }, ...(signal ? { signal } : {}) });
    if (!res.ok) throw finError("PROVIDER_UNAVAILABLE", `OpenRouter's models list answered ${res.status}`, { status: res.status });
    const models = parseModels(await readJson(res, LISTING_MAX_BYTES));
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
