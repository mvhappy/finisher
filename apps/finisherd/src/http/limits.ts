// Rate limits of the HTTP front (5B Decision 13): token buckets keyed by remote address,
// device or the whole server. Pairing redeems 10/min per address and 30/min per server
// (`PAIRING_RATE_LIMITED`), token and ticket requests 30/min per device and 120/min per server
// (`TOKEN_RATE_LIMITED`); at most 4,096 buckets, the least recently used dropped first.

export interface Rate {
  /** Tokens added per minute. */
  readonly perMinute: number;
  /** The most a bucket holds (a burst). */
  readonly burst: number;
}

export const MAX_BUCKETS = 4096;
export const PAIR_PER_ADDRESS: Rate = { perMinute: 10, burst: 10 };
export const PAIR_PER_SERVER: Rate = { perMinute: 30, burst: 30 };
export const TOKEN_PER_DEVICE: Rate = { perMinute: 30, burst: 30 };
export const TOKEN_PER_SERVER: Rate = { perMinute: 120, burst: 120 };

interface Bucket {
  tokens: number;
  at: number;
}

export class Buckets {
  readonly #rate: Rate;
  readonly #max: number;
  readonly #clock: () => number;
  /** In use order: a `Map` keeps insertion order, and a take moves its key to the end. */
  readonly #buckets = new Map<string, Bucket>();

  constructor(rate: Rate, max: number = MAX_BUCKETS, clock: () => number = Date.now) {
    this.#rate = rate;
    this.#max = max;
    this.#clock = clock;
  }

  /** Takes one token for `key`: true when there was one. */
  take(key: string): boolean {
    const now = this.#clock();
    const b = this.#buckets.get(key) ?? { tokens: this.#rate.burst, at: now };
    this.#buckets.delete(key);
    b.tokens = Math.min(this.#rate.burst, b.tokens + ((now - b.at) / 60_000) * this.#rate.perMinute);
    b.at = now;
    const ok = b.tokens >= 1;
    if (ok) b.tokens -= 1;
    this.#buckets.set(key, b);
    while (this.#buckets.size > this.#max) this.#buckets.delete(this.#buckets.keys().next().value!);
    return ok;
  }

  /** Seconds until `key` has a token again (a `Retry-After`), at least 1. */
  retryAfterS(key: string): number {
    const b = this.#buckets.get(key);
    if (b === undefined || b.tokens >= 1) return 1;
    return Math.max(1, Math.ceil(((1 - b.tokens) / this.#rate.perMinute) * 60));
  }

  get size(): number {
    return this.#buckets.size;
  }
}
