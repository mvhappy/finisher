// `finisherd status [--json]` (read-only: runs without a terminal).
import { valueOf } from "../client.ts";
import { has, onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

interface Status {
  version: string;
  protocol: number;
  envId: string;
  pid: number;
  listeners: Array<{ transport: string; endpoint: string }>;
  connections: Record<string, number>;
  journal: { enabled: boolean };
  auditFailures: number;
  jobs?: { caps: { killTree: string; watchdog: boolean; cgroupMemory: boolean; leafLimits: boolean; sandbox: string; sandboxed: boolean } };
  warnings: Array<Record<string, unknown>>;
}

export async function status(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "status", ["json"]);
  if (ctx.args.words.length > 0) usage("status takes no arguments");
  const s = await withConnection(ctx, async (c) => valueOf<Status>(await c.call("server.status", {})));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(s));
    return 0;
  }
  const o = ctx.output;
  o.out(`Finisher server ${s.version} (protocol ${s.protocol}), process ${s.pid}`);
  o.out(`Environment: ${s.envId}`);
  for (const l of s.listeners) o.out(`Listening (${l.transport}): ${l.endpoint}`);
  const conns = Object.entries(s.connections).map(([k, n]) => `${k} ${n}`);
  o.out(`Connections: ${conns.length > 0 ? conns.join(", ") : "none"}`);
  o.out(`Diagnostics: ${s.journal.enabled ? "recording" : "not recording"}`);
  if (s.jobs !== undefined) {
    const c = s.jobs.caps;
    o.out(`Jobs: kill tree ${c.killTree}, watchdog ${c.watchdog ? "on" : "off"}, memory limits ${c.cgroupMemory ? "kernel" : "sampled"}, leaf limits ${c.leafLimits ? "on" : "off"}, ${c.sandboxed ? `sandboxed (${c.sandbox})` : "unsandboxed"}`);
  }
  if (s.auditFailures > 0) o.out(`Audit log: ${s.auditFailures} entries could not be written since start`);
  for (const w of s.warnings) o.out(warningLine(w));
  return 0;
}

/** `Warning: CODE (profile, reason): message Fix: …` (the details a sandbox warning carries). */
export function warningLine(w: Record<string, unknown>): string {
  const d = (w.details ?? {}) as Record<string, unknown>;
  const facts = [d.profile, d.reason ?? d.need].filter((x) => x !== undefined).map(String);
  const head = `Warning: ${String(w.code)}${facts.length > 0 ? ` (${facts.join(", ")})` : ""}`;
  const message = typeof w.message === "string" ? `: ${w.message}` : "";
  const fix = typeof w.fix === "string" ? ` Fix: ${w.fix}` : "";
  return `${head}${message}${fix}`;
}
