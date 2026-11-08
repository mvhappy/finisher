// `finisherd project takeover <dir|projectId> --confirm` and `finisherd project open <dir>
// --relocate` (spec §4.6): a person's decisions about a project's lease and where it lives.
// Both need a terminal and a typed code.
import { isAbsolute, resolve } from "node:path";
import { valueOf } from "../client.ts";
import { confirmed, requireTty } from "../confirm.ts";
import { has, onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

/** A folder rather than a project id: a path (with a separator, or `.`/`..`). */
const isPath = (s: string) => isAbsolute(s) || s.includes("/") || s.includes("\\") || s === "." || s === "..";

export async function project(ctx: CommandContext): Promise<number> {
  const [sub, target, ...more] = ctx.args.words;
  if (target === undefined || more.length > 0) usage(`project ${sub ?? ""} needs one folder or project id`.replace("  ", " "));
  if (sub === "takeover") return takeover(ctx, target);
  if (sub === "open") return open(ctx, target);
  return usage(sub === undefined ? "project needs a command" : `unknown project command: ${sub}`);
}

async function takeover(ctx: CommandContext, target: string): Promise<number> {
  onlyFlags(ctx.args, "project takeover", ["confirm"]);
  if (!has(ctx.args, "confirm")) usage("project takeover here confirms the takeover: add --confirm", "Without --confirm, the fin command takes over a project whose holder has stopped.");
  requireTty(ctx.prompt, "project takeover");
  const status = await withConnection(ctx, async (c) => {
    // A folder is opened (or attached to) first, for its project id.
    const projectId = isPath(target) ? valueOf<{ projectId: string }>(await c.call("project.open", { dir: resolve(target) })).projectId : target;
    return confirmed(
      c,
      ctx.prompt,
      `This makes this server the writer of project ${projectId}, even if another one still holds it. Make sure nothing else is editing it.`,
      "project.takeover",
      { projectId, confirm: true },
    );
  });
  ctx.output.out(JSON.stringify(status));
  return 0;
}

async function open(ctx: CommandContext, target: string): Promise<number> {
  onlyFlags(ctx.args, "project open", ["relocate"]);
  if (!has(ctx.args, "relocate")) usage("project open here confirms a move: add --relocate", "Without --relocate, the fin command opens a project.");
  if (!isPath(target)) usage("project open --relocate needs the project's folder");
  const dir = resolve(target);
  requireTty(ctx.prompt, "project open --relocate");
  const ref = await withConnection(ctx, (c) =>
    confirmed<{ projectId: string }>(
      c,
      ctx.prompt,
      `This tells the server the project now lives at ${dir}, and that the old folder is no longer it.`,
      "project.open",
      { dir, relocate: true },
    ),
  );
  ctx.output.out(`Project ${ref.projectId} opened at ${dir}.`);
  return 0;
}
