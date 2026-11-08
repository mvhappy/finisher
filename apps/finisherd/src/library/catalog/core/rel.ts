// Raw directory chains (plan 5D Decision 10): a file is opened by its directory's raw names
// from the root plus its own, never by `relKey`. Used by both catalog threads.
import type { DatabaseSync, StatementSync } from "node:sqlite";

const CHAIN_SQL =
  "WITH RECURSIVE up (id, parent, name, depth) AS (" +
  "SELECT id, parent, name, 0 FROM dirs WHERE id = ? " +
  "UNION ALL SELECT d.id, d.parent, d.name, up.depth + 1 FROM dirs d JOIN up ON d.id = up.parent) " +
  "SELECT name FROM up WHERE parent IS NOT NULL ORDER BY depth DESC";

const b64 = (u: Uint8Array): string => Buffer.from(u.buffer, u.byteOffset, u.byteLength).toString("base64");

/** Answers base64 raw chains of directories, statement prepared once per call site. */
export class DirChains {
  readonly #stmt: StatementSync;
  readonly #cache = new Map<number, string[]>();
  constructor(db: DatabaseSync) {
    this.#stmt = db.prepare(CHAIN_SQL);
  }

  /** The directory's components below the root (empty for the root). */
  of(dirId: number): string[] {
    const hit = this.#cache.get(dirId);
    if (hit !== undefined) return hit;
    const chain = (this.#stmt.all(dirId) as Array<{ name: Uint8Array }>).map((r) => b64(r.name));
    this.#cache.set(dirId, chain);
    return chain;
  }

  /** A file's `RawRel`: its directory's chain plus its own raw name. */
  file(dirId: number, name: Uint8Array): { components: string[] } {
    return { components: [...this.of(dirId), b64(name)] };
  }
}

export { b64 as base64 };
