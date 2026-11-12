// plugins 0005 (sp8b, authority keys; sp8 spec §3.3, Decision 28): the policy generation of each
// principal class (`person`, `system`, `agent:<root session>`, `<kind>:<id>`). A class's
// generation increments when one of its credentials is minted, narrowed or ended (an agent
// session tree that starts, ends or gains a child) or a device is revoked (`person`); plugin
// processes, temp directories and authority-keyed `pluginData` of an older generation are
// retired. A missing row is generation 0.
// Frozen once merged: never edit.
import type { DatabaseSync } from "node:sqlite";
import type { Migration } from "../../migrations.ts";

export const plugins0005: Migration = {
  n: 5,
  up: (db: DatabaseSync) => {
    db.exec(`CREATE TABLE plugin_authority (class TEXT PRIMARY KEY, generation INTEGER NOT NULL CHECK (generation >= 0));`);
  },
};
