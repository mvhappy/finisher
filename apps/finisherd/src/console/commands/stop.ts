// `finisherd stop`: the graceful stop (spec §4.7). A service command: it runs without a terminal,
// because service managers call it (and Windows has no SIGTERM); a same-user process could end
// the server anyway. It waits until the server has removed `finisherd.json`.
import { readFileSync } from "node:fs";
import { infoPath } from "../../config.ts";
import { consoleError, valueOf } from "../client.ts";
import { onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

/** How long `stop` waits for the server to finish shutting down. */
export const STOP_WAIT_MS = 30_000;

const pidOf = (serverData: string): number | undefined => {
  try {
    return (JSON.parse(readFileSync(infoPath(serverData), "utf8")) as { pid?: number }).pid;
  } catch {
    return undefined;
  }
};

export async function stop(ctx: CommandContext, waitMs: number = STOP_WAIT_MS): Promise<number> {
  onlyFlags(ctx.args, "stop", []);
  if (ctx.args.words.length > 0) usage("stop takes no arguments");
  const pid = pidOf(ctx.serverData);
  await withConnection(ctx, async (c) => valueOf(await c.call("server.stop", {})));
  const deadline = Date.now() + waitMs;
  while (pid !== undefined && pidOf(ctx.serverData) === pid) {
    if (Date.now() > deadline) {
      throw consoleError("SERVER_TIMEOUT", "the server is still shutting down", { waitedMs: waitMs }, "Wait a little and check again with the status command.");
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  ctx.output.out("The Finisher server has stopped.");
  return 0;
}
