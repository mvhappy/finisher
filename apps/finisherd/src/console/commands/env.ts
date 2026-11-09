// `finisherd env show [--json]` (read-only), `env name <name>`, `env adopt` and `env reset` (plan
// 5B Task 12, spec §5.8): the environment devices pair with. Naming and adopting are y/N; a
// reset gives the data a new identity, deletes every device and revokes every agent and MCP
// credential, so the environment's name is typed back (owner decision of 2026-11-08).
import type { Environment } from "@finisher/contracts";
import { valueOf } from "../client.ts";
import { confirmed, proceed, requireTty, typeName } from "../confirm.ts";
import { has, onlyFlags, pinGroups, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function env(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub === "show") return show(ctx, rest);
  if (sub === "name") return name(ctx, rest);
  if (rest.length > 0) usage(`env ${sub ?? ""} takes no further arguments`);
  if (sub === "adopt") return adopt(ctx);
  if (sub === "reset") return reset(ctx);
  return usage(sub === undefined ? "env needs show, name, adopt or reset" : `unknown env command: ${sub}`);
}

async function show(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "env show", ["json"]);
  if (rest.length > 0) usage("env show takes --json only");
  const e = await withConnection(ctx, async (c) => valueOf<Environment>(await c.call("env.describe", {})));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(e));
    return 0;
  }
  const o = ctx.output;
  o.out(`Environment "${safe(e.name)}": ${safe(e.envId)}`);
  o.out(`State: ${e.state === "moved" ? "moved or copied: devices wait until you run env adopt (it was moved) or env reset (it is a copy)" : "ok"}`);
  o.out(`Identity pin: ${pinGroups(e.spki)}`);
  o.out(`Protocol: ${e.protocol.min}–${e.protocol.max}`);
  for (const r of e.routes) o.out(`Route (${r.kind}): ${safe(r.url)}`);
  return 0;
}

async function name(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "env name", []);
  const [n, ...more] = rest;
  if (n === undefined || more.length > 0) usage("env name needs one name (quote it if it has spaces)");
  requireTty(ctx.prompt, "env name");
  const e = await withConnection(ctx, (c) => confirmed<Environment>(c, ctx.prompt, `This names the environment "${safe(n)}": devices show it when they pair.`, proceed, "env.name", { name: n }));
  ctx.output.out(`The environment is now "${safe(e.name)}".`);
  return 0;
}

async function adopt(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "env adopt", []);
  requireTty(ctx.prompt, "env adopt");
  await withConnection(ctx, (c) =>
    confirmed(
      c,
      ctx.prompt,
      "This says the server data was moved here, not copied: paired devices are let in again. If the old copy still runs, run env reset instead.",
      proceed,
      "env.adopt",
      {},
    ),
  );
  ctx.output.out("Adopted: devices are let in again.");
  return 0;
}

async function reset(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "env reset", []);
  requireTty(ctx.prompt, "env reset");
  const r = await withConnection(ctx, async (c) => {
    const e = valueOf<Environment>(await c.call("env.describe", {}));
    return confirmed<{ envId: string; devices: number; credentials: number }>(
      c,
      ctx.prompt,
      `This makes "${safe(e.name)}" a new environment with a new identity: every paired device is deleted and must pair again, and every agent and MCP credential is revoked.`,
      typeName("environment's name", e.name),
      "env.reset",
      {},
    );
  });
  ctx.output.out(`New environment ${safe(r.envId)}: deleted ${r.devices} device(s), revoked ${r.credentials} credential(s).`);
  return 0;
}
