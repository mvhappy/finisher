// The chain's refusals (spec §3.5, §4.4, §4.6). Every fix addresses the person reading it, in
// words: no command line, no backticks (a fix handed to an injected agent must not be a
// runnable instruction). Human-only refusals say where the person can do it (`details.where`).
import type { FinError, Principal, Scope } from "@finisher/contracts";
import { finError } from "../errors.ts";

/** Where a person does what a human-only refusal names (spec §3.5). */
export type Where = "app" | "console" | "settingsFile" | "env";

const WHAT: Partial<Record<Scope, string>> = {
  "project.confirm": "confirm taking over a project or moving where it lives",
  "diagnostics.admin": "change what Finisher records or remove what it recorded",
  "library.admin": "change which folders Finisher's library reads",
  "settings.admin": "change this server's settings",
  "auth.admin": "manage credentials, devices and this server",
  "plugin.install": "install plugins",
  "plugin.admin": "add, remove or distrust plugin registries and turn plugins on or off",
};

export function humanOnly(tag: string, scope: Scope, principal: Principal): FinError {
  const what = WHAT[scope] ?? "do this";
  return finError(
    "HUMAN_ONLY",
    `${tag} needs a person: ${principal.kind} principals cannot ${what}`,
    { tag, scope, principal: principal.kind, where: "console" satisfies Where },
    `Only you, the owner of this Finisher server, can ${what}: use the Finisher app or the server's console. ` +
      "If a tool or an agent asked for this, check that you want it before you do it yourself.",
  );
}

export function scopeDenied(tag: string, missing: ReadonlyArray<Scope>, principal: Principal): FinError {
  return finError(
    "SCOPE_DENIED",
    `${tag} needs ${missing.join(", ")}, which this ${principal.kind} credential does not hold`,
    { tag, scopes: [...missing], principal: principal.kind },
    "This credential was not given the right to do this. Ask the owner of this Finisher server for a credential that includes it.",
  );
}

/**
 * `SCOPE_DENIED` (reason `principalKind`) for a call only some kinds of principal may make
 * (`RpcRule.kinds`), whatever scopes the caller holds: a session's own state is its credential's,
 * and in 7A people start and drive sessions.
 */
export function wrongKind(tag: string, principal: Principal, allowed: ReadonlyArray<string>): FinError {
  return finError(
    "SCOPE_DENIED",
    `${tag} is not available to a ${principal.kind} principal`,
    { tag, scopes: [], principal: principal.kind, allowed: [...allowed], reason: "principalKind" },
    allowed.includes("agent")
      ? "This call is for a running agent session's own credential only."
      : "Agent sessions are started and driven by a person, from the Finisher app or the console.",
  );
}

/**
 * `SCOPE_DENIED {reason: reach}`: a call that acts beyond any one project (`ruleOf(tag).allProjects`)
 * from a principal that reaches only some (5C-2 review M2).
 */
export function reachDenied(tag: string, principal: Principal): FinError {
  return finError(
    "SCOPE_DENIED",
    `${tag} acts on every project, and this ${principal.kind} credential reaches only some`,
    { tag, scopes: [], reason: "reach", principal: principal.kind },
    "Ask the owner of this Finisher server for a credential that reaches every project, or act on one job at a time.",
  );
}

/** `PROJECT_NOT_FOUND` for a project outside a principal's reach: the same answer as an unknown one. */
export function outOfReach(projectId: string | undefined): FinError {
  return finError(
    "PROJECT_NOT_FOUND",
    projectId === undefined ? "no such project" : `project ${projectId} is not known to this server`,
    projectId === undefined ? {} : { projectId },
    "Check the project's id or folder.",
  );
}

export function pathNotAllowed(tag: string, path: string): FinError {
  return finError(
    "PATH_NOT_ALLOWED",
    `${tag}: ${path} is outside the folders this credential may use`,
    { tag, path },
    "This credential may only use the folders of its own projects. Use a file inside one of them, or ask the owner of this Finisher server for wider access.",
  );
}

export function branchNotAllowed(tag: string, branch: string): FinError {
  return finError(
    "BRANCH_NOT_ALLOWED",
    `${tag}: this credential may not write to branch ${branch}`,
    { tag, branch },
    "This credential may only write to the branches it was given. Write to one of those, or ask the owner of this Finisher server for a credential that covers this branch.",
  );
}

/** A write into a protected branch from a principal without the scope that reaches it (5E Decision 16). */
export function branchProtected(tag: string, branch: string): FinError {
  return finError(
    "BRANCH_PROTECTED",
    `${tag}: branch ${branch} is protected`,
    { tag, branch },
    "This branch is protected: a person reviews these changes and merges them. Write to another branch and ask for a review.",
  );
}

export function approvalRequired(tag: string, approvalId: string, reason: string): FinError {
  return finError(
    "APPROVAL_REQUIRED",
    `${tag} waits for a person's approval: ${reason}`,
    { tag, approvalId, reason },
    "A person must approve this first. Ask the owner of this Finisher server to look at the request in the Finisher app.",
  );
}

/**
 * `APPROVAL_PENDING {approval}` (exit 8, sub-project 7 §7.3): a person has been asked on a
 * durable card; the request is not denied. `reused`: an identical open card answered.
 */
export function approvalPending(tag: string, approval: { readonly id: string; readonly expiresAt: number }, command: string, reused: boolean): FinError {
  return finError(
    "APPROVAL_PENDING",
    `${command} waits for a person to answer approval ${approval.id}`,
    { tag, approval: approval.id, command, expiresAt: approval.expiresAt, reused },
    "A person has been asked to approve this; it is not denied. Wait for their answer, then repeat the same request.",
  );
}

/**
 * `CONSENT_UI_ONLY` (exit 2; arch §21.1, D14b): turning training on is the app's onboarding
 * question, answered by a person; every principal but a `device` is refused, the console too.
 * The fix names the ways in, in words.
 */
export function consentUiOnly(tag: string, principal: Principal): FinError {
  return finError(
    "CONSENT_UI_ONLY",
    "turning training on is a person's choice, made in the Finisher app, not a command's",
    { tag, purpose: "training", value: "on", principal: principal.kind, where: "app" satisfies Where },
    "Only you, the owner of this Finisher server, can opt in to training data: answer the question in the Finisher app, " +
      "or set privacy.training to on by hand in the server's settings file, or set FIN_TRAINING to on in the environment the server starts in.",
  );
}

/** Every human-only refusal's fix, for the test that keeps commands out of them. */
export function humanOnlyFixes(): ReadonlyArray<FinError> {
  const nobody = { kind: "agent", id: "x", scopes: [], projects: "*", branches: "*", human: false } as unknown as Principal;
  return (Object.keys(WHAT) as Scope[]).map((s) => humanOnly("test.tag", s, nobody));
}
