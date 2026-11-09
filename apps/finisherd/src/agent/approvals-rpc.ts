// The approval RPCs (sub-project 7B Task 5, Decisions 11 and 12; spec §7.3, §11.2):
//
// - `agent.approval.wait {id, timeoutMs ≤ 90000}` (`agent.read`; a session for its own cards
//   only, a person for any in reach): `{approval}` when allowed (exit 0), `APPROVAL_DENIED`
//   (exit 7: do not retry) when denied or expired, `APPROVAL_PENDING` (exit 8, repeatable) when
//   the wait ran out first. Another session's card answers `APPROVAL_NOT_FOUND`, as a missing one.
// - `agent.approval.list`: a session lists only its own cards, whatever it names.
// - `agent.approval.answer` (a person's: `agent.approve`, human-only, people's kinds): a `budget`
//   card through `Runtime.approve` with its `add` (`remember` is `INVALID_REQUEST` there), a
//   `rule` or `provider` card through `Approvals.answer`, with `remember` (`remember.ts`).
import type { Approval, FinError, Principal, RememberScope } from "@finisher/contracts";
import { Effect } from "effect";
import { approvalPending } from "../chain/fixes.ts";
import { finError, toFinError } from "../errors.ts";
import type { AgentApprovals } from "./approvals.ts";
import type { BudgetRaise, Runtime } from "./runtime.ts";
import { approvalInReach, approvalNotFound, projectsFilter, sessionInReach } from "./session-access.ts";
import type { AgentLookup } from "./store-lookup.ts";
import type { AgentStore } from "./store.ts";

type Guard = <A>(tag: string, self: Effect.Effect<A, FinError>) => Effect.Effect<A, FinError>;

export interface ApprovalRpcDeps {
  readonly guard: Guard;
  readonly store: AgentStore;
  readonly lookup: AgentLookup;
  readonly runtime: Runtime;
  readonly approvals: AgentApprovals;
  readonly principalOf: (options: object) => Principal;
}

/** `APPROVAL_DENIED` (exit 7): a person said no, or no one answered in time. */
export function approvalDenied(a: Approval, how: "deny" | "expired"): FinError {
  return finError(
    "APPROVAL_DENIED",
    how === "deny" ? `approval ${a.id} was denied` : `approval ${a.id} expired before a person answered it`,
    { approval: a.id, decision: how, ...(a.by === undefined ? {} : { by: a.by }) },
    "Do not repeat this request: do something else, or ask the person what they want.",
  );
}

/** The approval `id` when the caller may see it: a session only its own (else exactly a missing one's answer). */
function visible(store: AgentStore, p: Principal, id: string) {
  return Effect.flatMap(approvalInReach(store, p, id), (r) =>
    p.kind === "agent" && r.approval.sessionId !== p.id ? Effect.fail(approvalNotFound(id)) : Effect.succeed(r),
  );
}

const sync = <A>(f: () => A) => Effect.try({ try: f, catch: toFinError });

export function approvalHandlers(deps: ApprovalRpcDeps) {
  const { guard, store } = deps;
  const by = deps.principalOf;
  return {
    "agent.approval.wait": ({ id, timeoutMs }: { id: string; timeoutMs: number }, options: object) =>
      guard("agent.approval.wait", Effect.gen(function* () {
        const { approval } = yield* visible(store, by(options), id);
        const o = yield* deps.approvals.wait(approval.id, timeoutMs);
        const now = store.approval(approval.id) ?? approval;
        if (o === "allow") return { approval: now };
        if (o === "pending") {
          const command = (now.subject as { command?: unknown } | null)?.command;
          return yield* Effect.fail(approvalPending("agent.approval.wait", now, typeof command === "string" ? command : now.kind, true));
        }
        return yield* Effect.fail(approvalDenied(now, o));
      })),
    "agent.approval.list": (q: { sessionId?: string; projectId?: string; status?: "open" | "answered" | "expired" }, options: object) =>
      guard("agent.approval.list", Effect.suspend(() => {
        const p = by(options);
        // A session sees only its own cards.
        const query = p.kind === "agent" ? { ...q, sessionId: p.id } : q;
        const filter = projectsFilter(p);
        const list = () => sync(() => ({ approvals: deps.lookup.approvalIds({ ...query, ...(filter === undefined ? {} : { projects: filter }) }).flatMap((x) => store.approval(x) ?? []) }));
        return query.sessionId === undefined ? list() : Effect.flatMap(sessionInReach(store, p, query.sessionId), list);
      })),
    "agent.approval.answer": (req: { id: string; decision: "allow" | "deny"; remember?: RememberScope; add?: BudgetRaise }, options: object) =>
      guard("agent.approval.answer", Effect.gen(function* () {
        const p = by(options);
        const { approval } = yield* approvalInReach(store, p, req.id);
        if (approval.kind === "budget") {
          if (req.remember !== undefined) {
            return yield* Effect.fail(finError("INVALID_REQUEST", "a budget approval cannot be remembered as a rule", { field: "remember", reason: "budgetApproval" }, "answer without remember"));
          }
          const r = yield* deps.runtime.approve(approval.id, req.decision, req.add ?? {}, p);
          return { approval: r.approval };
        }
        return yield* deps.approvals.answer(approval.id, req.decision, p, req.remember);
      })),
  };
}
