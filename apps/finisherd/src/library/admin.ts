// The library service's owner actions (plan 5D Decisions 17, 42, 47, 48): add, remove, set,
// accept a changed volume or removed nested mounts, and scans asked for. Scopes, human-only
// checks and challenges are the RPC layer's (Task 13); these functions only act.
import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { RawRel } from "@finisher/core-types";
import { FinError, type JobView, type Priority } from "@finisher/contracts";
import { libraryProbeVersion } from "@finisher/fin-node";
import type { LibraryView } from "./api.ts";
import { libraryDir } from "./catalog/open.ts";
import { probeParams } from "./kind-params.ts";
import type { Actor, LibraryPatch, NewLibrary } from "./registry.ts";
import { type LibraryService, SYSTEM_OWNER, exec } from "./service.ts";

const ACTIVE = ["queued", "running", "paused"] as const;
/** Files of `libraries/<id>/` that belong to sub-project 8E, never deleted here (Decision 42). */
const KEPT = new Set(["vectors"]);

const notFound = (id: string): FinError => new FinError({ code: "LIBRARY_NOT_FOUND", message: "there is no such library", details: { id } });

/** Adds a library and starts its first full scan (`trigger: add`): `{library, job}` (Decision 48). */
export async function addLibrary(svc: LibraryService, input: NewLibrary, actor: Actor): Promise<{ library: LibraryView; job?: JobView }> {
  const row = await exec(svc.registry.add(input, actor));
  await svc.attach(row, { isNew: true });
  const job = await svc.submitScan(row.id, { mode: "full", trigger: "add" });
  svc.usage.touch(row.id);
  return { library: svc.view(svc.registry.get(row.id) ?? row), ...(job === undefined ? {} : { job }) };
}

/**
 * Removes a library (Decision 42): cancels its jobs, stops its watch, closes and deletes its
 * catalog files and 5D's other files in `libraries/<id>/`, keeps `vectors/` (8E owns it), then
 * deletes the row (audited).
 */
export async function removeLibrary(svc: LibraryService, serverData: string, id: string, actor: Actor): Promise<void> {
  const row = svc.registry.get(id);
  if (row === undefined) throw notFound(id);
  const { engine, catalogs } = svc.deps;
  for (const job of engine.list({ library: id, state: [...ACTIVE], limit: 500 }).jobs) {
    await exec(engine.cancel(job.id)).catch(() => undefined);
  }
  svc.forget(id);
  await exec(catalogs.indexer.unwatch(id)).catch(() => undefined);
  await exec(catalogs.indexer.close(id)).catch(() => undefined);
  const dir = libraryDir(serverData, id);
  let names: string[] = [];
  try {
    names = readdirSync(dir);
  } catch {
    // No catalog directory yet.
  }
  for (const name of names) if (!KEPT.has(name)) rmSync(join(dir, name), { recursive: true, force: true });
  if (names.every((n) => !KEPT.has(n))) rmSync(dir, { recursive: true, force: true });
  await exec(svc.registry.remove(id, actor));
  svc.replan();
}

/**
 * `library.set` (without `acceptVolume`): the registry's fields, then what follows from them.
 * Pausing cancels the library's scans (they cost no attempt, Decision 40); resuming plans it
 * again; new include or exclude rules start a full scan, so known rows they now exclude go.
 */
export async function setLibrary(svc: LibraryService, id: string, patch: LibraryPatch, actor: Actor): Promise<LibraryView> {
  const before = svc.registry.get(id);
  if (before === undefined) throw notFound(id);
  const row = await exec(svc.registry.set(id, patch, actor));
  if (row.state === "paused" && before.state !== "paused") {
    for (const job of svc.deps.engine.list({ kind: "library.scan", library: id, state: [...ACTIVE], limit: 500 }).jobs) {
      await exec(svc.deps.engine.cancel(job.id)).catch(() => undefined);
    }
  }
  const filters = patch.include !== undefined || patch.exclude !== undefined;
  if (filters && row.state !== "paused" && row.state !== "offline") await svc.submitScan(id, { mode: "full", trigger: "demand" });
  svc.replan();
  return svc.view(svc.registry.get(id) ?? row);
}

/**
 * The owner accepts the volume now under the root (`library.set {acceptVolume: true}`,
 * human-only, Decision 17): a changed volume, or a root that is no longer a mount root. The
 * identity is measured again and stored (audited), the pools follow its key, and the library
 * comes back with a full scan (`trigger: mount`; rows the new volume lacks go missing).
 */
export async function acceptVolume(svc: LibraryService, id: string, actor: Actor): Promise<LibraryView> {
  const row = svc.registry.get(id);
  if (row === undefined) throw notFound(id);
  const identity = await exec(svc.registry.measure(row));
  svc.registry.setVolume(id, identity, actor);
  const updated = svc.registry.get(id)!;
  svc.book.add(updated);
  if (updated.state === "offline") await svc.goOnline(id);
  else await svc.submitScan(id, { mode: "full", trigger: "demand" });
  return svc.view(svc.registry.get(id) ?? updated);
}

/**
 * The owner removed nested mounts for good (Decision 17; the 7b note): every offline mount
 * point below the root (or the ones named) forgets its mount, and a targeted scan lists them.
 * Their rows then go missing like any directory's (after the usual empty-listing
 * confirmation, Decision 25). Answers the directories listed and the scan.
 */
export async function acceptUnmounted(
  svc: LibraryService,
  id: string,
  dirs?: ReadonlyArray<RawRel>,
): Promise<{ dirs: ReadonlyArray<RawRel>; job?: JobView }> {
  if (svc.registry.get(id) === undefined) throw notFound(id);
  const r = await exec(svc.deps.catalogs.indexer.acceptUnmounted(id, dirs));
  const job = await svc.submitScan(id, { mode: "dirs", dirs: r.dirs, trigger: "demand" });
  return { dirs: r.dirs, ...(job === undefined ? {} : { job }) };
}

/**
 * `library.scan {id, full?, dirs?}`: a full scan, a targeted scan of `dirs`, or else a poll,
 * all `trigger: demand`; the library counts as in use for 10 minutes.
 */
export async function scanLibrary(svc: LibraryService, id: string, req: { readonly full?: boolean; readonly dirs?: ReadonlyArray<RawRel> }): Promise<JobView> {
  const row = svc.registry.get(id);
  if (row === undefined) throw notFound(id);
  if (row.state === "paused") throw new FinError({ code: "LIBRARY_PAUSED", message: "the library is paused", details: { id } });
  if (row.state === "offline") throw new FinError({ code: "LIBRARY_OFFLINE", message: "the library is offline", details: { id, reason: row.offlineReason ?? null } });
  svc.usage.touch(id);
  const job =
    req.full === true
      ? await svc.submitScan(id, { mode: "full", trigger: "demand" })
      : req.dirs !== undefined && req.dirs.length > 0
        ? await svc.submitScan(id, { mode: "dirs", dirs: req.dirs, trigger: "demand" })
        : await svc.submitScan(id, { mode: "poll", trigger: "demand" });
  if (job === undefined) throw new FinError({ code: "WORKER_UNAVAILABLE", message: "the scan could not be submitted", details: { id } });
  return job;
}

/**
 * A `library.probe` job of up to 256 catalogued files (for the probe stage, Task 11). A
 * metadata-only library (Decision 14) opens nothing: its files are recorded `skipped` with
 * `METADATA_ONLY` instead, and no job is submitted.
 */
export async function probeFiles(
  svc: LibraryService,
  library: string,
  fileIds: ReadonlyArray<number>,
  priority?: Priority,
): Promise<{ job?: JobView; skipped: number }> {
  const row = svc.registry.get(library);
  if (row === undefined) throw notFound(library);
  const { engine, catalogs } = svc.deps;
  const params = probeParams(row, fileIds, svc.book.readSlots(library));
  if (params !== undefined) {
    const job = await exec(engine.submit(SYSTEM_OWNER, { kind: "library.probe", params, library, ...(priority === undefined ? {} : { priority }) }));
    return { job, skipped: 0 };
  }
  const { lines } = await exec(catalogs.reader.fileLines(library, { fileIds: [...fileIds], hydrate: true }));
  const version = libraryProbeVersion();
  const rows = lines.map((l) => ({ fileId: l.id, size: l.expect.size, mtimeNs: l.expect.mtimeNs, probeVersion: version, state: "skipped" as const, code: "METADATA_ONLY" }));
  if (rows.length > 0) await exec(catalogs.indexer.applyProbed(library, rows));
  return { skipped: rows.length };
}
