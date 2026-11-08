// `finisherd diagnostics …` (spec §4.7, plan 5A Decisions 28 and 31): the owner's diagnostics
// commands. Consent changes, purges and `project default` change things: each needs a terminal
// and a typed code. `export --content` only reads (no code). Training cannot be turned on here:
// `CONSENT_UI_ONLY` (exit 2) before anything connects, as the server would answer too.
import { consentUiOnly } from "../../chain/fixes.ts";
import { CONSOLE } from "../../principal.ts";
import { ConsoleError, valueOf } from "../client.ts";
import { confirmed, requireTty } from "../confirm.ts";
import { all, has, one, onlyFlags, parseCount, parseDate, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

const VALUES = ["on", "off", "unset"] as const;
type Value = (typeof VALUES)[number];

const SAYS: Record<"diagnostics" | "training", Record<Value, string>> = {
  diagnostics: {
    on: "This turns diagnostics recording on for this server.",
    off: "This turns diagnostics recording off for this server.",
    unset: "This clears your diagnostics choice: the server's default applies again.",
  },
  training: {
    on: "",
    off: "This turns training data off for this server.",
    unset: "This clears your training choice: the app asks again.",
  },
};

export async function diagnostics(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  switch (sub) {
    case "consent": {
      const [purpose, value, ...more] = rest;
      if ((purpose !== "diagnostics" && purpose !== "training") || more.length > 0) usage("diagnostics consent takes diagnostics or training, then on, off or unset");
      return consent(ctx, purpose, value);
    }
    case "on":
    case "off":
    case "unset":
      if (rest.length > 0) usage(`diagnostics ${sub} takes no arguments`);
      return consent(ctx, "diagnostics", sub);
    case "purge":
      return purge(ctx, rest);
    case "project":
      return project(ctx, rest);
    case "export":
      return exportContent(ctx, rest);
    default:
      return usage(sub === undefined ? "diagnostics needs a command" : `unknown diagnostics command: ${sub}`);
  }
}

async function consent(ctx: CommandContext, purpose: "diagnostics" | "training", value: string | undefined): Promise<number> {
  onlyFlags(ctx.args, "diagnostics consent", []);
  if (!(VALUES as ReadonlyArray<string>).includes(value ?? "")) usage("the consent value is on, off or unset");
  const v = value as Value;
  // Never offered here (Decision 31): the app's onboarding question, or a hand edit.
  if (purpose === "training" && v === "on") throw new ConsoleError(consentUiOnly("diagnostics.consent", CONSOLE));
  requireTty(ctx.prompt, `diagnostics consent ${purpose} ${v}`);
  await withConnection(ctx, (c) => confirmed(c, ctx.prompt, SAYS[purpose][v], "diagnostics.consent", { purpose, value: v }));
  ctx.output.out(`Consent for ${purpose} is now ${v === "unset" ? "unset" : v}.`);
  return 0;
}

async function purge(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "diagnostics purge", ["all", "project", "before"]);
  if (rest.length > 0) usage("diagnostics purge takes --all, --project <id> or --before <date>");
  const project = one(ctx.args, "project");
  const before = one(ctx.args, "before");
  const chosen = [has(ctx.args, "all"), project !== undefined, before !== undefined].filter(Boolean).length;
  if (chosen !== 1) usage("diagnostics purge takes exactly one of --all, --project <id> or --before <date>");
  const selector = project !== undefined ? { project } : before !== undefined ? { before: parseDate("before", before) } : "all";
  const what =
    selector === "all"
      ? "This removes every recorded diagnostics entry on this server."
      : "project" in selector
        ? `This removes the diagnostics entries of project ${selector.project}.`
        : `This removes the diagnostics entries from before ${new Date(selector.before).toISOString()}.`;
  requireTty(ctx.prompt, "diagnostics purge");
  const report = await withConnection(ctx, (c) => confirmed<{ removed?: number }>(c, ctx.prompt, what, "diagnostics.purge", { selector }));
  ctx.output.out(`Removed ${report.removed ?? 0} diagnostics entries.`);
  return 0;
}

async function project(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "diagnostics project", []);
  const [what, projectId, ...more] = rest;
  if (what !== "default" || projectId === undefined || more.length > 0) usage("diagnostics project takes default and a project id");
  requireTty(ctx.prompt, "diagnostics project default");
  await withConnection(ctx, (c) =>
    confirmed(c, ctx.prompt, `This lets project ${projectId} be recorded again, for everyone who works on it.`, "diagnostics.project", {
      projectId,
      collect: "default",
    }),
  );
  ctx.output.out(`Project ${projectId} follows the server's diagnostics choice again.`);
  return 0;
}

/** `export --content [--kind <k>]… [--since <date>] [--project <id>] [--limit <n>]`: JSONL on stdout. */
async function exportContent(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "diagnostics export", ["content", "kind", "since", "project", "limit"]);
  if (rest.length > 0) usage("diagnostics export takes only flags");
  if (!has(ctx.args, "content")) usage("diagnostics export here is for content: add --content", "Without content, the fin command exports diagnostics too.");
  const since = one(ctx.args, "since");
  const project = one(ctx.args, "project");
  const limit = one(ctx.args, "limit");
  const kinds = all(ctx.args, "kind");
  const filter = {
    ...(since === undefined ? {} : { since: parseDate("since", since) }),
    ...(project === undefined ? {} : { project }),
    ...(kinds.length === 0 ? {} : { kinds }),
    ...(limit === undefined ? {} : { limit: parseCount("limit", limit) }),
  };
  await withConnection(ctx, async (c) => {
    let cursor: string | undefined;
    do {
      const page = valueOf<{ lines: unknown[]; next?: string }>(
        await c.call("diagnostics.export", { filter, content: true, ...(cursor === undefined ? {} : { cursor }) }),
      );
      for (const line of page.lines) ctx.output.out(JSON.stringify(line));
      cursor = page.next;
    } while (cursor !== undefined);
  });
  return 0;
}
