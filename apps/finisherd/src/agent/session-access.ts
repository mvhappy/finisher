// Project reach for the session RPCs (sub-project 7A; for Task 12's handlers). The chain checks
// a payload's `projectId` against `principal.projects`, but `agent.session.*`, `agent.watch`
// and `agent.approval.answer` name a session or an approval, not a project: each handler must
// resolve it through these, so a caller limited to some projects can neither see nor drive a
// session of another one. Out of reach answers exactly what a missing one does
// (`SESSION_NOT_FOUND`, `APPROVAL_NOT_FOUND`), so nothing tells the two apart.
import type { Approval, FinError, Principal, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "../errors.ts";

/** What the helpers read (`AgentStore` is one). */
export interface SessionLookup {
  get(sessionId: string): Session | undefined;
  approval(id: string): Approval | undefined;
}

const inReach = (p: Principal, projectId: string) => p.projects === "*" || p.projects.includes(projectId);

// The store's own `SESSION_NOT_FOUND`, word for word.
const notFound = (sessionId: string): FinError =>
  finError("SESSION_NOT_FOUND", "no such agent session", { sessionId }, "list sessions with `finisherd agent ls`");

/** The session `sessionId` when its project is in `principal`'s reach, else `SESSION_NOT_FOUND`. */
export function sessionInReach(store: SessionLookup, principal: Principal, sessionId: string): Effect.Effect<Session, FinError> {
  return Effect.suspend(() => {
    const s = store.get(sessionId);
    return s !== undefined && inReach(principal, s.projectId) ? Effect.succeed(s) : Effect.fail(notFound(sessionId));
  });
}

/** `APPROVAL_NOT_FOUND` (exit 3), for a missing approval and one out of reach alike. */
export function approvalNotFound(id: string): FinError {
  return finError("APPROVAL_NOT_FOUND", "no such approval", { id }, "list the approvals you can see with `fin approval ls` (a person: `finisherd agent show <session>`)");
}

/** The approval `id` and its session when in `principal`'s reach, else `APPROVAL_NOT_FOUND`. */
export function approvalInReach(store: SessionLookup, principal: Principal, id: string): Effect.Effect<{ approval: Approval; session: Session }, FinError> {
  return Effect.suspend(() => {
    const approval = store.approval(id);
    const session = approval === undefined ? undefined : store.get(approval.sessionId);
    return approval !== undefined && session !== undefined && inReach(principal, session.projectId)
      ? Effect.succeed({ approval, session })
      : Effect.fail(approvalNotFound(id));
  });
}

/** `agent.session.list`'s `projects` filter for `principal` (undefined: every project). */
export function projectsFilter(principal: Principal): ReadonlyArray<string> | undefined {
  return principal.projects === "*" ? undefined : principal.projects;
}
