// Library pools (plan 5D Decision 32; spec §6.3): per **volume key**, so two libraries on one
// NAS share its disks. `io:<key>.scan` (1: full scans), `io:<key>.poll` (1: polls, freshness
// passes and watch rescans; a separate slot so a poll never waits hours behind a full scan) and
// `io:<key>.read` (2 for network and sync roots, 4 for local ones: probes, hashes and every
// stage that reads originals). Bandwidth (Decision 36) is split evenly over the read slots.
import type { PoolName } from "../jobs/kinds.ts";
import type { LibraryStorage } from "./registry.ts";

/** A library's pools (frozen for sub-projects 6 and 8: `Libraries.pools(id)`). */
export interface LibraryPools {
  readonly scan: PoolName;
  readonly poll: PoolName;
  readonly read: PoolName;
}

const KEY = /^[0-9a-f]{1,64}$/;

/** The pools of a volume. */
export function libraryPools(volumeKey: string): LibraryPools {
  if (!KEY.test(volumeKey)) throw new Error(`library pools: bad volume key ${JSON.stringify(volumeKey)}`);
  return { scan: `io:${volumeKey}.scan`, poll: `io:${volumeKey}.poll`, read: `io:${volumeKey}.read` };
}

/** Read slots per volume: 4 for local roots, 2 for network and sync ones. */
export const readSlots = (storage: LibraryStorage): number => (storage === "local" ? 4 : 2);

/** Walker threads (Decision 9): 64 for local roots, 32 for network and sync ones. */
export const scanThreads = (storage: LibraryStorage): number => (storage === "local" ? 64 : 32);

/**
 * One read slot's share of the library's bandwidth (Decision 36), in bytes per second, or
 * `undefined` when the library has no budget. At least 1 byte per second.
 */
export function readBpsPerSlot(bandwidthMiBs: number | undefined, slots: number): number | undefined {
  if (bandwidthMiBs === undefined) return undefined;
  return Math.max(1, Math.floor((bandwidthMiBs * 1024 * 1024) / Math.max(1, slots)));
}

interface Member {
  readonly key: string;
  readonly storage: LibraryStorage;
}

/**
 * Which libraries share which volume's pools. `add` defines (or resizes) the volume's pools:
 * scan and poll 1, read the smallest of its libraries' `readSlots` (a NAS shared by a network
 * and a local-overridden library gets the gentler count). `remove` resizes what is left; a
 * volume with no library keeps its pools (jobs queued on them may still drain).
 */
export class LibraryPoolBook {
  readonly #define: (name: PoolName, size: number) => void;
  readonly #members = new Map<string, Member>();

  constructor(define: (name: PoolName, size: number) => void) {
    this.#define = define;
  }

  /** Defines the pools of `lib`'s volume and answers them. */
  add(lib: { readonly id: string; readonly volume: { readonly key: string }; readonly storage: LibraryStorage }): LibraryPools {
    const pools = libraryPools(lib.volume.key);
    const before = this.#members.get(lib.id);
    this.#members.set(lib.id, { key: lib.volume.key, storage: lib.storage });
    if (before !== undefined && before.key !== lib.volume.key) this.#resize(before.key);
    this.#resize(lib.volume.key);
    return pools;
  }

  remove(id: string): void {
    const m = this.#members.get(id);
    if (m === undefined) return;
    this.#members.delete(id);
    this.#resize(m.key);
  }

  /** The library's pools, once added. */
  pools(id: string): LibraryPools | undefined {
    const m = this.#members.get(id);
    return m === undefined ? undefined : libraryPools(m.key);
  }

  /** The read pool's size for `lib`'s volume (what its bandwidth is split over). */
  readSlots(id: string): number | undefined {
    const m = this.#members.get(id);
    return m === undefined ? undefined : this.#readSize(m.key);
  }

  #readSize(key: string): number | undefined {
    let size: number | undefined;
    for (const m of this.#members.values()) if (m.key === key) size = Math.min(size ?? Infinity, readSlots(m.storage));
    return size;
  }

  #resize(key: string): void {
    const read = this.#readSize(key);
    if (read === undefined) return;
    const pools = libraryPools(key);
    this.#define(pools.scan, 1);
    this.#define(pools.poll, 1);
    this.#define(pools.read, read);
  }
}
