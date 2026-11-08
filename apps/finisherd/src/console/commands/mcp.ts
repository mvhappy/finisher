// `finisherd mcp setup --out <file> [--label <name>] [--scope <scope>]...` (spec §4.7, plan 5A
// Task 9): mints an `mcp` credential (one per adapter install: 30 days unused, no absolute
// limit by default) into the file the person names. Needs a terminal and a typed code.
import { has, usage } from "../format.ts";
import { mint } from "./agent.ts";
import type { CommandContext } from "./context.ts";

export async function mcp(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub === "setup") {
    if (!has(ctx.args, "out")) usage("mcp setup needs --out <file>: where the MCP adapter reads its credential");
    return mint(ctx, "mcp", rest);
  }
  return usage(sub === undefined ? "mcp needs a command: setup" : `unknown mcp command: ${sub}`);
}
