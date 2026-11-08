// `asset.probe`'s probe (review of Task 5, M2; the dangling-symlink finding after it).
import type { FinError, Principal, Probe } from "@finisher/contracts";
import { probe, probeBeneath } from "@finisher/fin-node";
import { Effect } from "effect";
import { pathNotAllowed } from "./chain/fixes.ts";
import { type ChainProjects, inside, rootsOf } from "./chain/resolve.ts";
import { attemptJson } from "./errors.ts";

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
  if (principal?.projects === "*") return attemptJson<Probe>(() => probe(path));
  return Effect.gen(function* () {
    const roots = principal === undefined ? [] : ((yield* rootsOf(principal, deps, projectId)) ?? []);
    // The innermost root that holds it (projects' folders may nest).
    const root = roots.filter((r) => inside(path, r)).sort((a, b) => b.length - a.length)[0];
    if (root === undefined) return yield* Effect.fail(pathNotAllowed(TAG, path));
    return yield* Effect.mapError(attemptJson<Probe>(() => probeBeneath(root, path)), (e) =>
      e.code === "PATH_NOT_ALLOWED" ? pathNotAllowed(TAG, path) : e,
    );
  });
}
