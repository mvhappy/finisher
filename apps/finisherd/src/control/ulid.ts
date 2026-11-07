// Lowercase ULIDs (48-bit millisecond time + 80 random bits, Crockford base32), the form
// `fin-store::server_id` uses, for the environment id and other control-plane ids.
import { randomBytes } from "node:crypto";

const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";

/** A new lowercase ULID for `now` (milliseconds since the epoch). */
export function ulid(now: number = Date.now()): string {
  let time = "";
  let t = Math.floor(now);
  for (let i = 0; i < 10; i++) {
    time = ALPHABET[t % 32] + time;
    t = Math.floor(t / 32);
  }
  const bytes = randomBytes(10);
  let rand = "";
  // 80 bits → 16 characters, 5 bits at a time.
  let acc = 0n;
  for (const b of bytes) acc = (acc << 8n) | BigInt(b);
  for (let i = 0; i < 16; i++) {
    rand = ALPHABET[Number(acc & 31n)] + rand;
    acc >>= 5n;
  }
  return time + rand;
}

/** True for a 26-character Crockford base32 ULID (either case). */
export function isUlid(s: string): boolean {
  return /^[0-7][0-9a-hjkmnp-tv-z]{25}$/i.test(s);
}
