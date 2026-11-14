// One-time WebSocket tickets (spec §4.3, plan 5A Decision 23). `auth.ticket` mints 32 random
// bytes (base64url) bound to the caller's principal; the server keeps only `SHA-256(ticket)`
// with the principal and the expiry. A ticket is taken by one upgrade (`upgrade.ts`), whether
// or not it is still live, and never again; expired ones are swept every 10 s. Tickets are
// never logged, audited or put in an error. Each principal holds at most `MAX_TICKETS_PER_PRINCIPAL`
// live tickets (a new one drops its own oldest) and mints at most `MINT_RATE`; past the global
// `MAX_LIVE_TICKETS` a mint is refused rather than another principal's ticket dropped (review of
// Task 6, finding 4: one principal could evict the UI's ticket). Every `fin` and tool of the
// user is the one `local` principal, so `local`'s bucket and 8-ticket cap are per connection
// (the minting connection, `transport:clientId`), under a larger cap and bucket for `local` as
// a whole (review of Task 7, M1: one looping client must not lock out the others).
import { createHash, randomBytes } from "node:crypto";
import { type FinError, TICKET_DEFAULT_TTL_S, TICKET_MAX_TTL_S } from "@finisher/contracts";
import { Context, Effect, type Scope } from "effect";
import { finError } from "../errors.ts";
import type { Principal } from "../principal.ts";

/** A ticket is 32 random bytes in base64url (43 characters). */
export const TICKET_BYTES = 32;
const TICKET_SHAPE = /^[A-Za-z0-9_-]{43}$/;
/** How often expired tickets are dropped. */
export const SWEEP_MS = 10_000;
/** Live tickets kept at most, every principal together: past it a mint is refused (`TICKET_LIMIT`). */
export const MAX_LIVE_TICKETS = 1024;
/** Live tickets one principal holds at most: a newer one drops that principal's oldest. */
export const MAX_TICKETS_PER_PRINCIPAL = 8;
/** Mints per principal: a burst, then this many per second. */
export const MINT_RATE = { perSecond: 4, burst: 16 } as const;
/** Live tickets of `local` as a whole, every connection together: past it a mint is refused. */
export const MAX_TICKETS_LOCAL = 64;
/** Mints of `local` as a whole, every connection together. */
export const LOCAL_MINT_RATE = { perSecond: 16, burst: 64 } as const;
/** Principals with a mint bucket at once (oldest dropped past it). */
const MAX_BUCKETS = 1024;

/** The principal's key: `<kind>:<id>`. */
export const principalKeyOf = (p: Principal): string => `${p.kind}:${p.id}`;

/**
 * Who holds a ticket for its caps: a credential, or for `local` the minting connection
 * (`local:<transport>:<clientId>`; `local:` alone when the connection is not known).
 */
export const ownerOf = (p: Principal, connection?: string): string =>
  p.kind === "local" ? `local:${connection ?? ""}` : principalKeyOf(p);

/** A redeemed ticket: its principal and the owner it was minted by (`ownerOf`). */
export interface Redeemed {
  readonly principal: Principal;
  readonly owner: string;
}

/** `TICKET_LIMIT` (exit 5): try again shortly. */
function ticketLimit(reason: "rateLimited" | "full"): FinError {
  return finError(
    "TICKET_LIMIT",
    reason === "full" ? "the server holds as many live tickets as it can" : "too many tickets asked for in a short time",
    { reason },
    "Wait a few seconds and try again.",
  );
}

const digest = (ticket: string): string => createHash("sha256").update(ticket, "utf8").digest("base64url");

export class Tickets {
  readonly #live = new Map<string, { readonly principal: Principal; readonly owner: string; readonly key: string; readonly expiresAt: number }>();
  readonly #buckets = new Map<string, { tokens: number; at: number }>();
  readonly #now: () => number;
  readonly #dropHooks: Array<(pred: (p: Principal) => boolean) => void> = [];

  constructor(now: () => number = Date.now) {
    this.#now = now;
  }

  /**
   * A ticket for `principal`, live for `ttlS` seconds (default 30, at most 60, at least 1).
   * Throws `TICKET_LIMIT` past the owner's or the principal's mint rate, `local`'s cap or the
   * global cap. `connection` (`transport:clientId`) is the minting connection, the owner of a
   * `local` ticket.
   */
  mint(principal: Principal, ttlS: number = TICKET_DEFAULT_TTL_S, connection?: string): { ticket: string; expiresAt: number } {
    const ttl = Number.isFinite(ttlS) ? Math.min(TICKET_MAX_TTL_S, Math.max(1, Math.floor(ttlS))) : TICKET_DEFAULT_TTL_S;
    const owner = ownerOf(principal, connection);
    const key = principalKeyOf(principal);
    const local = principal.kind === "local";
    // `local` as a whole first, so a refused mint costs no connection's own token.
    if (local && !this.#peek(key, LOCAL_MINT_RATE)) throw ticketLimit("rateLimited");
    if (!this.#take(owner, MINT_RATE)) throw ticketLimit("rateLimited");
    if (local) this.#take(key, LOCAL_MINT_RATE);
    // The owner's own oldest first (the map keeps insertion order).
    const own = [...this.#live].filter(([, e]) => e.owner === owner);
    for (const [k] of own.slice(0, Math.max(0, own.length - MAX_TICKETS_PER_PRINCIPAL + 1))) this.#live.delete(k);
    if (local && [...this.#live.values()].filter((e) => e.key === key).length >= MAX_TICKETS_LOCAL) this.sweep();
    if (local && [...this.#live.values()].filter((e) => e.key === key).length >= MAX_TICKETS_LOCAL) throw ticketLimit("full");
    if (this.#live.size >= MAX_LIVE_TICKETS) this.sweep();
    if (this.#live.size >= MAX_LIVE_TICKETS) throw ticketLimit("full");
    const ticket = randomBytes(TICKET_BYTES).toString("base64url");
    const expiresAt = this.#now() + ttl * 1000;
    this.#live.set(digest(ticket), { principal, owner, key, expiresAt });
    return { ticket, expiresAt };
  }

  /** `id`'s bucket, refilled to now. */
  #bucket(id: string, rate: { readonly perSecond: number; readonly burst: number }): { tokens: number; at: number } {
    const now = this.#now();
    let b = this.#buckets.get(id);
    if (b === undefined) {
      b = { tokens: rate.burst, at: now };
      this.#buckets.set(id, b);
      if (this.#buckets.size > MAX_BUCKETS) this.#buckets.delete(this.#buckets.keys().next().value!);
    }
    b.tokens = Math.min(rate.burst, b.tokens + ((now - b.at) / 1000) * rate.perSecond);
    b.at = now;
    return b;
  }

  /** Whether `id`'s bucket holds a token, taking none. */
  #peek(id: string, rate: { readonly perSecond: number; readonly burst: number }): boolean {
    return this.#bucket(id, rate).tokens >= 1;
  }

  /** One mint from `id`'s bucket. */
  #take(id: string, rate: { readonly perSecond: number; readonly burst: number }): boolean {
    const b = this.#bucket(id, rate);
    if (b.tokens < 1) return false;
    b.tokens -= 1;
    return true;
  }

  /** The principal of a live `ticket`, which is used up; `undefined` for an unknown or expired one. */
  take(ticket: string): Principal | undefined {
    return this.redeem(ticket)?.principal;
  }

  /** `take`, with the owner the ticket was minted by (the WebSocket caps count per owner). */
  redeem(ticket: string): Redeemed | undefined {
    if (!TICKET_SHAPE.test(ticket)) return undefined;
    const key = digest(ticket);
    const entry = this.#live.get(key);
    if (entry === undefined) return undefined;
    this.#live.delete(key);
    return entry.expiresAt > this.#now() ? { principal: entry.principal, owner: entry.owner } : undefined;
  }

  /**
   * Runs `hook` with the same predicate whenever `dropWhere` drops the tickets of revoked
   * principals: what else a principal holds in memory (6B-1: its media capability URLs) goes
   * with its tickets, through every revoke path (credentials, devices, rotation). A hook that
   * throws never stops the revocation.
   */
  onDrop(hook: (pred: (p: Principal) => boolean) => void): void {
    this.#dropHooks.push(hook);
  }

  /** Drops the tickets of the principals `pred` selects (a credential revoked). */
  dropWhere(pred: (p: Principal) => boolean): number {
    let n = 0;
    for (const [key, entry] of this.#live) {
      if (!pred(entry.principal)) continue;
      this.#live.delete(key);
      n++;
    }
    for (const hook of this.#dropHooks) {
      try {
        hook(pred);
      } catch (e) {
        process.stderr.write(`finisherd: warning: a revocation hook failed: ${String(e)}\n`);
      }
    }
    return n;
  }

  /** Drops every expired ticket. */
  sweep(): void {
    const now = this.#now();
    for (const [key, entry] of this.#live) if (entry.expiresAt <= now) this.#live.delete(key);
  }

  get size(): number {
    return this.#live.size;
  }
}

/** A `Tickets` swept every `SWEEP_MS` for the life of the scope. */
export const makeTickets = (sweepMs: number = SWEEP_MS): Effect.Effect<Tickets, never, Scope.Scope> =>
  Effect.map(
    Effect.acquireRelease(
      Effect.sync(() => {
        const tickets = new Tickets();
        const timer = setInterval(() => tickets.sweep(), sweepMs);
        timer.unref();
        return { tickets, timer };
      }),
      ({ timer }) => Effect.sync(() => clearInterval(timer)),
    ),
    ({ tickets }) => tickets,
  );

export class TicketsTag extends Context.Service<TicketsTag, Tickets>()("finisherd/Tickets") {}
