// The agent runtime's shared pieces (sub-project 7A-2): one session store, one credential
// service and one branch-lineage cache per server, built once in `main.ts` and shared by both
// transports' handlers (whose chains check branch and path reach with them) and the runtime.
import { Context } from "effect";
import type { Clock, Ids } from "./clock.ts";
import type { SessionCredentials } from "./credentials.ts";
import type { OutcomeTracker } from "./outcomes.ts";
import type { RuntimeJournal } from "./runtime-deps.ts";
import type { BranchLineage } from "./reach.ts";
import type { AgentStore } from "./store.ts";

export interface AgentCore {
  readonly store: AgentStore;
  readonly credentials: SessionCredentials;
  readonly lineage: BranchLineage;
  /** What became of each agent transaction (Task 11): the chains' after stage and session delete feed it. */
  readonly outcomes: OutcomeTracker;
  readonly clock: Clock;
  readonly ids: Ids;
  /** The runtime's journal (Task 10's `AgentJournal` over both D34 stores). */
  readonly journal: RuntimeJournal;
}

export class AgentCoreTag extends Context.Service<AgentCoreTag, AgentCore>()("finisherd/AgentCore") {}
