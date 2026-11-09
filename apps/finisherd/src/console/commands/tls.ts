// `finisherd tls show [--json]` (read-only) and `finisherd tls rotate` (plan 5B Task 12): the
// identity's pin, in groups of four to read aloud against a device, and a new identity key.
// A rotation revokes every device (they pair again) and cannot be undone, so the environment's
// name is typed back (owner decision of 2026-11-08).
import type { Environment } from "@finisher/contracts";
import { valueOf } from "../client.ts";
import { confirmed, requireTty, typeName } from "../confirm.ts";
import { has, onlyFlags, pinGroups, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function tls(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (rest.length > 0) usage(`tls ${sub ?? ""} takes no further arguments`);
  if (sub === "show") return show(ctx);
  if (sub === "rotate") return rotate(ctx);
  return usage(sub === undefined ? "tls needs show or rotate" : `unknown tls command: ${sub}`);
}

async function show(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "tls show", ["json"]);
  const env = await withConnection(ctx, async (c) => valueOf<Environment>(await c.call("env.describe", {})));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify({ spki: env.spki, routes: env.routes }));
    return 0;
  }
  ctx.output.out(`Identity pin: ${pinGroups(env.spki)}`);
  for (const r of env.routes) ctx.output.out(`Route (${r.kind}): ${safe(r.url)}  ${r.pin === "identity" ? "pinned" : "trusted certificate"}`);
  return 0;
}

async function rotate(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "tls rotate", []);
  requireTty(ctx.prompt, "tls rotate");
  const r = await withConnection(ctx, async (c) => {
    const env = valueOf<Environment>(await c.call("env.describe", {}));
    return confirmed<{ spki: string; revoked: number }>(
      c,
      ctx.prompt,
      `This gives "${safe(env.name)}" a new identity key and certificate. Every paired device is revoked and must pair again; the app's pin changes.`,
      typeName("environment's name", env.name),
      "env.rotateIdentity",
      {},
    );
  });
  ctx.output.out(`New identity pin: ${pinGroups(r.spki)}. Revoked ${r.revoked} device(s).`);
  return 0;
}
