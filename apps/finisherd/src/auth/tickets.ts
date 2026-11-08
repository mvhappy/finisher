// One-time WebSocket tickets (spec §4.3, plan 5A Decision 23). `auth.ticket` mints 32 random
// bytes (base64url) bound to the caller's principal; the server keeps only `SHA-256(ticket)`
// with the principal and the expiry. A ticket is taken by one upgrade (`upgrade.ts`), whether
// or not it is still live, and never again; expired ones are swept every 10 s. Tickets are
// never logged, audited or put in an error.
import { createHash, randomBytes } from "node:crypto";
import { TICKET_DEFAULT_TTL_S, TICKET_MAX_TTL_S } from "@finisher/contracts";
import { Context, Effect, type Scope } from "effect";
import type { Principal } from "../principal.ts";

/** A ticket is 32 random bytes in base64url (43 characters). */
export const TICKET_BYTES = 32;
const TICKET_SHAPE = /^[A-Za-z0-9_-]{43}$/;
/** How often expired tickets are dropped. */
export const SWEEP_MS = 10_000;
/** Live tickets kept at most: past it the oldest is dropped (a minting flood bounds memory). */
export const MAX_LIVE_TICKETS = 1024;

const digest = (ticket: string): string => createHash("sha256").update(ticket, "utf8").digest("base64url");

export class Tickets {
  readonly #live = new Map<string, { readonly principal: Principal; readonly expiresAt: number }>();
  readonly #now: () => number;

  constructor(now: () => number = Date.now) {
    this.#now = now;
  }

  /** A ticket for `principal`, live for `ttlS` seconds (default 30, at most 60, at least 1). */
  mint(principal: Principal, ttlS: number = TICKET_DEFAULT_TTL_S): { ticket: string; expiresAt: number } {
    const ttl = Number.isFinite(ttlS) ? Math.min(TICKET_MAX_TTL_S, Math.max(1, Math.floor(ttlS))) : TICKET_DEFAULT_TTL_S;
    if (this.#live.size >= MAX_LIVE_TICKETS) this.sweep();
    while (this.#live.size >= MAX_LIVE_TICKETS) {
      const oldest = this.#live.keys().next().value;
      if (oldest === undefined) break;
      this.#live.delete(oldest);
    }
    const ticket = randomBytes(TICKET_BYTES).toString("base64url");
    const expiresAt = this.#now() + ttl * 1000;
    this.#live.set(digest(ticket), { principal, expiresAt });
    return { ticket, expiresAt };
  }

  /** The principal of a live `ticket`, which is used up; `undefined` for an unknown or expired one. */
  take(ticket: string): Principal | undefined {
    if (!TICKET_SHAPE.test(ticket)) return undefined;
    const key = digest(ticket);
    const entry = this.#live.get(key);
    if (entry === undefined) return undefined;
    this.#live.delete(key);
    return entry.expiresAt > this.#now() ? entry.principal : undefined;
  }

  /** Drops the tickets of the principals `pred` selects (a credential revoked). */
  dropWhere(pred: (p: Principal) => boolean): number {
    let n = 0;
    for (const [key, entry] of this.#live) {
      if (!pred(entry.principal)) continue;
      this.#live.delete(key);
      n++;
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
