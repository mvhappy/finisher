// Persistent status warnings of the agent runtime (owner decision F2; spec §8.9, §15). Until
// 7C's OS sandbox, every host is unsandboxed for agent tools: sessions run, with this warning
// on `agent.session.start`'s answer, in `agent.providers`, in `server.status` and at start.
import type { AgentWarning } from "@finisher/contracts";

/**
 * Before 7C an agent's tools run as the user: a tool can read `local.key` and authenticate as
 * `local`, so the session's scopes, branch and path reach constrain only tools that use
 * `FIN_CREDENTIAL` as they should.
 */
export const AGENT_TOOLS_UNSANDBOXED: AgentWarning = Object.freeze({
  code: "SANDBOX_DISABLED",
  message:
    "Agent tools run as your user, without a sandbox: a tool can bypass the session's scopes (for example by using local.key). Run agents only on projects and prompts you trust.",
  details: Object.freeze({ profile: "agent", os: process.platform, reason: "before7C" }),
  fix: "The OS sandbox for agent tools arrives with sub-project 7C; until then nothing turns this warning off.",
});

/** The agent runtime's warnings now (7A: always the unsandboxed one). */
export function agentWarnings(): AgentWarning[] {
  return [AGENT_TOOLS_UNSANDBOXED];
}
