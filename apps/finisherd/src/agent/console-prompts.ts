// `finisherd agent run <name> [--project <dir|id>] [--param k=v]...` and `finisherd agent prompts`
// (sub-project 7B Task 10, spec §6.4): saved prompts from the console. Running one is
// `agent.run` and needs no terminal; trusting a project's prompt files is `project trust`
// (`console-rules.ts`), which lists them first. `fin` has no command for either.
import type { ListedSavedPrompt, Run, Session } from "@finisher/contracts";
import { type ConsoleConnection, valueOf } from "../console/client.ts";
import { type CommandContext, withConnection } from "../console/commands/context.ts";
import { all, has, one, onlyFlags, usage } from "../console/format.ts";
import { safe } from "../console/safe.ts";
import { projectArg, sessionLine } from "./console.ts";

const call = async <A>(c: ConsoleConnection, tag: string, payload: unknown) => valueOf<A>(await c.call(tag, payload));

/** A prompt as a line: name, source, role/mode, parameters. */
export function promptLine(p: ListedSavedPrompt): string {
  const params = p.params.map((q) => `${q.name}:${q.type === "enum" ? (q.values ?? []).join("|") : q.type}${q.default === undefined ? "" : `=${q.default}`}`).join(" ");
  return `${p.name}  ${p.source}  ${p.role}/${p.mode}  ${safe(p.title)}${params === "" ? "" : `  [${safe(params)}]`}`;
}

/** `--param k=v` pairs, values as text (the server types them). */
function paramsOf(flags: ReadonlyArray<string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of flags) {
    const i = f.indexOf("=");
    if (i <= 0) usage(`--param needs name=value, not ${f}`);
    out[f.slice(0, i)] = f.slice(i + 1);
  }
  return out;
}

export async function agentRun(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent run", ["project", "param", "harness", "provider", "model", "title", "json"]);
  const [name, ...more] = rest;
  if (name === undefined || more.length > 0) usage("agent run needs one saved prompt name (agent prompts lists them)");
  const project = one(ctx.args, "project");
  if (project === undefined) usage("agent run needs --project <dir|id>");
  const provider = one(ctx.args, "provider");
  if (provider !== undefined && provider !== "openrouter" && provider !== "local") usage("--provider is openrouter or local");
  const opt = (flag: string, key: string) => (one(ctx.args, flag) === undefined ? {} : { [key]: one(ctx.args, flag) });
  const params = paramsOf(all(ctx.args, "param"));
  const payload = { projectId: await projectArg(project), name, ...(Object.keys(params).length === 0 ? {} : { params }), ...opt("harness", "harness"), ...opt("provider", "provider"), ...opt("model", "model"), ...opt("title", "title") };
  const r = await withConnection(ctx, (c) => call<{ session: Session; run?: Run }>(c, "agent.savedPrompt.run", payload));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else {
    ctx.output.out(`Started agent session ${r.session.id}: ${sessionLine(r.session)}`);
    if (r.run !== undefined) ctx.output.out(`Run ${r.run.id} started. Follow it with: finisherd agent watch ${r.session.id}`);
  }
  return 0;
}

export async function agentPrompts(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent prompts", ["project", "json"]);
  if (rest.length > 0) usage("agent prompts takes --project and --json only");
  const project = one(ctx.args, "project");
  const payload = project === undefined ? {} : { projectId: await projectArg(project) };
  const r = await withConnection(ctx, (c) => call<{ prompts: ListedSavedPrompt[]; ignored?: ListedSavedPrompt[]; invalid?: Array<{ source: string; reason: string }>; projectPrompts?: string }>(c, "agent.savedPrompt.list", payload));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(r));
    return 0;
  }
  for (const p of r.prompts) ctx.output.out(promptLine(p));
  for (const p of r.ignored ?? []) ctx.output.out(`${promptLine(p)}  [ignored: project's prompts are ${r.projectPrompts ?? "untrusted"}]`);
  for (const b of r.invalid ?? []) ctx.output.err(`finisherd: ${safe(b.source)} does not load: ${safe(b.reason)}`);
  if (r.projectPrompts !== undefined) ctx.output.out(`project prompts (prompts/*.json): ${r.projectPrompts}`);
  return 0;
}
