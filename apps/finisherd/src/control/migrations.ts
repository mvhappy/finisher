// `control.db` migrations in independent per-area chains (plan 5A Decision 2, spec §3.1).
//
// Every area of the control plane (`core`, `auth`, `jobs`, `libraries`, `reviews`, `devices`)
// owns one chain: a frozen, ordered list `control/migrations/<chain>/0001.ts, 0002.ts, …`.
// Chains are independent: each has its own `formatVersion.<chain>` and
// `minReaderVersion.<chain>` in `meta`, its applied steps are rows `migrations(chain, n)`, and
// no table of one chain has a foreign key into another's. So batches built in parallel (5A,
// 5C-1) never fight over migration numbers.
//
// A chain registers itself with `registerChain(name, migrations)` (a module side effect,
// before `openControlStore`), and `openControlStore` runs `core` first, then the other chains
// in name order. A migration, once merged, is frozen: never edit it, add the next one.
import type { DatabaseSync } from "node:sqlite";

/** One step of a chain. `n` counts from 1 within the chain. `up` runs inside the open transaction. */
export interface Migration {
  readonly n: number;
  readonly up: (db: DatabaseSync) => void;
}

/** A registered chain: its steps and the versions this code writes and can read. */
export interface Chain {
  readonly name: string;
  readonly migrations: ReadonlyArray<Migration>;
  /** The chain's format this code writes: by default its last step's `n`. */
  readonly formatVersion: number;
  /**
   * The oldest reader of this chain's tables that can still use them, written to `meta` as
   * `minReaderVersion.<chain>`. A database whose value is above this code's `formatVersion`
   * refuses the start (`CONTROL_DB_TOO_NEW`). Default 1: every step so far is additive.
   */
  readonly minReaderVersion: number;
}

export interface ChainVersions {
  readonly formatVersion?: number;
  readonly minReaderVersion?: number;
}

const NAME = /^[a-z][a-z0-9]*$/;

/** A set of chains. The process-wide one is `CONTROL_CHAINS`; tests make their own. */
export class ChainRegistry {
  readonly #chains = new Map<string, Chain>();

  /**
   * Registers `name` with its frozen steps. Registering the same steps again (a module
   * loaded twice) is a no-op; different steps under one name are a bug and throw.
   */
  register(name: string, migrations: ReadonlyArray<Migration>, versions: ChainVersions = {}): Chain {
    if (!NAME.test(name)) throw new Error(`control chain name ${JSON.stringify(name)} must match ${NAME}`);
    migrations.forEach((m, i) => {
      if (m.n !== i + 1) throw new Error(`control chain ${name}: step ${i + 1} is numbered ${m.n}`);
    });
    const last = migrations.length;
    const chain: Chain = {
      name,
      migrations,
      formatVersion: versions.formatVersion ?? last,
      minReaderVersion: versions.minReaderVersion ?? 1,
    };
    if (chain.formatVersion < last) throw new Error(`control chain ${name}: formatVersion below its last step`);
    if (chain.minReaderVersion > chain.formatVersion) throw new Error(`control chain ${name}: minReaderVersion above formatVersion`);
    const known = this.#chains.get(name);
    if (known !== undefined) {
      const same =
        known.formatVersion === chain.formatVersion &&
        known.minReaderVersion === chain.minReaderVersion &&
        known.migrations.length === last &&
        known.migrations.every((m, i) => m.up === migrations[i]?.up);
      if (!same) throw new Error(`control chain ${name} is already registered with other steps`);
      return known;
    }
    this.#chains.set(name, chain);
    return chain;
  }

  /** `core` first (it creates `meta` and `migrations`), then the others by name. */
  ordered(): ReadonlyArray<Chain> {
    const all = [...this.#chains.values()];
    return [...all.filter((c) => c.name === "core"), ...all.filter((c) => c.name !== "core").sort((a, b) => (a.name < b.name ? -1 : 1))];
  }

  get(name: string): Chain | undefined {
    return this.#chains.get(name);
  }
}

/** The process-wide chains `openControlStore` runs unless given another registry. */
export const CONTROL_CHAINS = new ChainRegistry();

/** Registers a chain with the process-wide registry (see `ChainRegistry.register`). */
export function registerChain(name: string, migrations: ReadonlyArray<Migration>, versions?: ChainVersions): Chain {
  return CONTROL_CHAINS.register(name, migrations, versions);
}

/** What `planMigrations` found. */
export interface MigrationPlan {
  /** Chains whose `minReaderVersion` in the database is above what this code reads. */
  readonly tooNew: ReadonlyArray<{ chain: string; minReaderVersion: number; readerVersion: number }>;
  readonly pending: ReadonlyArray<{ chain: Chain; steps: ReadonlyArray<Migration> }>;
  /** True when any migration had been applied before (the database is not new). */
  readonly existing: boolean;
}

function tableExists(db: DatabaseSync, name: string): boolean {
  return db.prepare("SELECT 1 AS x FROM sqlite_master WHERE type = 'table' AND name = ?").get(name) !== undefined;
}

/** Reads what each chain has applied and what it requires of its readers. */
export function planMigrations(db: DatabaseSync, registry: ChainRegistry): MigrationPlan {
  const hasMeta = tableExists(db, "meta");
  const hasMigrations = tableExists(db, "migrations");
  const meta = (key: string): string | undefined => {
    if (!hasMeta) return undefined;
    const row = db.prepare("SELECT value FROM meta WHERE key = ?").get(key) as { value: string } | undefined;
    return row?.value;
  };
  const applied = (chain: string): number => {
    if (!hasMigrations) return 0;
    const row = db.prepare("SELECT MAX(n) AS n FROM migrations WHERE chain = ?").get(chain) as { n: number | null };
    return row.n ?? 0;
  };
  const tooNew: Array<{ chain: string; minReaderVersion: number; readerVersion: number }> = [];
  const pending: Array<{ chain: Chain; steps: ReadonlyArray<Migration> }> = [];
  let existing = false;
  for (const chain of registry.ordered()) {
    const minReader = Number(meta(`minReaderVersion.${chain.name}`) ?? 0);
    if (minReader > chain.formatVersion) {
      tooNew.push({ chain: chain.name, minReaderVersion: minReader, readerVersion: chain.formatVersion });
    }
    const done = applied(chain.name);
    if (done > 0) existing = true;
    const steps = chain.migrations.filter((m) => m.n > done);
    if (steps.length > 0) pending.push({ chain, steps });
  }
  return { tooNew, pending, existing };
}

/**
 * Applies `plan.pending` in one transaction the caller holds: each step, its `migrations`
 * row, and the chain's `formatVersion`/`minReaderVersion` in `meta` (never lowered: a newer
 * code may have written a higher format its readers can still use).
 */
export function applyMigrations(db: DatabaseSync, plan: MigrationPlan, now: number): void {
  for (const { chain, steps } of plan.pending) {
    for (const step of steps) {
      step.up(db);
      db.prepare("INSERT INTO migrations (chain, n, appliedAt) VALUES (?, ?, ?)").run(chain.name, step.n, now);
    }
    const upsert = db.prepare(
      "INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = " +
        "CASE WHEN CAST(value AS INTEGER) < CAST(excluded.value AS INTEGER) THEN excluded.value ELSE value END",
    );
    upsert.run(`formatVersion.${chain.name}`, String(chain.formatVersion));
    upsert.run(`minReaderVersion.${chain.name}`, String(chain.minReaderVersion));
  }
}
