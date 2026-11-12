// `finisherd review policy show|set` (5E plan Decision 25, spec §8.5): a project's branch policy,
// the owner's. `show` reads it; `set` is human-only: a terminal and `Proceed? [y/N]` (the
// console's model since the owner decision of 2026-11-08), then the console sends its own
// challenge code. `--auto-merge on` is refused with `AUTO_MERGE_UNAVAILABLE` before anything is
// asked (the server says the same). Protected branches are shown by id.
import type { BranchPolicy } from "@finisher/contracts";
import { projectArg } from "../../agent/console.ts";
import { AUTO_MERGE_AVAILABLE, autoMergeUnavailable } from "../../reviews/policy.ts";
import { type ConsoleConnection, consoleError, valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { all, has, one, onlyFlags, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

const onOff = (flag: string, value: string | undefined): boolean | undefined => {
  if (value === undefined) return undefined;
  if (value !== "on" && value !== "off") usage(`--${flag} is on or off`);
  return value === "on";
};

interface BranchRow {
  readonly id: string;
  readonly name: string;
}

/** `name (id)`, or the id alone for a branch the list does not hold. */
const label = (rows: ReadonlyArray<BranchRow>, id: string): string => {
  const b = rows.find((r) => r.id === id);
  return safe(b === undefined || b.name === id ? id : `${b.name} (${id})`);
};

const words = (p: BranchPolicy, rows: ReadonlyArray<BranchRow>): string =>
  [
    `protected branches: ${p.protected.length === 0 ? "none" : p.protected.map((id) => label(rows, id)).join(", ")}`,
    `require review: ${p.requireReview ? "on" : "off"}`,
    `auto merge: ${p.autoMerge.enabled ? "on" : "off"}${p.autoMerge.enabled ? ` (into ${label(rows, p.autoMerge.into ?? "main")}${p.autoMerge.roles === undefined ? "" : `; roles: ${p.autoMerge.roles.join(", ") || "none"}`})` : ""}`,
  ].join("\n  ");

/** Every branch of the project, archived ones too (a protected branch may have been archived). */
async function branchRows(c: ConsoleConnection, projectId: string): Promise<ReadonlyArray<BranchRow>> {
  return valueOf<{ branches: ReadonlyArray<BranchRow> }>(await c.call("branch.list", { projectId, all: true })).branches;
}

/** A branch reference (id, name or `name#id`) as its id; unknown is `BRANCH_NOT_FOUND`, before anything is asked. */
function resolveRef(rows: ReadonlyArray<BranchRow>, ref: string): string {
  const hit = rows.find((r) => r.id === ref) ?? rows.find((r) => r.name === ref) ?? rows.find((r) => `${r.name}#${r.id}` === ref);
  if (hit === undefined) throw consoleError("BRANCH_NOT_FOUND", `no branch ${safe(ref)} in this project`, { branch: ref }, "Name a branch by its id, its name or name#id.");
  return hit.id;
}

export async function review(ctx: CommandContext): Promise<number> {
  const [sub, action, ...more] = ctx.args.words;
  if (sub !== "policy") return usage(sub === undefined ? "review needs a command: policy show|set" : `unknown review command: ${sub}`);
  if (more.length > 0) usage("review policy takes no more words");
  if (action === "show") return show(ctx);
  if (action === "set") return set(ctx);
  return usage(action === undefined ? "review policy needs show or set" : `unknown review policy command: ${action}`);
}

async function projectOf(ctx: CommandContext): Promise<string> {
  const p = one(ctx.args, "project");
  if (p === undefined) usage("review policy needs --project <dir|id>");
  return projectArg(p);
}

async function show(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "review policy show", ["project", "json"]);
  const projectId = await projectOf(ctx);
  const { r, rows } = await withConnection(ctx, async (c) => ({
    r: valueOf<{ policy: BranchPolicy }>(await c.call("review.policy.get", { projectId })),
    rows: await branchRows(c, projectId),
  }));
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Review policy of project ${projectId}:\n  ${words(r.policy, rows)}`);
  return 0;
}

async function set(ctx: CommandContext): Promise<number> {
  onlyFlags(ctx.args, "review policy set", ["project", "protect", "unprotect", "require-review", "auto-merge", "json"]);
  const protect = all(ctx.args, "protect");
  const unprotect = all(ctx.args, "unprotect");
  const requireReview = onOff("require-review", one(ctx.args, "require-review"));
  const autoMerge = onOff("auto-merge", one(ctx.args, "auto-merge"));
  if (protect.length + unprotect.length === 0 && requireReview === undefined && autoMerge === undefined) {
    usage("review policy set needs --protect, --unprotect, --require-review or --auto-merge");
  }
  // Before the project is even looked up, and before anything is asked.
  if (autoMerge === true && !AUTO_MERGE_AVAILABLE) {
    const e = autoMergeUnavailable();
    throw consoleError(e.code, e.message, e.details as Record<string, unknown>, e.fix);
  }
  const both = protect.filter((b) => unprotect.includes(b));
  if (both.length > 0) usage(`${both.map(safe).join(", ")} is in both --protect and --unprotect`, "Name a branch in one of them.");
  const projectId = await projectOf(ctx);
  requireTty(ctx.prompt, "review policy set");
  const { r, rows } = await withConnection(ctx, async (c) => {
    // Resolved first: an unknown branch fails before anything is asked. The server applies the
    // branches to the list as it is when the write lands (no read-modify-write here).
    const rows = await branchRows(c, projectId);
    const add = protect.map((b) => resolveRef(rows, b));
    const remove = unprotect.map((b) => resolveRef(rows, b));
    if (add.some((id) => remove.includes(id))) usage("a branch is in both --protect and --unprotect", "Name it in one of them.");
    const payload: Record<string, unknown> = { projectId };
    if (add.length > 0) payload.protect = add;
    if (remove.length > 0) payload.unprotect = remove;
    if (requireReview !== undefined) payload.requireReview = requireReview;
    if (autoMerge !== undefined) payload.autoMerge = { enabled: autoMerge };
    const change = [
      ...(add.length === 0 ? [] : [`protects ${add.map((id) => label(rows, id)).join(", ")}`]),
      ...(remove.length === 0 ? [] : [`stops protecting ${remove.map((id) => label(rows, id)).join(", ")}`]),
      ...(requireReview === undefined ? [] : [`turns "require review" ${requireReview ? "on" : "off"}`]),
      ...(autoMerge === undefined ? [] : [`turns auto merge ${autoMerge ? "on" : "off"}`]),
    ];
    const r = await confirmed<{ policy: BranchPolicy; old: BranchPolicy }>(
      c,
      ctx.prompt,
      `This ${change.join(" and ")} for project ${projectId}. A protected branch takes writes only from a reviewer. It is audited.`,
      proceed,
      "review.policy",
      payload,
    );
    return { r, rows };
  });
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Review policy of project ${projectId}:\n  ${words(r.policy, rows)}`);
  return 0;
}
