// Key collisions inside one directory (plan 5D Decision 11), recomputed each time a listing of
// the directory closes. Two rows collide when their `relKey`s are equal; both keep their rows
// (uniqueness is on raw bytes), and each gets the bits `fin_library::collide` defines, by the
// first key step after which the two names are equal:
// - `LOSSY` (4): their decodings are equal already (two invalid names that both became
//   U+FFFD, or an invalid name and a valid one holding U+FFFD);
// - `NFC` (1): their decodings differ, their NFC forms are equal (NFC/NFD twins);
// - `CASE` (2): their NFC forms differ, only the folded keys are equal (case-insensitive root).
// Files and subdirectories share a directory's namespace, so both are compared together.
import type { DatabaseSync, StatementSync } from "node:sqlite";

export const COLLISION_NFC = 1;
export const COLLISION_CASE = 2;
export const COLLISION_LOSSY = 4;

export interface Named {
  /** The raw name bytes. */
  readonly name: Uint8Array;
  readonly nameNfc: string;
  readonly relKey: string;
}

const decoder = new TextDecoder("utf-8");

/**
 * The bits of each name, in input order (0 for a name that meets no other). `decode` is
 * UTF-8 with U+FFFD for each maximal invalid subsequence, as Rust's `from_utf8_lossy`.
 */
export function collisionBits(names: ReadonlyArray<Named>): number[] {
  const groups = new Map<string, number[]>();
  names.forEach((n, i) => {
    const g = groups.get(n.relKey);
    if (g === undefined) groups.set(n.relKey, [i]);
    else g.push(i);
  });
  const bits = names.map(() => 0);
  for (const members of groups.values()) {
    if (members.length < 2) continue;
    const decoded = members.map((i) => decoder.decode(names[i]!.name));
    members.forEach((i, a) => {
      let b = 0;
      members.forEach((j, c) => {
        if (c === a) return;
        if (decoded[a] === decoded[c]) b |= COLLISION_LOSSY;
        else if (names[i]!.nameNfc === names[j]!.nameNfc) b |= COLLISION_NFC;
        else b |= COLLISION_CASE;
      });
      bits[i] = b;
    });
  }
  return bits;
}

export interface CollisionStatements {
  readonly keys: StatementSync;
  readonly files: StatementSync;
  readonly dirs: StatementSync;
  readonly flagged: StatementSync;
  readonly clearFiles: StatementSync;
  readonly clearDirs: StatementSync;
  readonly setFile: StatementSync;
  readonly setDir: StatementSync;
}

export function prepareCollisions(db: DatabaseSync): CollisionStatements {
  const live = "state != 'missing'";
  return {
    keys: db.prepare(
      `SELECT relKey FROM (SELECT relKey FROM files WHERE dirId = ? AND ${live} UNION ALL ` +
        `SELECT relKey FROM dirs WHERE parent = ? AND ${live}) GROUP BY relKey HAVING count(*) > 1`,
    ),
    files: db.prepare(`SELECT id, name, nameNfc, relKey FROM files WHERE dirId = ? AND ${live} AND relKey = ?`),
    dirs: db.prepare(`SELECT id, name, nameNfc, relKey FROM dirs WHERE parent = ? AND ${live} AND relKey = ?`),
    flagged: db.prepare(
      "SELECT EXISTS (SELECT 1 FROM files WHERE dirId = ? AND collision != 0) OR EXISTS (SELECT 1 FROM dirs WHERE parent = ? AND collision != 0) AS x",
    ),
    clearFiles: db.prepare("UPDATE files SET collision = 0 WHERE dirId = ? AND collision != 0"),
    clearDirs: db.prepare("UPDATE dirs SET collision = 0 WHERE parent = ? AND collision != 0"),
    setFile: db.prepare("UPDATE files SET collision = ? WHERE id = ?"),
    setDir: db.prepare("UPDATE dirs SET collision = ? WHERE id = ?"),
  };
}

type Row = Named & { readonly id: number; readonly table: "files" | "dirs" };

/** Recomputes the bits of every row directly in `dirId`; answers how many rows collide. */
export function recomputeCollisions(st: CollisionStatements, dirId: number): number {
  // Most directories hold no flagged row; an UPDATE right after their inserts costs ~100 µs.
  if ((st.flagged.get(dirId, dirId) as { x: number }).x === 1) {
    st.clearFiles.run(dirId);
    st.clearDirs.run(dirId);
  }
  const keys = st.keys.all(dirId, dirId) as Array<{ relKey: string }>;
  let n = 0;
  for (const { relKey } of keys) {
    const rows: Row[] = [
      ...(st.files.all(dirId, relKey) as Array<Omit<Row, "table">>).map((r) => ({ ...r, table: "files" as const })),
      ...(st.dirs.all(dirId, relKey) as Array<Omit<Row, "table">>).map((r) => ({ ...r, table: "dirs" as const })),
    ];
    collisionBits(rows).forEach((bits, i) => {
      const r = rows[i]!;
      (r.table === "files" ? st.setFile : st.setDir).run(bits, r.id);
      if (bits !== 0) n++;
    });
  }
  return n;
}
