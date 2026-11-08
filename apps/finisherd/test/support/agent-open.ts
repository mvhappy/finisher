// Opening an agent session as the `local` starter (7A Task 7), with the root branch created
// over RPC as the runtime does through the registry. No vitest here: the agent session host
// (`agent-session-host.ts`, which `fin`'s Rust tests run) uses it too.
import type { Mode } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect } from "effect";
import type { AgentCore } from "../../src/agent/core.ts";
import { openSession, type SessionBranchOps } from "../../src/agent/credentials.ts";
import { toFinError } from "../../src/errors.ts";
import { LOCAL } from "../../src/principal.ts";
import type { NdjsonClient } from "./ndjson-client.ts";

/** `SessionBranchOps` over RPC, as `c` (the starter). */
export function rpcBranchOps(c: NdjsonClient): SessionBranchOps {
  const call = <A>(tag: string, payload: unknown) =>
    Effect.flatMap(Effect.promise(() => c.call(tag, payload)), (e) => (e.ok ? Effect.succeed(e.value as A) : Effect.fail(toFinError(Object.assign(new Error(e.error.message), { ...e.error, finNode: true })))));
  const branches = (projectId: string) => Effect.map(call<{ branches: BranchView[] }>("branch.list", { projectId, all: true }), (r) => r.branches);
  return {
    branches,
    resolveBranch: (projectId, ref) =>
      Effect.flatMap(branches(projectId), (bs) => {
        const b = bs.find((x) => x.id === ref || x.name === ref || x.display === ref);
        return b === undefined ? Effect.fail(toFinError(Object.assign(new Error("no branch"), { code: "BRANCH_NOT_FOUND", finNode: true }))) : Effect.succeed(b.id);
      }),
    createBranch: (projectId, req) => Effect.map(call<{ branch: BranchView }>("branch.create", { projectId, ...req }), (r) => r.branch),
  };
}

export interface OpenOptions {
  readonly mode?: Mode;
  readonly role?: string;
  readonly base?: string;
  readonly root?: string;
}

/** Opens a session on `projectId` as `local` (Decision 15 steps 2–5). */
export async function open(core: AgentCore, starter: NdjsonClient, projectId: string, o: OpenOptions = {}) {
  return Effect.runPromise(
    openSession(
      { creds: core.credentials, store: core.store, ops: rpcBranchOps(starter), ids: core.ids },
      {
        projectId,
        harness: "builtin",
        provider: "openrouter",
        model: "anthropic/claude-sonnet-5.5",
        weights: "closed",
        role: o.role ?? "editor",
        mode: o.mode ?? "draft",
        budget: {},
        createdBy: { principal: { kind: "local", id: "local" } },
        ...(o.base === undefined ? {} : { base: o.base }),
        ...(o.root === undefined ? {} : { root: o.root }),
      },
      LOCAL,
    ),
  );
}

