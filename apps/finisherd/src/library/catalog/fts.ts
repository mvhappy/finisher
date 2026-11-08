// The `names` FTS5 index (plan 5D Decisions 23, 43): a contentless table (`content = ''`,
// `contentless_delete = 1`), rowid = `files.id`, over two columns:
// - `name`: the file's NFC display name;
// - `dir`: its directory's `relKey` (the NFC path, case-folded on case-insensitive roots;
//   `unicode61` folds case and, with `remove_diacritics 2`, diacritics anyway).
// Task 7a measured the whole index at ~126 bytes per file on the 100k tree
// (`catalog-size.test.ts`), so directory tokens stay per file and no `dirnames` table exists.
//
// Every write to `files` that adds, renames or deletes a row goes through these helpers in the
// same transaction, so the index follows the table exactly.
import type { DatabaseSync, StatementSync } from "node:sqlite";

export interface FtsStatements {
  readonly insert: StatementSync;
  readonly delete: StatementSync;
  readonly reset: StatementSync;
}

/**
 * A deletion from a `contentless_delete` table writes tombstone pages whose rowids lie far
 * above 2^53, and SQLite then reports one of them as the connection's last insert rowid:
 * `node:sqlite`'s `run()` of **any** statement on the connection would throw converting it.
 * So the FTS statements answer bigints, and after a deletion (and after each commit, which
 * flushes pending tombstones) `ftsSettle` brings the connection's last rowid back to 0 with
 * a one-row temp table.
 */
export function prepareFts(db: DatabaseSync): FtsStatements {
  db.exec("CREATE TEMP TABLE IF NOT EXISTS rowid_reset (x INTEGER PRIMARY KEY)");
  const st = {
    insert: db.prepare("INSERT INTO names (rowid, name, dir) VALUES (?, ?, ?)"),
    delete: db.prepare("DELETE FROM names WHERE rowid = ?"),
    reset: db.prepare("INSERT OR REPLACE INTO temp.rowid_reset (x) VALUES (0)"),
  };
  for (const s of Object.values(st)) s.setReadBigInts(true);
  return st;
}

export function ftsSettle(st: FtsStatements): void {
  st.reset.run();
}

export function ftsAdd(st: FtsStatements, fileId: number, nameNfc: string, dirKey: string): void {
  st.insert.run(fileId, nameNfc, dirKey);
}

export function ftsRemove(st: FtsStatements, fileId: number): void {
  st.delete.run(fileId);
  st.reset.run();
}

/** A rename or a move: the row's tokens are replaced. */
export function ftsReplace(st: FtsStatements, fileId: number, nameNfc: string, dirKey: string): void {
  st.delete.run(fileId);
  st.insert.run(fileId, nameNfc, dirKey);
}

/**
 * The terms of a query as `unicode61` splits text: runs of letters, numbers, private-use
 * characters and marks (marks stay inside a term, as they do for the tokenizer before it
 * removes diacritics). The query is taken in NFC first, as the indexed names are, so a name
 * typed or pasted decomposed (macOS) finds its composed twin. A run of marks alone (a stray
 * combining accent) tokenises to nothing and is dropped.
 */
export function queryTerms(query: string): string[] {
  const runs = query.normalize("NFC").match(/[\p{L}\p{N}\p{M}\p{Co}]+/gu) ?? [];
  return runs.filter((t) => /[\p{L}\p{N}\p{Co}]/u.test(t));
}

/**
 * An FTS5 `MATCH` expression of prefix terms (`"interv"* "take"*`, all required, in either
 * column), or `undefined` for a query without terms. Terms hold only letters, numbers and
 * marks, so no FTS syntax from the query survives quoting (`AND`, `NEAR`, `col:`, `-`, `^`,
 * `*` and `"` are separators or literal words).
 */
export function matchExpression(query: string): string | undefined {
  const terms = queryTerms(query);
  if (terms.length === 0) return undefined;
  return terms.map((t) => `"${t}"*`).join(" ");
}
