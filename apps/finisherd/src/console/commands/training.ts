// `finisherd training closed-targets [pendingCounsel|cleared|excluded]` (plan 7A Decision 28;
// sub-project 7 §12.7, Owner decision 1): the counsel gate on closed-model outputs as training
// targets. No argument shows the state (default `pendingCounsel`); recording another is a
// person's, with a terminal and a `y` (owner decision of 2026-11-08), and audited. Nothing in sub-project 7 drops data
// by it: the dataset builder and any upload path must honour it.
import type { ClosedModelTargets } from "@finisher/contracts";
import { valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { has, onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

const STATES: ReadonlyArray<ClosedModelTargets> = ["pendingCounsel", "cleared", "excluded"];

const MEANING: Record<ClosedModelTargets, string> = {
  pendingCounsel: "closed-model outputs may not be training targets until counsel has reviewed it",
  cleared: "counsel cleared closed-model outputs as training targets",
  excluded: "closed-model outputs are never training targets",
};

export async function training(ctx: CommandContext): Promise<number> {
  const [sub, state, ...more] = ctx.args.words;
  if (sub !== "closed-targets") return usage(sub === undefined ? "training needs a command: closed-targets" : `unknown training command: ${sub}`);
  onlyFlags(ctx.args, "training closed-targets", ["json"]);
  if (more.length > 0) usage("training closed-targets takes at most one state");
  if (state === undefined) {
    const r = await withConnection(ctx, async (c) => valueOf<{ state: ClosedModelTargets }>(await c.call("training.closedTargets.get", {})));
    ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `${r.state}: ${MEANING[r.state]}.`);
    return 0;
  }
  if (!(STATES as ReadonlyArray<string>).includes(state)) usage(`the state is one of ${STATES.join(", ")}`);
  const next = state as ClosedModelTargets;
  requireTty(ctx.prompt, "training closed-targets");
  const r = await withConnection(ctx, (c) =>
    confirmed<{ state: ClosedModelTargets }>(c, ctx.prompt, `This records the counsel gate as ${next}: ${MEANING[next]}. It is audited.`, proceed, "training.closedTargets.set", { state: next }),
  );
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify(r) : `Recorded: ${r.state}.`);
  return 0;
}
