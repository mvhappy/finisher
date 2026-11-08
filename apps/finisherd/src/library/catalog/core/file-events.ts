// Which file rows a scan batch changed (plan 5D Task 11: `Libraries.onFile` events). The diff
// notes ids as it writes (`DiffConn.events`, set for one batch file); once every transaction
// of the batch committed, `settleEvents` reads the rows' final states so each id is reported
// once, as what it ended up as: a pending rename that resolved is `renamed` (not `missing`),
// a new row merged into a missing one is gone (its old row is `renamed`), and so on.
import type { DatabaseSync } from "node:sqlite";

/** Ids noted per batch file; past this the rest are dropped and `truncated` is set. */
export const MAX_EVENT_IDS = 100_000;

export interface FileIdSink {
  readonly added: Set<number>;
  readonly changed: Set<number>;
  readonly missing: Set<number>;
  readonly renamed: Set<number>;
  noted: number;
  truncated: boolean;
}

/** The rows one batch file changed, by what each became. */
export interface FileChanges {
  readonly added: number[];
  readonly changed: number[];
  readonly missing: number[];
  readonly renamed: number[];
  /** More than `MAX_EVENT_IDS` rows changed: the lists are incomplete. */
  readonly truncated?: boolean;
}

export const newSink = (): FileIdSink => ({ added: new Set(), changed: new Set(), missing: new Set(), renamed: new Set(), noted: 0, truncated: false });

/** Notes `id` under `kind` (no-op without a sink). */
export function noteFile(sink: FileIdSink | undefined, kind: "added" | "changed" | "missing" | "renamed", id: number): void {
  if (sink === undefined) return;
  if (sink.noted >= MAX_EVENT_IDS) {
    sink.truncated = true;
    return;
  }
  sink.noted++;
  sink[kind].add(id);
}

const CHUNK = 900;

/** The final lists of `sink`: every id once, by the row's state now. */
export function settleEvents(db: DatabaseSync, sink: FileIdSink): FileChanges {
  const all = [...new Set([...sink.added, ...sink.changed, ...sink.missing, ...sink.renamed])];
  const state = new Map<number, string>();
  const st = db.prepare("SELECT id, state FROM files WHERE id IN (SELECT value FROM json_each(?))");
  for (let i = 0; i < all.length; i += CHUNK) {
    for (const r of st.all(JSON.stringify(all.slice(i, i + CHUNK))) as Array<{ id: number; state: string }>) state.set(r.id, r.state);
  }
  const live = (id: number): boolean => state.has(id) && state.get(id) !== "missing";
  const renamed = [...sink.renamed].filter(live);
  const taken = new Set(renamed);
  const added = [...sink.added].filter((id) => live(id) && !taken.has(id));
  for (const id of added) taken.add(id);
  const missing = [...sink.missing].filter((id) => state.get(id) === "missing" && !taken.has(id));
  for (const id of missing) taken.add(id);
  const changed = [...sink.changed].filter((id) => state.has(id) && !taken.has(id));
  const sort = (a: number[]) => a.sort((x, y) => x - y);
  return { added: sort(added), changed: sort(changed), missing: sort(missing), renamed: sort(renamed), ...(sink.truncated ? { truncated: true } : {}) };
}
