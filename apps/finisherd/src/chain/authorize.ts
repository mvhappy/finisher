// Stage 4 of the chain (spec §4.4, §4.5, §4.6; plan 5A Decisions 18–21): every scope the RPC
// needs (all of them), the human-only rule, and reach (the principal's projects and, for
// principals with roots, every resolved path). In this order:
//   0. device-only payloads (training on, Decision 21): `CONSENT_UI_ONLY` for every other
//      principal, the console too, before the human check;
//   1. human-only, over every scope the call needs: its refusal tells a person what to do,
//      which `SCOPE_DENIED` does not. Which human-only scopes a call needs follows from its
//      payload alone, never from whether a project is open (`scopes.test.ts` pins it);
//   2. the RPC's own scopes;
//   3. reach;
//   4. the scopes the resolved facts add (attaching or loading, a reopen), so that a refusal
//      never tells a caller whether a project outside its reach is open here (review of
//      Task 5, L1);
//   5. a call that acts beyond any one project (`allProjects`) needs `projects: "*"`
//      (5C-2 review M2: `SCOPE_DENIED {reason: reach}`).
import {
  baseScopes,
  escalatedScopes,
  type FinError,
  type FinRpcTag,
  HUMAN_ONLY,
  type Principal,
  requiredScopes,
  ruleOf,
  type Scope,
} from "@finisher/contracts";
import { Effect } from "effect";
import { consentUiOnly, humanOnly, outOfReach, pathNotAllowed, reachDenied, scopeDenied } from "./fixes.ts";
import { type ChainProjects, inside, type Resolved, rootsOf } from "./resolve.ts";

/** The scope half of stage 4: human-only first, then every required scope. */
export function checkScopes(tag: string, required: ReadonlyArray<Scope>, principal: Principal): FinError | undefined {
  const human = required.find((s) => HUMAN_ONLY.has(s));
  if (human !== undefined && !principal.human) return humanOnly(tag, human, principal);
  const missing = required.filter((s) => !principal.scopes.includes(s));
  return missing.length > 0 ? scopeDenied(tag, missing, principal) : undefined;
}

export function authorize(
  tag: FinRpcTag,
  principal: Principal,
  payload: unknown,
  resolved: Resolved,
  deps: ChainProjects,
): Effect.Effect<void, FinError> {
  return Effect.gen(function* () {
    if (principal.kind !== "device" && ruleOf(tag).deviceOnly?.(payload as never) === true) {
      return yield* Effect.fail(consentUiOnly(tag, principal));
    }
    const human = requiredScopes(tag, payload, resolved.facts).find((s) => HUMAN_ONLY.has(s));
    if (human !== undefined && !principal.human) return yield* Effect.fail(humanOnly(tag, human, principal));
    const refused = checkScopes(tag, baseScopes(tag), principal);
    if (refused !== undefined) return yield* Effect.fail(refused);
    if (principal.projects !== "*") {
      const allowed = principal.projects;
      // `project.open` by folder: a folder that names no project, and one that names a project
      // outside the principal's reach, get the same answer, without the id, so a caller cannot
      // learn which folders are projects or what they are (review of Task 6, finding 5). A
      // folder that names no project cannot be checked: closed, as an unknown project.
      if (tag === "project.open" && (resolved.openedProject === undefined || !allowed.includes(resolved.openedProject))) {
        return yield* Effect.fail(outOfReach(undefined));
      }
      const outside = resolved.projects.find((p) => !allowed.includes(p));
      if (outside !== undefined) return yield* Effect.fail(outOfReach(outside));
    }
    const roots = yield* rootsOf(principal, deps, resolved.pathsIn);
    if (roots !== undefined) {
      const outside = resolved.paths.find((p) => !roots.some((r) => inside(p, r)));
      if (outside !== undefined) return yield* Effect.fail(pathNotAllowed(tag, outside));
    }
    const escalated = checkScopes(tag, escalatedScopes(tag, payload, resolved.facts), principal);
    if (escalated !== undefined) return yield* Effect.fail(escalated);
    if (principal.projects !== "*" && ruleOf(tag).allProjects?.(payload as never) === true) return yield* Effect.fail(reachDenied(tag, principal));
  });
}
