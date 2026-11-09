// The library service's owner actions (plan 5D Decisions 17, 42, 47, 48): add, remove, set,
// accept a changed volume or removed nested mounts, and scans asked for. Scopes, human-only
// checks and challenges are the RPC layer's (Task 13); these functions only act.
import { readdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import type { RawRel } from "@finisher/core-types";
import { FinError, type JobView, type Priority } from "@finisher/contracts";
import type { LibraryView } from "./api.ts";
import { libraryDir } from "./catalog/open.ts";
import { submitProbe } from "./pump.ts";
import type { Actor, LibraryPatch, NewLibrary } from "./registry.ts";
import { type LibraryService, RECHECK_PROBE_MS, exec } from "./service.ts";

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
    await svc.stages.stop(id);
  }
  if (row.state !== "paused" && before.state === "paused") svc.stages.kick(id);
  if (patch.stages !== undefined || patch.kinds !== undefined || patch.hydrate !== undefined) svc.stages.kick(id);
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
 *
 * Decision 18 (never mark missing when unsure), the Tasks 9–11 review: the root is probed
 * first, and a root that is unmounted or empty is refused (`LIBRARY_ROOT_SUSPECT`) unless
 * `force`: accepting it would drop the whole library. Until a full scan succeeds, a root listing
 * that would mark more than half of its directories missing is held instead
 * (`ROOT_DROP_SUSPECT`, the library offline); accepting again with `force` confirms it.
 */
export async function acceptVolume(svc: LibraryService, id: string, actor: Actor, opts: { readonly force?: boolean } = {}): Promise<LibraryView> {
  const row = svc.registry.get(id);
  if (row === undefined) throw notFound(id);
  const force = opts.force === true;
  const probe = await exec(svc.registry.probe(row, RECHECK_PROBE_MS));
  if (!force && (probe.state === "unmounted" || probe.state === "empty")) {
    throw new FinError({
      code: "LIBRARY_ROOT_SUSPECT",
      message: "the root looks unmounted or empty: accepting it would mark the whole library missing (pass force to accept it anyway)",
      details: { id, state: probe.state },
    });
  }
  const identity = await exec(svc.registry.measure(row));
  svc.registry.setVolume(id, identity, actor);
  const updated = svc.registry.get(id)!;
  svc.book.add(updated);
  await exec(svc.deps.catalogs.indexer.setDropGuard(id, !force));
  // Any offline reason but an unusable catalog (`CATALOG_*`, kept by `goOnline`) gives way to the accept.
  if (updated.state === "offline") await svc.goOnline(id);
  else await svc.submitScan(id, { mode: "full", trigger: "demand" });
  return svc.view(svc.registry.get(id) ?? updated);
}

/**
 * The owner accepts a root that is empty for good (`library.set {acceptEmptyRoot: true}`,
 * human-only, audited): the root really lists empty now (an unmounted, changed or unreachable
 * root is refused with `LIBRARY_ROOT_SUSPECT`: that is not this), the catalog stops refusing an
 * empty root listing (`ROOT_EMPTY_SUSPECT`), and the library comes back with a full scan
 * (`trigger: mount`). The root directory is then like any other that listed empty: its files
 * go missing only after the usual confirmation (`DIR_EMPTY_SUSPECT`: a second empty listing
 * 60 s later). A listing that finds the root non-empty ends the accept.
 */
export async function acceptEmptyRoot(svc: LibraryService, id: string, actor: Actor): Promise<LibraryView> {
  const row = svc.registry.get(id);
  if (row === undefined) throw notFound(id);
  const probe = await exec(svc.registry.probe(row, RECHECK_PROBE_MS));
  if (probe.state !== "empty" && probe.state !== "online") {
    throw new FinError({
      code: "LIBRARY_ROOT_SUSPECT",
      message: "the root is not empty, it is unavailable: there is no empty root to accept",
      details: { id, state: probe.state },
    });
  }
  await exec(svc.deps.catalogs.indexer.setEmptyRootOk(id, true));
  svc.registry.auditAccept(id, "acceptEmptyRoot", actor);
  if (row.state === "offline") await svc.goOnline(id);
  else await svc.submitScan(id, { mode: "full", trigger: "mount" });
  return svc.view(svc.registry.get(id) ?? row);
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
  actor?: Actor,
): Promise<{ dirs: ReadonlyArray<RawRel>; job?: JobView }> {
  if (svc.registry.get(id) === undefined) throw notFound(id);
  const r = await exec(svc.deps.catalogs.indexer.acceptUnmounted(id, dirs));
  if (actor !== undefined) svc.registry.auditAccept(id, "acceptUnmounted", actor);
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
  if (row.state === "paused") throw new FinError({ code: "LIBRARY_PAUSED", message: "the library is paused", details: { id }, fix: "Resume the library (fin library set <lib> --resume), then scan it again." });
  if (row.state === "offline") throw new FinError({ code: "LIBRARY_OFFLINE", message: "the library is offline", details: { id, reason: row.offlineReason ?? null }, fix: "Its root is not usable right now; the server checks again every minute and scans once it is back. Scan again then." });
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
 * A `library.probe` job of up to 256 catalogued files (the probe stage's submit, `pump.ts`). A
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
  return submitProbe({ catalogs: svc.deps.catalogs, engine: svc.deps.engine, book: svc.book, files: svc.fileEvents }, row, fileIds, priority);
}
