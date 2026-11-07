// Injected time and ids (plan 7A: every runtime module takes them, so timeouts, budgets and
// retention are tested without sleeping).
import { randomBytes } from "node:crypto";

export interface Clock {
  /** Milliseconds since the Unix epoch. */
  now(): number;
}

export const systemClock: Clock = { now: () => Date.now() };

/** A clock tests move by hand. */
export function manualClock(start = 1_794_571_200_000): Clock & { advance(ms: number): void; set(ms: number): void } {
  let t = start;
  return {
    now: () => t,
    advance: (ms) => {
      t += ms;
    },
    set: (ms) => {
      t = ms;
    },
  };
}

export interface Ids {
  /** A new canonical ULID. */
  ulid(): string;
}

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** The canonical text of a ULID for `ms` and 80 random bits. */
export function ulidAt(ms: number, random: Buffer = randomBytes(10)): string {
  let time = "";
  let t = ms;
  for (let i = 0; i < 10; i++) {
    time = CROCKFORD[t % 32] + time;
    t = Math.floor(t / 32);
  }
  let bits = 0n;
  for (const b of random.subarray(0, 10)) bits = (bits << 8n) | BigInt(b);
  let rand = "";
  for (let i = 0; i < 16; i++) {
    rand = CROCKFORD[Number(bits & 31n)] + rand;
    bits >>= 5n;
  }
  return time + rand;
}

export function systemIds(clock: Clock = systemClock): Ids {
  return { ulid: () => ulidAt(clock.now()) };
}
