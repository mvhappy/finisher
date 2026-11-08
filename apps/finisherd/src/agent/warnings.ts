// Persistent status warnings of the agent runtime (owner decision F2; spec §8.9, §15), on
// `agent.session.start`'s answer, in `agent.providers`, in `server.status` and at start. They
// come from the one sandbox status source (`sandbox/status.ts`, plan 7C Decision 23): while the
// `agent` profile is not enforced on this host (not yet ready on this OS, or Landlock off),
// agent tools run as the user and a tool can read `local.key` and authenticate as `local`, so
// the session's scopes, branch and path reach constrain only tools that use `FIN_CREDENTIAL`
// as they should.
import type { AgentWarning } from "@finisher/contracts";
import { currentSandboxStatus, sandboxWarnings, type SandboxStatus } from "../sandbox/status.ts";

/** The agent runtime's warnings now: the `agent` profile's, if it is not enforced. */
export function agentWarnings(status: SandboxStatus = currentSandboxStatus()): AgentWarning[] {
  return sandboxWarnings(status, ["agent"]);
}
