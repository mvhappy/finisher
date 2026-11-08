// agent 0005 (7B Task 5, plan 7B Decision 10; spec §7.3). Frozen once merged: never edit, add
// 0006. `approvals.grantIdem`: the idempotency key of the call that consumed an `allow` answer
// (its one-time grant), so a handler that fails without committing restores exactly that
// call's grant. A server before this step ignores the column and consumes no grants, so the
// chain's `minReaderVersion` stays 4.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0005: Migration = {
  n: 5,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE approvals ADD COLUMN grantIdem TEXT;
      CREATE INDEX approvals_grants ON approvals (sessionId, argsHash) WHERE status = 'answered' AND decision = 'allow';
    `);
  },
};
