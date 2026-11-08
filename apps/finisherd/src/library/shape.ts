// What the library RPCs show (plan 5D Task 13): catalog rows as the contracts' files and
// directories, views with their per-directory warnings, cursors, and jobs without their
// params (a scan's params hold raw directory chains and a generation, nothing a client needs).
import type { JobView } from "@finisher/contracts";
import { FinError } from "@finisher/contracts";
import type { LibraryFileResult, LibraryViewResult } from "@finisher/contracts";
import type { LibraryView } from "./api.ts";
import type { DirRow, FileRow } from "./catalog/core/queries.ts";
import type { LibraryService } from "./service.ts";
import { exec } from "./states.ts";

/** A job as a client sees it: no params, no result. */
export function publicJob(job: JobView): JobView {
  const { params: _params, result: _result, ...rest } = job;
  return { ...rest, params: {} };
}

/** `view` with the directories the catalog flags (`ROOT_UNMOUNTED {dir}`, `DIR_EMPTY_SUSPECT {dir}`) in its warnings. */
export async function withDirWarnings(svc: LibraryService, view: LibraryView): Promise<LibraryViewResult> {
  try {
    const r = (await exec(svc.deps.catalogs.reader.query(view.id, "core", "dirWarnings", {}))) as { warnings: Array<{ code: string; dir: string }> };
    return (r.warnings.length === 0 ? view : { ...view, warnings: [...view.warnings, ...r.warnings] }) as LibraryViewResult;
  } catch {
    return view as LibraryViewResult; // a catalog that is not open has no directories to flag
  }
}

const join = (dir: string, name: string): string => (dir === "" ? name : `${dir}/${name}`);

export function fileShape(f: FileRow, display: string): LibraryFileResult {
  return {
    id: f.id,
    dirId: f.dirId,
    name: f.nameNfc,
    display,
    ext: f.ext,
    kind: f.kind,
    size: f.size,
    mtimeNs: f.mtimeNs,
    state: f.state as LibraryFileResult["state"],
    code: f.code,
    placeholder: f.placeholder,
    fp: f.fp,
    fullHash: f.fullHash,
    probe: f.probe ?? null,
    probeVersion: f.probeVersion,
    durationMs: f.durationMs,
    width: f.width,
    height: f.height,
    collision: f.collision !== 0,
    hardlink: f.hardlink,
    firstSeen: f.firstSeen,
    lastSeen: f.lastSeen,
    lastChange: f.lastChange,
    missingSince: f.missingSince,
  };
}

/** A directory of a listing; `parent` is the listing's own display path (`""` for the root). */
export function dirShape(d: DirRow, display: string): { id: number; name: string; display: string; state: string; code: string | null; placeholder: string | null; collision: boolean } {
  return { id: d.id, name: d.nameNfc, display, state: d.state, code: d.code, placeholder: d.placeholder, collision: d.collision !== 0 };
}

export const childDisplay = join;

const invalidCursor = (): FinError => new FinError({ code: "CURSOR_INVALID", message: "the cursor is not one this server gave", details: {} });

/** `library.files`' cursor: the keyset `(nameNfc, id)` as opaque text. */
export function encodeCursor(after: { nameNfc: string; id: number }): string {
  return `f1.${Buffer.from(JSON.stringify([after.nameNfc, after.id])).toString("base64url")}`;
}

export function decodeCursor(cursor: string): { nameNfc: string; id: number } {
  if (!cursor.startsWith("f1.")) throw invalidCursor();
  try {
    const v = JSON.parse(Buffer.from(cursor.slice(3), "base64url").toString("utf8")) as unknown;
    if (Array.isArray(v) && v.length === 2 && typeof v[0] === "string" && Number.isSafeInteger(v[1])) return { nameNfc: v[0], id: v[1] as number };
  } catch {
    // fall through
  }
  throw invalidCursor();
}

/** `stageCounts` rows as one row per stage and version. */
export function foldStages(rows: ReadonlyArray<{ stage: string; version: string; state: string; n: number }>) {
  const out = new Map<string, { stage: string; version: string; done: number; queued: number; failed: number; stale: number; skipped: number }>();
  for (const r of rows) {
    const key = `${r.stage}\u0000${r.version}`;
    let o = out.get(key);
    if (o === undefined) out.set(key, (o = { stage: r.stage, version: r.version, done: 0, queued: 0, failed: 0, stale: 0, skipped: 0 }));
    if (r.state === "done") o.done += r.n;
    else if (r.state === "queued" || r.state === "running") o.queued += r.n;
    else if (r.state === "failed") o.failed += r.n;
    else if (r.state === "stale") o.stale += r.n;
    else if (r.state === "skipped") o.skipped += r.n;
  }
  return [...out.values()];
}
