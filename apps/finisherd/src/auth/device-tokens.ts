// Device access tokens (5B Decision 16, spec §5.3): `POST /auth/token` with a DPoP proof by a
// known, live device key gives `{accessToken, tokenType: "DPoP", expiresIn: 900}`. A token is
// 32 random bytes, base64url; only `SHA-256(token)` is kept, with `{deviceId, jkt, expiresAt}`,
// in memory: a restart needs a new proof, and there is no refresh token (any proof by a known key
// gets a token). At most 4 live per device; a newer one drops its oldest. A token is presented as
// `Authorization: DPoP <token>` with a proof by the same key carrying `ath` (`/auth/ws-ticket`).
// Tokens are never logged, audited, journalled or put in an error.
import { createHash, randomBytes } from "node:crypto";
import { ACCESS_TOKEN_TTL_S } from "@finisher/contracts";

export const MAX_TOKENS_PER_DEVICE = 4;

interface Held {
  readonly deviceId: string;
  readonly jkt: string;
  readonly expiresAt: number;
}

const digest = (token: string) => createHash("sha256").update(token, "ascii").digest("base64url");

export type TokenCheck = { deviceId: string } | { refused: "TOKEN_EXPIRED" | "DEVICE_REVOKED" | "DPOP_INVALID" };

export class DeviceTokens {
  readonly #clock: () => number;
  /** digest → holder, in issue order (a `Map` keeps it). */
  readonly #tokens = new Map<string, Held>();

  constructor(clock: () => number = Date.now) {
    this.#clock = clock;
  }

  issue(device: { readonly id: string; readonly jkt: string }): { accessToken: string; expiresIn: number } {
    const now = this.#clock();
    for (const [d, h] of this.#tokens) if (h.expiresAt <= now) this.#tokens.delete(d);
    const mine = [...this.#tokens].filter(([, h]) => h.deviceId === device.id);
    for (const [d] of mine.slice(0, Math.max(0, mine.length - (MAX_TOKENS_PER_DEVICE - 1)))) this.#tokens.delete(d);
    const accessToken = randomBytes(32).toString("base64url");
    this.#tokens.set(digest(accessToken), { deviceId: device.id, jkt: device.jkt, expiresAt: now + ACCESS_TOKEN_TTL_S * 1000 });
    return { accessToken, expiresIn: ACCESS_TOKEN_TTL_S };
  }

  /** The device a token stands for, when it is live and bound to `jkt` (the proof's key). */
  check(token: string, jkt: string): TokenCheck {
    const held = this.#tokens.get(digest(token));
    if (held === undefined) return { refused: this.#endedToken(token) ? "DEVICE_REVOKED" : "TOKEN_EXPIRED" };
    if (held.expiresAt <= this.#clock()) {
      this.#tokens.delete(digest(token));
      return { refused: "TOKEN_EXPIRED" };
    }
    if (held.jkt !== jkt) return { refused: "DPOP_INVALID" };
    return { deviceId: held.deviceId };
  }

  /** Digests of dropped tokens of ended devices, so a later use says why (bounded). */
  readonly #endedTokens = new Map<string, true>();

  #endedToken(token: string): boolean {
    return this.#endedTokens.has(digest(token));
  }

  /** Drops every token of `id` (its revocation): how many. */
  dropDevice(id: string): number {
    let n = 0;
    for (const [d, h] of this.#tokens) {
      if (h.deviceId !== id) continue;
      this.#tokens.delete(d);
      this.#endedTokens.set(d, true);
      n++;
    }
    while (this.#endedTokens.size > 4096) this.#endedTokens.delete(this.#endedTokens.keys().next().value!);
    return n;
  }

  /** Live tokens now. */
  get size(): number {
    return this.#tokens.size;
  }
}
