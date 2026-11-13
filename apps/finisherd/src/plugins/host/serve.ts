// What the plugin host's calls read in the server (sp8b Task 7), built from its pieces once
// they exist: the live invokers (credentials and devices, Task 6's `liveInvokers`), branch
// reach over 7A-2's session tree (`agentSubtree`) and the project registry's snapshots.
import { Effect } from "effect";
import { BranchReach, type BranchLineage } from "../../agent/reach.ts";
import type { AgentStore } from "../../agent/store.ts";
import type { Credentials } from "../../auth/credentials.ts";
import type { DeviceStore } from "../../auth/devices.ts";
import type { ProjectRegistryShape } from "../../projects.ts";
import type { CallDeps } from "./calls.ts";
import { agentSubtree } from "./reach.ts";
import { liveInvokers } from "./rights.ts";

export interface ServeSources {
  readonly credentials: Pick<Credentials, "lookup">;
  readonly devices: Pick<DeviceStore, "get" | "principalOf">;
  readonly sessions: AgentStore;
  readonly lineage: BranchLineage;
  readonly projects: ProjectRegistryShape;
}

export function pluginCallDeps(s: ServeSources): CallDeps {
  const p = s.projects;
  const chainProjects = {
    isOpen: p.isOpen,
    dirOf: p.dirOf,
    indexedDirOf: p.indexedDirOf,
    resolveBranch: p.resolveBranch,
    branches: (projectId: string) => Effect.map(p.branches(projectId, true), (r) => r.branches),
  };
  return {
    live: liveInvokers({
      credential: (ref) => s.credentials.lookup(ref)?.principal,
      device: (id) => {
        const d = s.devices.get(id);
        return d === undefined || d.revokedAt !== undefined ? undefined : s.devices.principalOf(d);
      },
    }),
    reach: { subtree: agentSubtree(s.sessions, new BranchReach(s.sessions, s.lineage, chainProjects)) },
    snapshot: (projectId, branch) => p.snapshot(projectId, branch),
  };
}
