// A `finisherd` in a child process for the Claude Code crash tests (plan 7D Task 10, after 7A's
// SIGKILLed child server): `node claude-crash-child.ts <serverData> <projectDir> <point>` serves
// `<serverData>` (its `settings.json` names the fake CLI) with the agent profile not ready
// (`direct` mode), creates a project, starts a Claude Code session, waits for `<point>` and
// prints one JSON line `{"type":"point","sessionId","projectId"}`; then it waits to be killed.
// Points: `toolCall`, `toolResult` (the node exists), `tx` (the session committed), or a file
// name under the session's workspace (the fake's shell wrote it).
import { existsSync } from "node:fs";
import { join } from "node:path";
import type { Session } from "@finisher/contracts";
import { sandboxStatus } from "@finisher/fin-node";
import { Effect } from "effect";
import type { AgentCore } from "../../src/agent/core.ts";
import { serve, StderrLogger } from "../../src/main.ts";
import { setSandboxStatusSource } from "../../src/sandbox/status.ts";
import { addAsset, commit, connectProved, ok, type ServerInfo, tx } from "./ndjson-client.ts";

const [serverData, projectDir, point] = process.argv.slice(2) as [string, string, string];
setSandboxStatusSource(() => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } }));

let core: AgentCore | undefined;
let ready!: (i: ServerInfo) => void;
const info = new Promise<ServerInfo>((r) => (ready = r));
Effect.runFork(
  serve({ serverData, onAgentCore: (c) => (core = c), agents: { timers: false }, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)),
);
const local = await connectProved({ info: await info, serverData }, "local");
const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: projectDir, name: "P", rate: "30" })).projectId;
ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
const s = ok<{ session: Session }>(await local.call("agent.session.start", { projectId, harness: "claudeCode", prompt: "trim c1" })).session;

const reached = (): boolean => {
  const store = core!.store;
  if (point === "toolCall" || point === "toolResult") return store.nodes(s.id, 1, 1000).some((n) => n.kind === point);
  if (point === "tx") return store.txsOfSession(s.id).length > 0;
  return existsSync(join(projectDir, "workspace", s.id, point));
};
const end = Date.now() + 30_000;
while (!reached()) {
  if (Date.now() > end) throw new Error(`claude-crash-child: ${point} not reached`);
  await new Promise((r) => setTimeout(r, 20));
}
process.stdout.write(`${JSON.stringify({ type: "point", sessionId: s.id, projectId })}\n`);
setInterval(() => {}, 60_000);
