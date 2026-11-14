// Server nonces for DPoP proofs (5B Decision 15, RFC 9449 §8): 32 random bytes, base64url, one
// per fixed 5-minute window of the clock; the previous window's stays valid, so a proof made
// just before a boundary still passes. A nonce of window `e` is accepted exactly until the
// start of window `e + 2` (`expiresAt`), which is when the verifier forgets the `jti`s it
// carried, so no proof outlives the memory of its replay (review of Task 7). A clock that runs
// backwards never brings an older window back. Every HTTP answer of the front carries the
// current nonce in `DPoP-Nonce`; a nonce is never logged, audited or put in an error.
import { randomBytes } from "node:crypto";

export const NONCE_ROTATE_MS = 5 * 60_000;

export class Nonces {
  readonly #clock: () => number;
  /** The newest window seen, and its nonce; the one before, when it was issued. */
  #epoch = Number.NEGATIVE_INFINITY;
  #current = "";
  #previous: string | undefined;

  constructor(clock: () => number = Date.now) {
    this.#clock = clock;
    this.#advance();
  }

  #advance(): void {
    const epoch = Math.floor(this.#clock() / NONCE_ROTATE_MS);
    if (epoch <= this.#epoch) return;
    // Only the window just before stays valid; after an idle gap, none.
    this.#previous = epoch === this.#epoch + 1 ? this.#current : undefined;
    this.#epoch = epoch;
    this.#current = randomBytes(32).toString("base64url");
  }

  current(): string {
    this.#advance();
    return this.#current;
  }

  /** Whether `n` is the current window's nonce or the previous one's. */
  accepts(n: string): boolean {
    return this.expiresAt(n) !== undefined;
  }

  /** When `n` stops being accepted (ms), or `undefined` when it is not accepted now. */
  expiresAt(n: string): number | undefined {
    this.#advance();
    if (n.length === 0) return undefined;
    if (n === this.#current) return (this.#epoch + 2) * NONCE_ROTATE_MS;
    if (this.#previous !== undefined && n === this.#previous) return (this.#epoch + 1) * NONCE_ROTATE_MS;
    return undefined;
  }
}
