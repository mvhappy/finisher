// Helpers of the "hooks" e2e scenario (7F Task 11): the hook configurations, the scripted
// session, trust, the audit and the hook counters.
import { join } from "node:path";
import { FIN, consoleRpc } from "./lib.mjs";

/** The scenario's steps, in order (each is announced as `step <n>: …`). */
export const STEPS = [
  "untrusted: the trim lands, no hook context, PROJECT_HOOKS_UNTRUSTED",
  "project trust lists the hooks and trusts them",
  "the trim is HOOK_BLOCKED (exit 7), one hook.block row",
  "the prompt's context is a notice {hookContext} in agent.watch",
  "a person's merge is refused by PreMerge",
  "editing the hooks file turns the block off until it is trusted again",
  "server.status.hooks counts what ran",
];

/** The project layer: a `PreCommand` hook blocking `item trim` and a `UserPrompt` hook adding context. */
export function projectHooks({ context }) {
  return {
    hooks: [
      { id: "no-trim", event: "PreCommand", match: "item.trim", run: ["node", "hook.mjs", "--write-project", "--reply", "block:trims are frozen"], file: "hook.mjs" },
      { id: "style", event: "UserPrompt", run: ["node", "hook.mjs", "--reply", `context:${context}`], file: "hook.mjs" },
    ],
  };
}

/** The user layer (`settings.json`): a `PreMerge` hook, and on Linux a probe that writes into the project folder. */
export function userHooks({ project, probe }) {
  const hooks = [{ id: "freeze-merge", event: "PreMerge", run: ["node", "hook.mjs", "--reply", "block:merges are frozen"], file: "hook.mjs" }];
  if (probe) hooks.push({ id: "probe", event: "UserPrompt", run: ["node", "hook.mjs", "--write", join(project, "planted.txt")], file: "hook.mjs" });
  return hooks;
}

/** One `fin item trim` per turn, then the end of the turn. */
export function script(turns) {
  return [{ name: "hooks", turns: Array.from({ length: turns }, () => [{ spawn: [FIN, "item", "trim", "c1", "--end", "-0.5s"] }, { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] }]) }];
}

/** The `child` results of a session's nodes, oldest first, with `fin`'s error code (if any) parsed out. */
export function childrenOf(nodes) {
  return nodes
    .filter((n) => n.kind === "notice" && n.payload?.child !== undefined)
    .map((n) => {
      const c = n.payload.child;
      let error;
      let reason;
      try {
        const e = JSON.parse(c.stdout).error;
        error = e?.code;
        reason = e?.details?.reason;
      } catch {
        // Not JSON.
      }
      return { code: c.code, error, reason, stdout: c.stdout, stderr: c.stderr };
    });
}

/** The `hookContext` notices of a session's nodes: `{event, hooks, text}` each. */
export const contextsOf = (nodes) => nodes.filter((n) => n.kind === "notice" && n.payload?.hookContext !== undefined).map((n) => n.payload.hookContext);

/** `finisherd project trust`'s calls: the hooks it lists, then `agent.project.trust` with the hash it showed. */
export async function trustProject(server, projectId) {
  const rules = await consoleRpc(server, "agent.rules.list", { projectId }, { confirm: false });
  const listed = await consoleRpc(server, "agent.hooks.list", { projectId }, { confirm: false });
  const trusted = await consoleRpc(server, "agent.project.trust", { projectId, expectedHash: rules.rulesHash, expectedHooksHash: listed.hooksHash });
  return { listed, trusted };
}

/** Every audit row of a kind (the console reads them). */
export async function auditOf(server, kind) {
  const rows = [];
  for (let cursor; ; ) {
    const page = await consoleRpc(server, "auth.audit.list", { kind, limit: 1000, ...(cursor ? { cursor } : {}) }, { confirm: false });
    rows.push(...page.entries);
    cursor = page.next;
    if (cursor === undefined) return rows;
  }
}

/** `server.status.hooks.events`. */
export async function hookStats(server) {
  return (await consoleRpc(server, "server.status", {}, { confirm: false })).hooks;
}

/** The session branch's state: the revision and clip c1 as `snapshot.get` shows them. */
export async function branchState(ws, projectId, branch) {
  const snap = await ws.call("snapshot.get", { projectId, branch });
  const items = snap.project.timelines.flatMap((t) => t.tracks.flatMap((tr) => tr.items));
  return JSON.stringify({ rev: snap.rev ?? snap.project.rev, clip: items.find((i) => i.id === "c1") });
}
