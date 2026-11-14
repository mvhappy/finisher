// `BranchReady`, `PreMerge` and `PostMerge` on the `auto` merge path (7F plan Task 8, Decision 13;
// spec §9.1). `startAgentReviews` (`agent/wiring.ts`) subscribes `branchReady` to the review
// link, gives the trigger `preMerge` (a test's own `AutoMergeTriggerOptions.preMerge` still
// overrides it) and wraps the trigger with `afterMerge`.
//
// - **`BranchReady`** runs for every `branchReady` event (a session's review request, its end with
//   unmerged commits), as the session (its credential's identity) in the event's project, with
//   payload `{sessionId, projectId, branch, headSeq, reviewId?}`. Its outcome is memoised per
//   event object, so the listener and the trigger's point share one run, and only that run
//   reaches the session: a block as a `notice {hook: {event: BranchReady, id, blocked}}`, context
//   through `SessionNotices.deliver` (Task 6 steers it into the running session or holds it).
//   An event of a session this server does not know runs nothing.
// - **`preMerge(ev, session, item)`**: false (the trigger's `hookBlocked`) when that `BranchReady`
//   outcome blocked, else when a `PreMerge` hook blocks (a failure or a timeout blocks by
//   default, Decision 3); its payload `{via: autoMerge, sessionId, from, into, item}`. A
//   `PreMerge` block is a notice in the session too (no caller gets `HOOK_BLOCKED` here); its
//   context is delivered as `BranchReady`'s is.
// - **`PostMerge`** (observe-only) after a merged outcome: `{via: autoMerge, sessionId, reviewId,
//   mergeId, from, into?}`.
//
// The trigger's types are matched structurally: only the server wiring may depend on the
// trigger's module (a guard of the 7B tests).
import type { HookEvent, ReviewItem, Session } from "@finisher/contracts";
import { Effect } from "effect";
import type { BranchReady } from "../agent/review-link.ts";
import type { Invoker } from "../plugins/host/rights.ts";
import type { HookAt, HookEngine, HookOutcome, SessionNotices } from "./engine.ts";

export interface ReviewHooksDeps {
  readonly engine: Pick<HookEngine, "fire" | "observe">;
  readonly sessions: { get(id: string): Session | undefined };
  /** The engine's session port (absent: nothing reaches the session). */
  readonly notices?: SessionNotices;
}

export interface ReviewHooks {
  /** The review link's listener: `BranchReady` hooks for one event. */
  readonly branchReady: (ev: BranchReady) => Promise<void>;
  /** The trigger's point (`PreMergeHook`): false refuses the merge (`hookBlocked`). */
  readonly preMerge: (ev: BranchReady, session: Session, item: ReviewItem) => Effect.Effect<boolean>;
  /** The trigger, firing `PostMerge` after each merged outcome. */
  readonly afterMerge: <O extends { readonly merged: boolean }, E>(trigger: (ev: BranchReady) => Effect.Effect<O, E>) => (ev: BranchReady) => Effect.Effect<O, E>;
}

/** A session as hooks see its principal: its credential's identity, its project. */
export function sessionPrincipal(s: Session): Invoker {
  return { kind: "agent", id: s.credentialId, scopes: [], projects: [s.projectId] };
}

const atOf = (s: Session, projectId: string): HookAt => ({ projectId, session: s, principal: sessionPrincipal(s) });

export function reviewHooks(d: ReviewHooksDeps): ReviewHooks {
  // Keyed by the event object: every listener of one emission gets the same one.
  const ready = new WeakMap<BranchReady, Promise<HookOutcome>>();
  const items = new WeakMap<BranchReady, ReviewItem>();

  /** What `BranchReady`'s hooks answered, and wrote into the session, once per event. */
  const reported = (event: HookEvent, at: HookAt, out: HookOutcome): void => {
    if (out.blocked !== undefined) d.notices?.notice(at, { hook: { event, id: out.blocked.hook, layer: out.blocked.layer, blocked: out.blocked.reason } });
    if (out.context !== undefined) d.notices?.deliver(at, event, { ran: out.ran, context: out.context, ...(out.contextHooks === undefined ? {} : { contextHooks: out.contextHooks }) });
  };

  const branchReadyOf = (ev: BranchReady): Promise<HookOutcome> => {
    let out = ready.get(ev);
    if (out === undefined) {
      const s = d.sessions.get(ev.sessionId);
      if (s === undefined) {
        out = Promise.resolve({ ran: 0 });
      } else {
        const at = atOf(s, ev.projectId);
        const payload = { sessionId: ev.sessionId, projectId: ev.projectId, branch: ev.branch, headSeq: ev.headSeq, ...(ev.reviewId === undefined ? {} : { reviewId: ev.reviewId }) };
        out = Effect.runPromise(d.engine.fire("BranchReady", at, payload)).then((o) => {
          reported("BranchReady", at, o);
          return o;
        });
      }
      ready.set(ev, out);
    }
    return out;
  };

  const preMerge = (ev: BranchReady, session: Session, item: ReviewItem): Effect.Effect<boolean> =>
    Effect.gen(function* () {
      items.set(ev, item);
      const first = yield* Effect.promise(() => branchReadyOf(ev));
      if (first.blocked !== undefined) return false;
      const at = atOf(session, ev.projectId);
      const payload = {
        via: "autoMerge",
        sessionId: session.id,
        ...(item.source === undefined ? {} : { from: item.source }),
        ...(item.target === undefined ? {} : { into: item.target }),
        item: {
          id: item.id,
          kind: item.kind,
          title: item.title,
          ...(item.source === undefined ? {} : { source: item.source }),
          ...(item.target === undefined ? {} : { target: item.target }),
          ...(item.snapshot === undefined ? {} : { snapshot: item.snapshot }),
        },
      };
      const out = yield* d.engine.fire("PreMerge", at, payload);
      reported("PreMerge", at, out);
      return out.blocked === undefined;
    });

  const afterMerge: ReviewHooks["afterMerge"] = (trigger) => (ev) =>
    Effect.tap(trigger(ev), (o) =>
      Effect.sync(() => {
        if (!o.merged) return;
        const s = d.sessions.get(ev.sessionId);
        if (s === undefined) return;
        const item = items.get(ev);
        const mergeId = (o as { readonly mergeId?: string }).mergeId;
        d.engine.observe("PostMerge", atOf(s, ev.projectId), {
          via: "autoMerge",
          sessionId: ev.sessionId,
          ...(ev.reviewId === undefined ? {} : { reviewId: ev.reviewId }),
          ...(mergeId === undefined ? {} : { mergeId }),
          from: item?.source ?? ev.branch,
          ...(item?.target === undefined ? {} : { into: item.target }),
        });
      }),
    );

  return { branchReady: async (ev) => void (await branchReadyOf(ev)), preMerge, afterMerge };
}
