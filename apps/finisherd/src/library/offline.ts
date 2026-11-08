// A library going offline and coming back (plan 5D Decisions 17, 18; the Tasks 9–11 review),
// for `LibraryService`. Offline: the catalog marks nothing missing, stage jobs are cancelled,
// the watch stops, and the root is re-probed every 60 s (a library already offline keeps that
// cadence). Back online: the watch starts again and a full scan runs (`trigger: mount`). A
// catalog that cannot be used (`CATALOG_*`) and a root whose listing would drop most of the
// library after an accepted volume (`ROOT_DROP_SUSPECT`) stay offline until the owner acts.
import type { RootState, VolumeIdentity } from "@finisher/core-types";
import type { LibraryRow } from "./registry.ts";
import { type LibraryService, RECHECK_PROBE_MS } from "./service.ts";
import { exec, identityOf, offlineReason, sameVolume } from "./states.ts";

/** Offline reasons only the owner clears (`acceptVolume`): a re-probe never brings them back. */
export const held = (reason: string | undefined): boolean => reason !== undefined && (reason.startsWith("CATALOG_") || reason === "ROOT_DROP_SUSPECT");

/** Stores a volume measured now (a full scan, an online re-probe); a new key moves the pools. */
export function refreshVolume(svc: LibraryService, row: LibraryRow, identity: VolumeIdentity | undefined): void {
  if (identity === undefined || sameVolume(identity, row.volume)) return;
  if (identity.key !== row.volume.key) svc.log("debug", `library service: library ${row.id}'s volume key changed, confirmed by its file ids`);
  svc.registry.setVolume(row.id, identity);
  const updated = svc.registry.get(row.id);
  if (updated !== undefined) svc.book.add(updated);
}

/** Puts `id` offline with `reason` (Decision 18); `probed`: the root state comes from a probe just now. */
export async function goOffline(svc: LibraryService, id: string, reason: string, state: RootState["state"], probed: boolean): Promise<void> {
  const row = svc.registry.get(id);
  if (row === undefined || svc.closed) return;
  const live = svc.liveOf(id);
  svc.deps.health.note(id, state === "online" ? "unreachable" : state);
  // Re-probed at once when it just went offline; a library already offline keeps its cadence
  // (a probe or hash failing `LIBRARY_OFFLINE` meanwhile does not start another probe).
  if (probed) live.lastRecheckAt = svc.now();
  else if (row.state !== "offline") live.lastRecheckAt = undefined;
  if (row.state !== "paused" && (row.state !== "offline" || row.offlineReason !== reason)) {
    // A held reason (`CATALOG_*`, `ROOT_DROP_SUSPECT`) is only replaced by the owner's accept.
    if (row.state !== "offline" || !held(row.offlineReason)) svc.registry.setState(id, "offline", reason);
    svc.log("warning", `library service: library ${id} is offline (${reason})`);
  }
  await exec(svc.deps.catalogs.indexer.setOffline(id, true)).catch(() => undefined);
  // Its watch goes with it; coming back online starts it again.
  if (live.watch !== "none") {
    await exec(svc.deps.catalogs.indexer.unwatch(id)).catch(() => undefined);
    live.watch = "none";
    delete live.watchReason;
  }
  await svc.stages.stop(id);
  svc.replan();
}

/**
 * The root is usable again: online, the catalog may mark missing again, its watch starts
 * again, and a full scan (`mount`). A catalog that cannot be used (`CATALOG_*`) keeps it
 * offline whatever the root says.
 */
export async function goOnline(svc: LibraryService, id: string): Promise<void> {
  const row = svc.registry.get(id);
  if (row === undefined || svc.closed || (row.state === "offline" && row.offlineReason?.startsWith("CATALOG_"))) return;
  if (row.state === "offline") svc.registry.setState(id, "online");
  svc.deps.health.note(id, "online");
  await exec(svc.deps.catalogs.indexer.setOffline(id, false)).catch(() => undefined);
  svc.log("debug", `library service: library ${id} is online again`);
  await svc.startWatch(svc.registry.get(id) ?? row);
  svc.stages.kick(id);
  await svc.submitScan(id, { mode: "full", trigger: "mount" });
}

/** Re-probes an offline library's root (≤ 60 s, Decision 18). */
export async function recheck(svc: LibraryService, id: string): Promise<void> {
  const live = svc.liveOf(id);
  const row = svc.registry.get(id);
  if (row === undefined || live.recheck) return;
  live.recheck = true;
  live.lastRecheckAt = svc.now();
  let state: RootState;
  try {
    state = await exec(svc.registry.probe(row, RECHECK_PROBE_MS));
  } catch {
    state = { state: "unreachable", code: "PROBE_FAILED" };
  } finally {
    live.recheck = false;
  }
  const now = svc.registry.get(id);
  if (now === undefined || now.state !== "offline" || svc.closed) return svc.replan();
  if (state.state === "online" && !held(now.offlineReason)) {
    svc.refreshVolume(now, identityOf(state));
    return goOnline(svc, id);
  }
  if (state.state === "online") return svc.replan();
  await goOffline(svc, id, offlineReason(state), state.state, true);
}
