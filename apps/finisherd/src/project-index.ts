// The `app.db` project index of the ProjectRegistry (`projects.ts`): where each project lives
// and when it was last opened.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";

/** `<server-data>/app.db`: `projects(id, dir, last_opened)`. */
export class ProjectIndex {
  readonly #db: DatabaseSync;

  constructor(serverData: string) {
    mkdirSync(serverData, { recursive: true });
    this.#db = new DatabaseSync(join(serverData, "app.db"));
    this.#db.exec(
      "CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, dir TEXT NOT NULL, last_opened INTEGER NOT NULL)",
    );
  }

  touch(id: string, dir: string): void {
    this.#db
      .prepare(
        "INSERT INTO projects (id, dir, last_opened) VALUES (?, ?, ?) " +
          "ON CONFLICT(id) DO UPDATE SET dir = excluded.dir, last_opened = excluded.last_opened",
      )
      .run(id, dir, Date.now());
  }

  dirOf(id: string): string | undefined {
    const row = this.#db.prepare("SELECT dir FROM projects WHERE id = ?").get(id) as { dir?: string } | undefined;
    return row?.dir;
  }

  close(): void {
    this.#db.close();
  }
}
