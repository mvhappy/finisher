// A catalog extension for the 5D Task 7a tests, loaded by URL in both catalog threads like
// 8C's and 8E's. Node built-ins only: the threads import it outside vitest.
import type { DatabaseSync } from "node:sqlite";
import { threadId } from "node:worker_threads";

export const migrations = [
  { n: 1, up: (db: DatabaseSync) => db.exec("CREATE TABLE notes (id INTEGER PRIMARY KEY, text TEXT NOT NULL)") },
];

const text = (args: unknown): string => String((args as { text?: unknown } | null)?.text ?? "");

export const ops = {
  add: (db: DatabaseSync, args: unknown) => db.prepare("INSERT INTO notes (text) VALUES (?) RETURNING id").get(text(args)),
  addThenFail: (db: DatabaseSync, args: unknown) => {
    db.prepare("INSERT INTO notes (text) VALUES (?)").run(text(args));
    throw new Error("the op failed after writing");
  },
  inTransaction: (db: DatabaseSync) => ({ inTransaction: db.isTransaction }),
  whoami: () => ({ threadId }),
  /** Holds the write transaction for `ms`, writing all along. */
  spin: (db: DatabaseSync, args: unknown) => {
    const ms = Number((args as { ms?: unknown }).ms ?? 100);
    const insert = db.prepare("INSERT INTO notes (text) VALUES ('spin')");
    const until = Date.now() + ms;
    let n = 0;
    while (Date.now() < until) {
      insert.run();
      n++;
    }
    return { n };
  },
  returnsFunction: () => ({ f: () => 1 }),
};

export const queries = {
  list: (db: DatabaseSync) => db.prepare("SELECT id, text FROM notes ORDER BY id").all(),
  whoami: () => ({ threadId }),
  tryWrite: (db: DatabaseSync) => db.prepare("INSERT INTO notes (text) VALUES ('from a query')").run(),
};
