// Copied or moved server data (5B Decision 31, spec §5.8). At start fin-node's
// `serverIdentity` says whether this server data's id was just renewed (it is a copy, or it
// moved to another machine or directory). When it was, and `server_ids` already maps other ids
// to this `envId`, two servers may now claim one environment: the environment becomes `moved`
// (audited `env.moved {reason}`) and stays so until the owner adopts it (a move) or resets it
// (a copy). While moved, `local` and `console` work; devices and the remote listener do not.
import type { ControlStore } from "../control/db.ts";
import type { Environment } from "./descriptor.ts";

/** Marks `env` moved when `renewed` says the id was renewed and other ids ran this environment. True when it is moved now. */
export function checkMoved(env: Environment, store: ControlStore, renewed: string | undefined): boolean {
  if (renewed !== undefined && env.state() !== "moved") {
    const row = store.db.prepare("SELECT COUNT(*) AS n FROM server_ids WHERE envId = ? AND serverId != ?").get(store.envId, store.serverId) as { n: number };
    if (row.n > 0) env.markMoved(renewed);
  }
  return env.state() === "moved";
}
