// The agent runtime's shared pieces (sub-project 7A-2): one session store, one credential
// service and one branch-lineage cache per server, built once in `main.ts` and shared by both
// transports' handlers (whose chains check branch and path reach with them) and the runtime.
import { Context } from "effect";
import type { Clock, Ids } from "./clock.ts";
import type { SessionCredentials } from "./credentials.ts";
import type { BranchLineage } from "./reach.ts";
import type { AgentStore } from "./store.ts";

export interface AgentCore {
  readonly store: AgentStore;
  readonly credentials: SessionCredentials;
  readonly lineage: BranchLineage;
  readonly clock: Clock;
  readonly ids: Ids;
}

export class AgentCoreTag extends Context.Service<AgentCoreTag, AgentCore>()("finisherd/AgentCore") {}
