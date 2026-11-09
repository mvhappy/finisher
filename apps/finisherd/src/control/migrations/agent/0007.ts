// agent 0007 (7B Task 7, plan 7B Decision 22; spec §13.1). Frozen once merged: never edit, add
// 0008. `budget_carves`: one row per child and carved kind (`tokens`, `money`): its parent, the
// amount reserved on the parent (raised with the child's limit), and whether it was settled
// (the parent charged what the child spent and the reserve returned), so a settlement happens
// once (`agent/carve.ts`). A server before this step would return every carve at its recovery
// and never charge a child's spend to its parent (a budget failing open), so the chain's
// `minReaderVersion` is 7 from this step.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0007: Migration = {
  n: 7,
  up: (db: DatabaseSync) => {
    db.exec(`
      CREATE TABLE budget_carves (
        sessionId TEXT NOT NULL, kind TEXT NOT NULL, parentId TEXT NOT NULL, amount INTEGER NOT NULL,
        settled INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (sessionId, kind)) WITHOUT ROWID;
      CREATE INDEX budget_carves_open ON budget_carves (parentId, kind) WHERE settled = 0;
    `);
  },
};
