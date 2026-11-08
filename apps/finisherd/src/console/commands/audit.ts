// `finisherd audit [--since <date>] [--kind <kind>] [--limit <n>] [--json]` (read-only: runs
// without a terminal) and `finisherd audit purge (--all | --before <date>)` (needs a terminal, and
// the word `audit` typed back: a purge cannot be undone).
import { AUDIT_KINDS, AUDIT_PAGE_MAX, type AuditEntry } from "@finisher/contracts";
import { valueOf } from "../client.ts";
import { confirmed, requireTty, typeName } from "../confirm.ts";
import { auditLine, has, one, onlyFlags, parseCount, parseDate, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function audit(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub === "purge") return purge(ctx, rest);
  if (sub !== undefined) usage(`unknown audit command: ${sub}`);
  onlyFlags(ctx.args, "audit", ["since", "kind", "limit", "json"]);
  const since = one(ctx.args, "since");
  const kind = one(ctx.args, "kind");
  if (kind !== undefined && !(AUDIT_KINDS as ReadonlyArray<string>).includes(kind)) usage(`--kind must be one of: ${AUDIT_KINDS.join(", ")}`);
  const limitText = one(ctx.args, "limit");
  const limit = limitText === undefined ? 100 : parseCount("limit", limitText);
  const query = {
    ...(since === undefined ? {} : { since: parseDate("since", since) }),
    ...(kind === undefined ? {} : { kind }),
  };
  // Newest first, page by page, up to `limit`.
  const entries = await withConnection(ctx, async (c) => {
    const got: AuditEntry[] = [];
    let cursor: string | undefined;
    do {
      const page = valueOf<{ entries: AuditEntry[]; next?: string }>(
        await c.call("auth.audit.list", { ...query, limit: Math.min(AUDIT_PAGE_MAX, limit - got.length), ...(cursor === undefined ? {} : { cursor }) }),
      );
      got.push(...page.entries);
      cursor = page.next;
    } while (cursor !== undefined && got.length < limit);
    return got;
  });
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ entries }));
  else if (entries.length === 0) ctx.output.out("No audit entries.");
  else for (const e of entries) ctx.output.out(auditLine(e));
  return 0;
}

async function purge(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "audit purge", ["all", "before"]);
  if (rest.length > 0) usage("audit purge takes --all or --before <date>");
  const all = has(ctx.args, "all");
  const beforeText = one(ctx.args, "before");
  if (all === (beforeText !== undefined)) usage("audit purge takes exactly one of --all or --before <date>");
  const before = beforeText === undefined ? undefined : parseDate("before", beforeText);
  requireTty(ctx.prompt, "audit purge");
  const what = all
    ? "This removes every entry of the server's security audit log."
    : `This removes the audit log entries from before ${new Date(before!).toISOString()}.`;
  const { removed } = await withConnection(ctx, (c) =>
    confirmed<{ removed: number }>(c, ctx.prompt, what, typeName("word", "audit"), "auth.audit.purge", all ? { all: true } : { before }),
  );
  ctx.output.out(`Removed ${removed} audit entries.`);
  return 0;
}
