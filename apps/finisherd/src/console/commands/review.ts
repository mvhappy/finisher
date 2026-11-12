// `finisherd review policy show|set` (5E plan Decision 25, spec §8.5): a project's branch policy,
// the owner's. `show` reads it; `set` is human-only: a terminal and `Proceed? [y/N]` (the
// console's model since the owner decision of 2026-11-08), then the console sends its own
// challenge code. `--auto-merge on` is refused with `AUTO_MERGE_UNAVAILABLE` before anything is
// asked (the server says the same). Protected branches are shown by id.
import type { BranchPolicy } from "@finisher/contracts";
import { projectArg } from "../../agent/console.ts";
import { AUTO_MERGE_AVAILABLE, autoMergeUnavailable } from "../../reviews/policy.ts";
import { consoleError, valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { all, has, one, onlyFlags, usage } from "../format.ts";
import { safe } from "../safe.ts";
import { type CommandContext, withConnection } from "./context.ts";

const onOff = (flag: string, value: string | undefined): boolean | undefined => {
  if (value === undefined) return undefined;
  if (value !== "on" && value !== "off") usage(`--${flag} is on or off`);
  return value === "on";
};

const words = (p: BranchPolicy): string =>
  [
    `protected branches: ${p.protected.length === 0 ? "none" : p.protected.map(safe).join(", ")}`,
    `require review: ${p.requireReview ? "on" : "off"}`,
    `auto merge: ${p.autoMerge.enabled ? "on" : "off"}`,
  ].join("\n  ");

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
  const r = await withConnection(ctx, async (c) => valueOf<{ policy: BranchPolicy }>(await c.call("review.policy.get", { projectId })));
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Review policy of project ${projectId}:\n  ${words(r.policy)}`);
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
  const projectId = await projectOf(ctx);
  requireTty(ctx.prompt, "review policy set");
  const r = await withConnection(ctx, async (c) => {
    const { policy: current } = valueOf<{ policy: BranchPolicy }>(await c.call("review.policy.get", { projectId }));
    const payload: Record<string, unknown> = { projectId };
    if (protect.length + unprotect.length > 0) {
      payload.protected = [...new Set([...current.protected, ...protect])].filter((b) => !unprotect.includes(b));
    }
    if (requireReview !== undefined) payload.requireReview = requireReview;
    if (autoMerge !== undefined) payload.autoMerge = { enabled: autoMerge };
    const change = [
      ...(protect.length === 0 ? [] : [`protects ${protect.map(safe).join(", ")}`]),
      ...(unprotect.length === 0 ? [] : [`stops protecting ${unprotect.map(safe).join(", ")}`]),
      ...(requireReview === undefined ? [] : [`turns "require review" ${requireReview ? "on" : "off"}`]),
      ...(autoMerge === undefined ? [] : [`turns auto merge ${autoMerge ? "on" : "off"}`]),
    ];
    return confirmed<{ policy: BranchPolicy; old: BranchPolicy }>(
      c,
      ctx.prompt,
      `This ${change.join(" and ")} for project ${projectId}. A protected branch takes writes only from a reviewer. It is audited.`,
      proceed,
      "review.policy",
      payload,
    );
  });
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Review policy of project ${projectId}:\n  ${words(r.policy)}`);
  return 0;
}
