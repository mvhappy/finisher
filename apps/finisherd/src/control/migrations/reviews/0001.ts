// reviews 0001 (5E Task 5, plan Decision 8, spec §8.1): the review workflow's tables.
// Frozen once merged: never edit. No foreign key leaves the chain, and none is declared inside it
// (the store deletes an item's rows together).
//
// - `reviews`: one row per item. `source` and `target` are branch ids; `snapshot` is the JSON
//   `{sourceSeq, sourceHash?, targetSeq, targetHash?, plannedAt}` its change list was planned at
//   (the list itself is derived, never stored); `requestedBy` is `{principal, actor?}`; `subject`
//   is a `mergeConflict`'s conflict list or a `pluginSuggestion`'s `{pluginId}`. A partial unique
//   index keeps one open `branchReview` and one open `mergeConflict` per `(projectId, source,
//   target)` (Decision 10).
// - `review_decisions`: keyed by `(itemId, txId)` of the origin transaction, so a decision
//   survives a refresh of the snapshot (spec §8.2). Only `accept` and `reject` are stored;
//   `pending` deletes the row.
// - `review_events`, `review_comments`: the item's history and its comments (review text: never
//   audited, never journalled).
// - `branch_policy`: a project's protected branches (ids), `requireReview` and the auto-merge
//   switch, in this server's `control.db` only.
// - `review_verdicts`: reserved for sub-project 10's QA gate and critic verdicts for a snapshot;
//   5E writes it only in tests.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const reviews0001: Migration = {
  n: 1,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE reviews (
        id TEXT PRIMARY KEY, projectId TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('branchReview', 'mergeConflict', 'pluginSuggestion')),
        state TEXT NOT NULL CHECK (state IN ('open', 'changesRequested', 'merged', 'rejected', 'dismissed', 'superseded')),
        source TEXT, target TEXT, snapshot TEXT, title TEXT NOT NULL, summary TEXT,
        requestedBy TEXT NOT NULL, createdAt INTEGER NOT NULL, updatedAt INTEGER NOT NULL,
        closedAt INTEGER, closedBy TEXT, mergeId TEXT, rederived INTEGER NOT NULL DEFAULT 0, subject TEXT) WITHOUT ROWID;
      CREATE INDEX reviews_state ON reviews (projectId, state);
      CREATE INDEX reviews_pair ON reviews (projectId, source, target);
      CREATE UNIQUE INDEX reviews_open_pair ON reviews (projectId, kind, source, target)
        WHERE kind IN ('branchReview', 'mergeConflict') AND state IN ('open', 'changesRequested');
      CREATE TABLE review_decisions (
        itemId TEXT NOT NULL, txId TEXT NOT NULL, decision TEXT NOT NULL CHECK (decision IN ('accept', 'reject')),
        decidedBy TEXT NOT NULL, reason TEXT, at INTEGER NOT NULL, PRIMARY KEY (itemId, txId)) WITHOUT ROWID;
      CREATE TABLE review_events (
        id INTEGER PRIMARY KEY AUTOINCREMENT, itemId TEXT NOT NULL, at INTEGER NOT NULL, kind TEXT NOT NULL,
        by TEXT NOT NULL, details TEXT NOT NULL DEFAULT '{}');
      CREATE INDEX review_events_item ON review_events (itemId, id);
      CREATE TABLE review_comments (
        id TEXT PRIMARY KEY, itemId TEXT NOT NULL, principal TEXT NOT NULL, actor TEXT, body TEXT NOT NULL,
        at TEXT, txId TEXT, createdAt INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX review_comments_item ON review_comments (itemId, createdAt);
      CREATE TABLE branch_policy (
        projectId TEXT PRIMARY KEY, protected TEXT NOT NULL, requireReview INTEGER NOT NULL DEFAULT 0,
        autoMerge TEXT NOT NULL) WITHOUT ROWID;
      CREATE TABLE review_verdicts (
        verdictId TEXT PRIMARY KEY, itemId TEXT NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('qa', 'critic')),
        snapshot TEXT NOT NULL, pass INTEGER NOT NULL, at INTEGER NOT NULL) WITHOUT ROWID;
      CREATE INDEX review_verdicts_item ON review_verdicts (itemId, kind);
    `);
  },
};
