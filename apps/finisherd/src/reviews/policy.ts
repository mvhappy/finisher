// The per-project branch policy write (5E plan Decisions 18–19, spec §8.5): `review.policy
// {projectId, protected?, requireReview?, autoMerge?}`, human-only and confirmed (the chain
// checks both: `auth.admin`, `needsConfirmation`). Protected entries are branch references,
// resolved to ids through the store; what the call does not send is kept. The write holds the
// project's policy gate exclusive (`gate.ts`), so it lands between the project's writes, never
// inside one. Audited `review.policy {old, new}` with branch ids and booleans only.
//
// Turning auto merge on is `AUTO_MERGE_UNAVAILABLE` while `AUTO_MERGE_AVAILABLE` is false:
// sub-project 10 flips it once QA-gate and critic verdicts are recorded (Decision 19). Turning
// it on records who did (`enabledBy`, kept in `branch_policy.autoMerge`, not in the answer);
// the internal path (`auto-merge.ts`) merges in that person's name and refuses without one.
import type { BranchPolicy, FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { chainOf } from "../chain/chain.ts";
import type { ChainProjects } from "../chain/resolve.ts";
import type { Audit } from "../control/audit.ts";
import { finError } from "../errors.ts";
import type { Reviews } from "./service.ts";

/** Whether the auto-merge path may be turned on and run (sub-project 10 sets it; `auto-merge.ts` checks it too). */
export const AUTO_MERGE_AVAILABLE = false;

export interface PolicyPayload {
  readonly projectId: string;
  readonly protected?: ReadonlyArray<string> | undefined;
  readonly protect?: ReadonlyArray<string> | undefined;
  readonly unprotect?: ReadonlyArray<string> | undefined;
  readonly requireReview?: boolean | undefined;
  /** The whole setting: `roles` absent admits every role (7B Task 12). */
  readonly autoMerge?: { readonly enabled: boolean; readonly roles?: ReadonlyArray<string> | undefined; readonly into?: string | undefined } | undefined;
}

export interface PolicyDeps {
  readonly reviews: Pick<Reviews, "store" | "gate">;
  readonly projects: Pick<ChainProjects, "resolveBranch">;
  readonly audit?: Audit | undefined;
  readonly transport: string;
  /** The names of the loaded roles (the agent runtime's catalogue): `autoMerge.roles` must name them. */
  readonly roles?: () => ReadonlySet<string>;
  /** Tests only (sub-project 10's switch); default `AUTO_MERGE_AVAILABLE`. */
  readonly autoMergeAvailable?: boolean;
}

export function autoMergeUnavailable(): FinError {
  return finError(
    "AUTO_MERGE_UNAVAILABLE",
    "automatic merging is not available yet",
    {},
    "Automatic merging needs the review checks of a later Finisher release. Review and merge the changes yourself for now.",
  );
}

/** The audited form of a policy: branch ids, booleans and the admitted roles' names. */
const word = (p: BranchPolicy) => ({
  protected: [...p.protected],
  requireReview: p.requireReview,
  autoMerge: p.autoMerge.enabled,
  ...(p.autoMerge.roles === undefined ? {} : { autoMergeRoles: [...p.autoMerge.roles] }),
  ...(p.autoMerge.into === undefined ? {} : { autoMergeInto: p.autoMerge.into }),
});

/** The written `autoMerge`: roles deduplicated and checked against the loaded roles, `into` resolved to an id. */
function autoMergeOf(deps: PolicyDeps, projectId: string, req: NonNullable<PolicyPayload["autoMerge"]>): Effect.Effect<BranchPolicy["autoMerge"], FinError> {
  return Effect.gen(function* () {
    const roles = req.roles === undefined ? undefined : [...new Set(req.roles)];
    const known = deps.roles?.();
    const unknown = known === undefined ? undefined : roles?.find((r) => !known.has(r));
    if (unknown !== undefined) {
      return yield* Effect.fail(finError("INVALID_REQUEST", "auto merge names a role this server does not have", { field: "roles", role: unknown }, "Name the server's roles (`roles/*.json` `name`)."));
    }
    const into = req.into === undefined ? undefined : yield* deps.projects.resolveBranch(projectId, req.into);
    return { enabled: req.enabled, ...(roles === undefined ? {} : { roles }), ...(into === undefined ? {} : { into }) };
  });
}

/** Stores the new policy and answers it with the one it replaced. */
export function writePolicy(deps: PolicyDeps, principal: { readonly kind: string; readonly id: string }, req: PolicyPayload) {
  return Effect.gen(function* () {
    if (req.autoMerge?.enabled === true && !(deps.autoMergeAvailable ?? AUTO_MERGE_AVAILABLE)) return yield* Effect.fail(autoMergeUnavailable());
    const { store, gate } = deps.reviews;
    const deltas = req.protect !== undefined || req.unprotect !== undefined;
    if (deltas && req.protected !== undefined) {
      return yield* Effect.fail(finError("INVALID_REQUEST", "protected cannot be sent with protect or unprotect", { field: "protected", reason: "exclusive" }, "Send the whole list, or the branches to add and remove."));
    }
    const resolve = (refs: ReadonlyArray<string> | undefined) => Effect.forEach(refs ?? [], (ref) => deps.projects.resolveBranch(req.projectId, ref));
    // Resolved before the gate (an unknown branch changes nothing); applied to the list as it is inside it.
    const add = yield* resolve(req.protect);
    const remove = yield* resolve(req.unprotect);
    if (add.some((id) => remove.includes(id))) {
      return yield* Effect.fail(finError("INVALID_REQUEST", "a branch is in both protect and unprotect", { field: "protect", reason: "both" }, "Name it in one of them."));
    }
    // The whole auto-merge setting (7B Task 12): roles deduplicated and known, `into` as an id.
    const autoMerge = req.autoMerge === undefined ? undefined : yield* autoMergeOf(deps, req.projectId, req.autoMerge);
    return yield* gate.exclusive(
      req.projectId,
      Effect.gen(function* () {
        const current = store.policy(req.projectId);
        const ids = deltas
          ? [...current.protected, ...add].filter((id) => !remove.includes(id))
          : req.protected === undefined
            ? current.protected
            : yield* resolve(req.protected);
        const policy: BranchPolicy = {
          protected: [...new Set(ids)],
          requireReview: req.requireReview ?? current.requireReview,
          autoMerge: autoMerge ?? current.autoMerge,
        };
        // Who turned auto merge on: the policy actor merges in their name (Decision 19).
        const enabledBy = req.autoMerge?.enabled === true ? `${principal.kind}:${principal.id}` : undefined;
        const { old } = store.setPolicy(req.projectId, policy, enabledBy);
        deps.audit?.append({
          kind: "review.policy",
          principal: { kind: principal.kind, id: principal.id },
          outcome: "ok",
          details: { projectId: req.projectId, transport: deps.transport, old: word(old), new: word(policy) },
        });
        return { policy, old };
      }),
    );
  });
}

/** The `review.policy` handler. */
export function policyHandler(deps: PolicyDeps) {
  return (payload: PolicyPayload, options: object): Effect.Effect<{ policy: BranchPolicy; old: BranchPolicy }, FinError> =>
    Effect.suspend(() => {
      const c = chainOf(options);
      if (c === undefined) return Effect.die(new Error("review.policy reached its handler without the chain"));
      return writePolicy(deps, c.principal, payload);
    });
}
