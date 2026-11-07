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
}

export function prepareFts(db: DatabaseSync): FtsStatements {
  return {
    insert: db.prepare("INSERT INTO names (rowid, name, dir) VALUES (?, ?, ?)"),
    delete: db.prepare("DELETE FROM names WHERE rowid = ?"),
  };
}

export function ftsAdd(st: FtsStatements, fileId: number, nameNfc: string, dirKey: string): void {
  st.insert.run(fileId, nameNfc, dirKey);
}

export function ftsRemove(st: FtsStatements, fileId: number): void {
  st.delete.run(fileId);
}

/** A rename or a move: the row's tokens are replaced. */
export function ftsReplace(st: FtsStatements, fileId: number, nameNfc: string, dirKey: string): void {
  st.delete.run(fileId);
  st.insert.run(fileId, nameNfc, dirKey);
}

/**
 * The terms of a query as `unicode61` splits text: runs of letters, numbers and marks (marks
 * stay inside a term, as they do for the tokenizer before it removes diacritics).
 */
export function queryTerms(query: string): string[] {
  return query.normalize("NFC").match(/[\p{L}\p{N}\p{M}\p{Co}]+/gu) ?? [];
}

/**
 * An FTS5 `MATCH` expression of prefix terms (`"interv"* "take"*`, all required), or
 * `undefined` for a query without terms. Terms hold only letters, numbers and marks, so no
 * FTS syntax from the query survives quoting.
 */
export function matchExpression(query: string): string | undefined {
  const terms = queryTerms(query);
  if (terms.length === 0) return undefined;
  return terms.map((t) => `"${t}"*`).join(" ");
}
