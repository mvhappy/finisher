// Resource pools (spec §6.3, 5C-1 plan Decision 33): named slot counters. A job takes one
// slot per name it lists (a name listed twice takes two), all or nothing. `io:<key>` pools
// are defined by their owner (5D); a job that needs an undefined pool cannot run.
export interface PoolView {
  readonly name: string;
  readonly size: number;
  readonly used: number;
  readonly stuck: boolean;
}

export interface Pools {
  size(name: string): number | undefined;
  used(name: string): number;
  /** Defines or resizes a pool (a smaller size leaves the slots in use taken). */
  define(name: string, size: number): void;
  /**
   * Takes one slot per listed name, or none. `overrun` lets a pool go over its size by that
   * many slots (a warm job's `cpu`, Decision 35). A stuck or undefined pool refuses.
   */
  take(names: ReadonlyArray<string>, overrun?: Readonly<Record<string, number>>): boolean;
  give(names: ReadonlyArray<string>): void;
  /** Whether nothing may be dispatched on the pool (two abandoned processes, Decision 18). */
  stuck(name: string): boolean;
  markStuck(name: string, stuck: boolean): void;
  snapshot(): PoolView[];
}

const NAME = /^(cpu|gpu|hwenc|net|io:[A-Za-z0-9._:-]{1,128})$/;

/** How many slots of each pool `names` asks for. */
export function slotCounts(names: ReadonlyArray<string>): Map<string, number> {
  const out = new Map<string, number>();
  for (const n of names) out.set(n, (out.get(n) ?? 0) + 1);
  return out;
}

/** Pools with the given sizes. */
export function makePools(sizes: Readonly<Record<string, number>> = {}): Pools {
  const size = new Map<string, number>();
  const used = new Map<string, number>();
  const stuck = new Set<string>();
  const pools: Pools = {
    size: (name) => size.get(name),
    used: (name) => used.get(name) ?? 0,
    define(name, n) {
      if (!NAME.test(name)) throw new Error(`job pools: bad pool name ${JSON.stringify(name)}`);
      if (!Number.isInteger(n) || n < 0) throw new Error(`job pools: pool ${name} needs a size ≥ 0`);
      size.set(name, n);
    },
    take(names, overrun = {}) {
      const want = slotCounts(names);
      for (const [name, n] of want) {
        const cap = size.get(name);
        if (cap === undefined || stuck.has(name)) return false;
        if ((used.get(name) ?? 0) + n > cap + (overrun[name] ?? 0)) return false;
      }
      for (const [name, n] of want) used.set(name, (used.get(name) ?? 0) + n);
      return true;
    },
    give(names) {
      for (const [name, n] of slotCounts(names)) used.set(name, Math.max(0, (used.get(name) ?? 0) - n));
    },
    stuck: (name) => stuck.has(name),
    markStuck(name, s) {
      if (s) stuck.add(name);
      else stuck.delete(name);
    },
    snapshot: () =>
      [...size.keys()].sort().map((name) => ({ name, size: size.get(name) ?? 0, used: used.get(name) ?? 0, stuck: stuck.has(name) })),
  };
  for (const [name, n] of Object.entries(sizes)) pools.define(name, n);
  return pools;
}

/** `cpu` = max(1, cores − 1), `gpu` 1, `hwenc` 2 (sub-project 6 resizes it), `net` 4. */
export function defaultPools(cores: number): Pools {
  return makePools({ cpu: Math.max(1, Math.floor(cores) - 1), gpu: 1, hwenc: 2, net: 4 });
}
