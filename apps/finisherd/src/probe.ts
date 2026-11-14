// `asset.probe`'s probe (review of Task 5, M2; the dangling-symlink finding after it).
import type { FinError, Principal, Probe } from "@finisher/contracts";
import { libraryProbeFile, probe, probeBeneath } from "@finisher/fin-node";
import { Effect } from "effect";
import { pathNotAllowed } from "./chain/fixes.ts";
import { type ChainProjects, inside, libraryReach, rootsOf } from "./chain/resolve.ts";
import { attemptJson, finError } from "./errors.ts";

const TAG = "asset.probe";

/**
 * The probe of `path`, canonical and checked by the chain, for `principal`. An unrestricted
 * principal gets a plain probe. A principal with roots (or a call without one, failing closed)
 * gets a probe beneath the root that holds `path` (its project's folder: `projectId`'s, for a
 * protocol 2 probe), opened from that folder without following any symlink. A path check alone
 * cannot do this: a dangling symlink canonicalises to itself, and a symlink can be swapped in
 * after any check; either way the open refuses, `PATH_NOT_ALLOWED`, and nothing of the target
 * is read.
 */
export function probeFor(
  principal: Principal | undefined,
  deps: ChainProjects,
  projectId: string | undefined,
  path: string,
): Effect.Effect<Probe, FinError> {
  return Effect.gen(function* () {
    // 5D Decision 47: a catalogued library file is read through the opener with its row, for
    // every principal that reaches its library; `probe(path)` never opens a library file.
    const row = principal === undefined || deps.libraryFile === undefined ? undefined : yield* deps.libraryFile(path);
    if (row !== undefined && principal !== undefined && libraryReach(principal, row.library)) {
      // A placeholder's data is not on this machine: refused without reading, unless the
      // library hydrates (`FILE_CLOUD_ONLY`, `open_for_read` decides the rest).
      if (row.state === "cloudOnly" && !row.hydrate) {
        return yield* Effect.fail(finError("FILE_CLOUD_ONLY", "the file is a cloud placeholder whose data is not on this machine", {}, "Make the file available offline in its sync client, then try again."));
      }
      if (row.offline) {
        return yield* Effect.fail(finError("LIBRARY_OFFLINE", "the file's library is offline", { library: row.library }, "Wait for the library to come back, or ask the owner of this Finisher server to check its folder."));
      }
      return yield* attemptJson<Probe>(() => libraryProbeFile(row.root, row.rel, row.expect, row.hydrate, path));
    }
    // "Restricted" is decided by `rootsOf` alone, as the chain decides it (review of Task 7, L3).
    const roots = principal === undefined ? [] : yield* rootsOf(principal, deps, projectId);
    if (roots === undefined) return yield* attemptJson<Probe>(() => probe(path));
    // The innermost root that holds it (projects' folders may nest).
    const root = roots.filter((r) => inside(path, r)).sort((a, b) => b.length - a.length)[0];
    if (root === undefined) return yield* Effect.fail(pathNotAllowed(TAG, path));
    return yield* Effect.mapError(attemptJson<Probe>(() => probeBeneath(root, path)), (e) =>
      e.code === "PATH_NOT_ALLOWED" ? pathNotAllowed(TAG, path) : e,
    );
  });
}
