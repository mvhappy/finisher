// `finisherd auth rotate-master` (review of 5A Tasks 8–9, M3): a new `credential.master`, which
// ends every agent and MCP credential. Needs a terminal and the typed code; says what it ends
// before it asks.
import { valueOf } from "../client.ts";
import { confirmed, requireTty } from "../confirm.ts";
import { onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function auth(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub !== "rotate-master") return usage(sub === undefined ? "auth needs a command: rotate-master" : `unknown auth command: ${sub}`);
  onlyFlags(ctx.args, "auth rotate-master", []);
  if (rest.length > 0) usage("auth rotate-master takes no arguments");
  requireTty(ctx.prompt, "auth rotate-master");
  const { ended } = await withConnection(ctx, async (c) => {
    const { credentials } = valueOf<{ credentials: unknown[] }>(await c.call("auth.credential.list", {}));
    return confirmed<{ ended: number }>(
      c,
      ctx.prompt,
      `This replaces the key every agent and MCP credential is made from. All ${credentials.length} live credential(s) stop working now, ` +
        "and so does every credential file made before, also one restored from a backup with an old control.db. Mint new ones afterwards.",
      "auth.credential.rotateMaster",
      {},
    );
  });
  ctx.output.out(`credential.master replaced: ${ended} credential(s) ended. Mint new ones for the harnesses that still need them.`);
  return 0;
}
