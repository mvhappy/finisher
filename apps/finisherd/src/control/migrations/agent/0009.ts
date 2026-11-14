// agent 0009 (7B Task 10, plan 7B Decision 15; spec §6.4, §9.4). Frozen once merged: never
// edit, add 0010. Both columns are additive, so `minReaderVersion` stays 8.
//
// - `project_trust.promptsHash`: the combined hash of a trusted project's `prompts/*.json`
//   (`none` when it had none), NULL when a person trusted the project's rules only (its prompt
//   files then stay untrusted). Kept beside `rulesHash`, not folded into it: remembering a rule
//   rewrites `finisher.rules.json` and refreshes `rulesHash` alone.
// - `sessions.trigger`: how a session came to be when it was not a person's prompt or a parent's
//   spawn: `savedPrompt` (the journal's `agent.session` trigger). NULL for every other session.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const agent0009: Migration = {
  n: 9,
  up: (db: DatabaseSync) => {
    db.exec(`
      ALTER TABLE project_trust ADD COLUMN promptsHash TEXT;
      ALTER TABLE sessions ADD COLUMN trigger TEXT;
    `);
  },
};
