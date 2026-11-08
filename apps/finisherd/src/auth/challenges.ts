// The console's typed confirmations (spec §4.7, plan 5A Decision 30): `console.challenge` issues
// a 6-character Crockford code to one connection of a human principal (`console`, and since 5B
// a `device`), valid for 60 s; a confirmed RPC from that connection (`needsConfirmation`: a
// console's mutating human-only calls, a device's `server.stop`) must carry it back
// (`challenge`) or it is `CONFIRMATION_REQUIRED`. The codes live on the connection itself, so they end with it and
// cannot be answered from another one. A code is used up by the call it confirms; a wrong
// answer voids every code of the connection (a guess costs a new challenge). Codes are never
// logged, audited or put in an error.
import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import { CHALLENGE_TTL_S, type FinError } from "@finisher/contracts";
import type { Connection } from "../connections.ts";
import type { Where } from "../chain/fixes.ts";
import { finError } from "../errors.ts";

/** Crockford's base 32: no I, L, O or U. */
const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
export const CHALLENGE_LENGTH = 6;
/** Live codes kept per connection; a newer one drops the oldest. */
export const MAX_LIVE_CHALLENGES = 4;

interface Live {
  readonly digest: Buffer;
  readonly expiresAt: number;
}

const live = new WeakMap<Connection, Live[]>();

const digest = (code: string): Buffer => createHash("sha256").update(code, "utf8").digest();

/** The code as typed, read the Crockford way: case-insensitive, `I`/`L` as 1, `O` as 0, dashes and spaces dropped. */
export function normaliseCode(typed: string): string {
  return typed
    .toUpperCase()
    .replace(/[\s-]/g, "")
    .replace(/[IL]/g, "1")
    .replace(/O/g, "0");
}

/** A fresh code for `c`, valid for `CHALLENGE_TTL_S`. */
export function issueChallenge(c: Connection, now: number = Date.now()): { code: string; expiresAt: number } {
  const bytes = randomBytes(CHALLENGE_LENGTH);
  let code = "";
  // 256 is a multiple of 32: every symbol is equally likely.
  for (const b of bytes) code += CROCKFORD[b % 32];
  const expiresAt = now + CHALLENGE_TTL_S * 1000;
  const codes = (live.get(c) ?? []).filter((x) => x.expiresAt > now);
  codes.push({ digest: digest(code), expiresAt });
  while (codes.length > MAX_LIVE_CHALLENGES) codes.shift();
  live.set(c, codes);
  return { code, expiresAt };
}

export type ChallengeRefusal = "missing" | "wrong" | "expired";

/**
 * Takes `typed` as the answer to one of `c`'s live codes: `undefined` when it matches (the code
 * is used up), the reason otherwise.
 */
export function answerChallenge(c: Connection | undefined, typed: unknown, now: number = Date.now()): ChallengeRefusal | undefined {
  if (c === undefined || typeof typed !== "string" || typed.length === 0) return "missing";
  const codes = live.get(c) ?? [];
  const want = digest(normaliseCode(typed));
  const hit = codes.findIndex((x) => timingSafeEqual(x.digest, want));
  if (hit < 0) {
    live.delete(c);
    return "wrong";
  }
  const [taken] = codes.splice(hit, 1);
  return taken!.expiresAt > now ? undefined : "expired";
}

/**
 * `CONFIRMATION_REQUIRED` (exit 7): the fix speaks to the person, in words: at the server's
 * console, or in the app on a paired device (5B: a device confirms `server.stop`).
 */
export function confirmationRequired(tag: string, reason: ChallengeRefusal, where: Extract<Where, "console" | "app"> = "console"): FinError {
  const why = reason === "missing" ? "none was sent" : reason === "wrong" ? "the code did not match" : "the code expired";
  return finError(
    "CONFIRMATION_REQUIRED",
    `${tag} needs a typed confirmation from ${where === "app" ? "this device" : "the console"} (${why})`,
    { tag, reason, where },
    where === "app"
      ? "This change needs you to confirm it: the app shows a short code, and the change goes ahead once that code is typed back on this device."
      : "This change needs the person at the server to confirm it: the server's console shows a short code, and the change goes ahead once that code is typed back in the same session.",
  );
}

/** Whether `kind` may ask for a code (`console.challenge`): the human principals, `console` and `device` (5B Decision 22). */
export function mayChallenge(principal: { readonly kind: string; readonly human: boolean }): boolean {
  return principal.human && (principal.kind === "console" || principal.kind === "device");
}

/** Credentials each console connection minted (review of Tasks 8–9, M2). */
const minted = new WeakMap<Connection, Set<string>>();

/** Notes that `c` minted credential `id`: it may revoke it again without a new code. */
export function noteMinted(c: Connection | undefined, id: string): void {
  if (c === undefined) return;
  const ids = minted.get(c) ?? new Set<string>();
  ids.add(id);
  minted.set(c, ids);
}

/**
 * Whether `auth.credential.revoke` of `payload.id` takes back a credential this same connection
 * minted (the console undoing a mint whose file it could not write): confirmed by that mint's
 * code already, and revoking only takes rights away.
 */
export function undoesOwnMint(c: Connection | undefined, tag: string, payload: unknown): boolean {
  if (tag !== "auth.credential.revoke" || c === undefined) return false;
  const id = (payload as { id?: unknown } | null)?.id;
  return typeof id === "string" && minted.get(c)?.has(id) === true;
}
