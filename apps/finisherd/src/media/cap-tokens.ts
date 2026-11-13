// Capability URLs for media bytes (6B-1 plan Decisions 13 and 14; spec 6 §7.3). `media.url
// {key, file}` is the one way to a URL: it needs `project.read` (the chain's) and reach over one
// of the projects the entry serves (`MediaCache.projectsOf`), and a key the caller cannot reach
// is `NOT_FOUND`, the answer for a key that is not there. The token is 32 random bytes in
// base64url, held in memory only (as its SHA-256, like a ticket), bound to `{key, file,
// principal}` and live for `CAP_TTL_MS`; a principal holds `MAX_CAP_TOKENS_PER_PRINCIPAL` at
// once (a newer one drops its oldest) and loses them all when it is revoked (`Tickets.onDrop`,
// the one revoke path). The URL is origin-relative: the client resolves it against the HTTP
// origin it reached the server on (the loopback listener, or the remote one over TLS).
import { createHash, randomBytes } from "node:crypto";
import type { FinError } from "@finisher/contracts";
import { Context, Effect } from "effect";
import { attempt, finError } from "../errors.ts";
import type { Principal } from "../principal.ts";
import { principalKeyOf } from "../auth/tickets.ts";
import type { MediaCacheService } from "./cache.ts";

/** How long a capability URL lives, at most (spec §7.3). */
export const CAP_TTL_MS = 10 * 60_000;
/** Live tokens one principal holds: a newer one drops its oldest. */
export const MAX_CAP_TOKENS_PER_PRINCIPAL = 64;
/** A token is 32 random bytes in base64url. */
export const CAP_TOKEN_BYTES = 32;
const TOKEN_SHAPE = /^[A-Za-z0-9_-]{43}$/;

export interface CapGrant {
  readonly key: string;
  readonly file: string;
  readonly principal: Principal;
  readonly expiresAt: number;
}

interface Held extends CapGrant {
  readonly owner: string;
}

const digest = (token: string): string => createHash("sha256").update(token, "utf8").digest("base64url");

export interface CapTokensOptions {
  readonly now?: () => number;
  readonly ttlMs?: number;
  readonly max?: number;
}

export class CapTokens {
  readonly #live = new Map<string, Held>();
  readonly #now: () => number;
  readonly #ttl: number;
  readonly #max: number;

  constructor(options: CapTokensOptions = {}) {
    this.#now = options.now ?? Date.now;
    this.#ttl = Math.min(options.ttlMs ?? CAP_TTL_MS, CAP_TTL_MS);
    this.#max = options.max ?? MAX_CAP_TOKENS_PER_PRINCIPAL;
  }

  /** A token for `file` of the entry `key`, for `principal`. Reach is the caller's to have checked. */
  mint(principal: Principal, key: string, file: string): { token: string; expiresAt: number } {
    const now = this.#now();
    this.#sweep(now);
    const owner = principalKeyOf(principal);
    // The principal's own oldest first (the map keeps insertion order).
    const own = [...this.#live].filter(([, g]) => g.owner === owner);
    for (const [d] of own.slice(0, Math.max(0, own.length - this.#max + 1))) this.#live.delete(d);
    const token = randomBytes(CAP_TOKEN_BYTES).toString("base64url");
    const expiresAt = now + this.#ttl;
    this.#live.set(digest(token), { key, file, principal, expiresAt, owner });
    return { token, expiresAt };
  }

  /** The grant of a live token; `undefined` for an unknown, malformed, expired or dropped one. */
  resolve(token: string): CapGrant | undefined {
    if (!TOKEN_SHAPE.test(token)) return undefined;
    const d = digest(token);
    const held = this.#live.get(d);
    if (held === undefined) return undefined;
    if (held.expiresAt <= this.#now()) {
      this.#live.delete(d);
      return undefined;
    }
    return held;
  }

  /** Drops the tokens of the principals `pred` selects (a credential revoked); how many. */
  dropWhere(pred: (p: Principal) => boolean): number {
    let n = 0;
    for (const [d, g] of this.#live) {
      if (!pred(g.principal)) continue;
      this.#live.delete(d);
      n++;
    }
    return n;
  }

  #sweep(now: number): void {
    for (const [d, g] of this.#live) if (g.expiresAt <= now) this.#live.delete(d);
  }

  get size(): number {
    return this.#live.size;
  }
}

export class CapTokensTag extends Context.Service<CapTokensTag, CapTokens>()("finisherd/CapTokens") {}

/** The one answer to a key the caller cannot reach and to a key or file that is not there. */
const notFound = (): FinError => finError("NOT_FOUND", "no such media file", {}, "ask for a file an entry lists, by the key a media job gave");

/** Whether `p` reaches an entry serving `projects`: every project (`"*"`), or one of them. */
export function reaches(p: Principal, projects: ReadonlyArray<string>): boolean {
  if (p.projects === "*") return true;
  const mine = p.projects;
  return projects.some((id) => mine.includes(id));
}

export interface MediaUrlDeps {
  readonly cache: MediaCacheService;
  readonly tokens: CapTokens;
}

/**
 * `media.url`: a capability URL for `file` of the entry `key` (the chain has checked `project.read`).
 * A key outside the caller's reach, a key that is not there and a file the entry does not list
 * are one `NOT_FOUND`; a cache that did not open is its `CACHE_IO`.
 */
export function mediaUrl(deps: MediaUrlDeps, principal: Principal, req: { readonly key: string; readonly file: string }): Effect.Effect<{ url: string; expiresAt: number }, FinError> {
  return Effect.gen(function* () {
    const cache = deps.cache.cache;
    // The unavailable service's `status` fails with the `CACHE_IO` of its open.
    if (cache === undefined) return yield* Effect.flatMap(deps.cache.status(), () => Effect.fail(notFound()));
    const meta = yield* attempt(() => cache.lookup(req.key));
    if (meta === null || !meta.files.some((f) => f.name === req.file)) return yield* Effect.fail(notFound());
    const projects = yield* attempt(() => cache.projectsOf(req.key));
    if (!reaches(principal, projects)) return yield* Effect.fail(notFound());
    const { token, expiresAt } = deps.tokens.mint(principal, req.key, req.file);
    return { url: `/media/cap/${token}/${encodeURIComponent(req.file)}`, expiresAt };
  });
}
