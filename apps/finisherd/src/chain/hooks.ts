// What the chain asks of the principals it does not know itself (sub-project 7A Task 7): an
// agent session's credential reaches its own branch subtree, its project folder and the
// libraries that project references, writes under the session's author, and every write it
// makes is recorded under the session after the engine answers. `agent/reach.ts` implements
// them; without hooks the chain is 5A's.
import type { FinError, FinRpcTag, Principal } from "@finisher/contracts";
import type { Effect } from "effect";
import type { Resolved } from "./resolve.ts";

export interface ChainHooks {
  /**
   * Stage 4: more roots for a principal with roots (the libraries its project references),
   * beside its projects' folders; `only`: the project a probe is for.
   */
  readonly extraRoots?: (principal: Principal, only: string | undefined) => Effect.Effect<ReadonlyArray<string>>;
  /** Stage 4, after reach and before the escalations: a refusal of its own for this principal. */
  readonly authorize?: (tag: FinRpcTag, principal: Principal, resolved: Resolved) => Effect.Effect<void, FinError>;
  /**
   * Before the handler: the author a credential principal's writes carry, whatever it sent
   * (`undefined`: the chain's default, `<kind>:<id>`).
   */
  readonly authorOf?: (principal: Principal) => string | undefined;
  /**
   * Stage 7, after the handler answered: never fails the call (the write has landed), and
   * reports its own failures itself.
   */
  readonly after?: (tag: FinRpcTag, principal: Principal, payload: unknown, result: unknown, headers: unknown) => Effect.Effect<void>;
}
