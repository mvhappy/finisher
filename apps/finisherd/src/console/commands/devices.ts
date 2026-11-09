// `finisherd devices [--all] [--json]` (read-only: runs without a terminal) and `finisherd
// revoke <id>` (plan 5B Task 12): a revocation ends the device's connections and tokens at once
// and cannot be undone (it pairs again), so the device's name is typed back (owner decision of
// 2026-11-08).
import type { Device } from "@finisher/contracts";
import { valueOf } from "../client.ts";
import { confirmed, requireTty, typeName } from "../confirm.ts";
import { has, onlyFlags, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

const when = (ms: number | undefined) => (ms === undefined ? "-" : new Date(ms).toISOString());

/** One device as a line for a person. */
export function deviceLine(d: Device): string {
  const parts = [safe(d.id), JSON.stringify(safe(d.name)), safe(d.platform), `paired ${when(d.createdAt)}`, `seen ${when(d.lastSeenAt)}${d.lastRoute === undefined ? "" : ` (${d.lastRoute})`}`];
  if (d.revokedAt !== undefined) parts.push(`revoked ${when(d.revokedAt)}`);
  return parts.join("  ");
}

export async function devices(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "devices", ["all", "json"]);
  if (ctx.args.words.length > 0) usage("devices takes --all and --json only");
  const all = has(ctx.args, "all");
  const list = await withConnection(ctx, async (c) => valueOf<{ devices: Device[] }>(await c.call("auth.device.list", all ? { all: true } : {})).devices);
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ devices: list }));
  else if (list.length === 0) ctx.output.out(all ? "No devices." : "No paired devices.");
  else for (const d of list) ctx.output.out(deviceLine(d));
  return 0;
}

export async function revoke(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "revoke", []);
  const [id, ...more] = ctx.args.words;
  if (id === undefined || more.length > 0) usage("revoke needs one device id (devices lists them)");
  requireTty(ctx.prompt, "revoke");
  const { revoked, name } = await withConnection(ctx, async (c) => {
    const found = valueOf<{ devices: Device[] }>(await c.call("auth.device.list", {})).devices.find((d) => d.id === id);
    if (found === undefined) return { revoked: false, name: "" };
    const r = await confirmed<{ revoked: boolean }>(
      c,
      ctx.prompt,
      `This revokes the device "${safe(found.name)}" (${safe(found.platform)}): its connections end now, its tokens stop working, and it must pair again.`,
      typeName("device's name", found.name),
      "auth.device.revoke",
      { id },
    );
    return { revoked: r.revoked, name: found.name };
  });
  ctx.output.out(revoked ? `Revoked "${safe(name)}" (${id}).` : `No live device ${id}: nothing changed.`);
  return 0;
}
