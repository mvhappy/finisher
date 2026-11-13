// `finisherd agent rules ls|add|rm` and `finisherd project trust <dir|id>` (sub-project 7B
// Decision 7): the owner's command rules from the console. Listing needs no terminal; adding
// and removing a `local` rule and trusting a project's `finisher.rules.json` are a person's,
// with a terminal and a confirmation (`confirmed`, owner decision of 2026-11-08): `y` to add a
// rule, the rule id's first 8 characters typed back to remove one, the project id's first 8
// characters to trust a project's files (what its rules allow, its agents may do; what its
// hooks run, runs on their events).
import type { ListedHook, ListedRule, ListedSavedPrompt } from "@finisher/contracts";
import { type ConsoleConnection, consoleError, valueOf } from "../console/client.ts";
import { type CommandContext, withConnection } from "../console/commands/context.ts";
import { confirmed, proceed, requireTty, typeName } from "../console/confirm.ts";
import { has, one, onlyFlags, usage } from "../console/format.ts";
import { safe } from "../console/safe.ts";
import { HOOKS_FILE } from "../hooks/config.ts";
import { projectArg } from "./console.ts";
import { promptLine } from "./console-prompts.ts";

const call = async <A>(c: ConsoleConnection, tag: string, payload: unknown) => valueOf<A>(await c.call(tag, payload));

export function ruleLine(r: ListedRule): string {
  const where = r.where === undefined ? "" : `  where ${safe(JSON.stringify(r.where))}`;
  const scope = r.sessionId !== undefined ? `  session ${r.sessionId}` : r.projectId !== undefined ? `  project ${r.projectId}` : "";
  return `${r.id}  ${r.layer}  ${r.effect} ${safe(r.command)}${where}${scope}  (${safe(r.source)})${r.applies ? "" : "  [ignored: project not trusted]"}${r.remembered === true ? "  [remembered: lifts a matching ask]" : ""}`;
}

/** A project hook as `project trust` shows it: id, event, what it matches and the program it runs. */
export function hookLine(h: ListedHook): string {
  const match = h.match === undefined ? "" : `  match ${safe(h.match)}`;
  return `${safe(h.id)}  ${h.event}${match}  run ${safe(JSON.stringify(h.run ?? []))}  (${safe(h.source)})${h.applies ? "" : "  [not trusted]"}`;
}

export async function agentRules(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  const [sub, ...more] = rest;
  if (sub === "ls" || sub === "list") return list(ctx, more);
  if (sub === "add") return add(ctx, more);
  if (sub === "rm") return remove(ctx, more);
  return usage(sub === undefined ? "agent rules needs ls, add or rm" : `unknown agent rules command: ${sub}`);
}

async function list(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent rules ls", ["project", "session", "json"]);
  if (rest.length > 0) usage("agent rules ls takes --project, --session and --json only");
  const project = one(ctx.args, "project");
  const session = one(ctx.args, "session");
  const payload = { ...(project === undefined ? {} : { projectId: await projectArg(project) }), ...(session === undefined ? {} : { sessionId: session }) };
  const r = await withConnection(ctx, (c) => call<{ rules: ListedRule[]; projectRules?: string }>(c, "agent.rules.list", payload));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(r));
    return 0;
  }
  if (r.projectRules !== undefined) ctx.output.out(`project rules (finisher.rules.json): ${r.projectRules}`);
  if (r.rules.length === 0) ctx.output.out("No command rules: every command is allowed within the credential's scopes.");
  for (const rule of r.rules) ctx.output.out(ruleLine(rule));
  return 0;
}

const whole = (flag: string, v: string | undefined) => {
  if (v === undefined) return undefined;
  if (!/^\d+$/.test(v)) usage(`--${flag} needs a whole number`);
  return Number(v);
};

async function add(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent rules add", ["project", "session", "branch", "mode", "preset", "bytes-over", "minutes-over", "json"]);
  const [effect, command, ...more] = rest;
  if ((effect !== "allow" && effect !== "ask" && effect !== "deny") || command === undefined || more.length > 0) {
    usage("agent rules add needs allow, ask or deny and one command pattern", "For example: finisherd agent rules add deny 'plugin.*' --project <dir|id>");
  }
  const mode = one(ctx.args, "mode");
  if (mode !== undefined && !["plan", "draft", "auto"].includes(mode)) usage("--mode is plan, draft or auto");
  const opt = (flag: string, key: string, v: unknown = one(ctx.args, flag)) => (v === undefined ? {} : { [key]: v });
  const where = {
    ...opt("branch", "branch"),
    ...opt("mode", "mode"),
    ...opt("preset", "preset"),
    ...opt("bytes-over", "bytesOver", whole("bytes-over", one(ctx.args, "bytes-over"))),
    ...opt("minutes-over", "minutesOver", whole("minutes-over", one(ctx.args, "minutes-over"))),
  };
  const project = one(ctx.args, "project");
  const session = one(ctx.args, "session");
  const payload = {
    effect,
    command,
    ...(Object.keys(where).length === 0 ? {} : { where }),
    ...(project === undefined ? {} : { projectId: await projectArg(project) }),
    ...(session === undefined ? {} : { sessionId: session }),
  };
  requireTty(ctx.prompt, "agent rules add");
  const scope = session !== undefined ? `session ${session}` : project !== undefined ? `project ${payload.projectId}` : "every project";
  const { rule } = await withConnection(ctx, (c) =>
    confirmed<{ rule: ListedRule }>(c, ctx.prompt, `This adds a local rule: ${effect} ${command} for agent sessions of ${scope}.`, proceed, "agent.rules.add", payload),
  );
  ctx.output.out(has(ctx.args, "json") ? JSON.stringify({ rule }) : `Added ${ruleLine(rule)}`);
  return 0;
}

async function remove(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent rules rm", []);
  const [id, ...more] = rest;
  if (id === undefined || more.length > 0) usage("agent rules rm needs one rule id (agent rules ls lists them)");
  requireTty(ctx.prompt, "agent rules rm");
  const { removed } = await withConnection(ctx, (c) => confirmed<{ removed: boolean }>(c, ctx.prompt, `This removes local rule ${id}.`, typeName("rule id's first 8 characters", id.slice(0, 8)), "agent.rules.remove", { id }));
  ctx.output.out(removed ? `Removed rule ${id}.` : `No local rule ${id}: nothing changed (rules of the other layers live in their files).`);
  return 0;
}

/** `finisherd project trust <dir|id>`: the project's rules file applies from now, as it is now. */
export async function projectTrust(ctx: CommandContext, target: string): Promise<number> {
  onlyFlags(ctx.args, "project trust", []);
  const projectId = await projectArg(target);
  requireTty(ctx.prompt, "project trust");
  const r = await withConnection(ctx, async (c) => {
    const { rules, rulesHash } = await call<{ rules: ListedRule[]; rulesHash?: string }>(c, "agent.rules.list", { projectId });
    if (rulesHash === undefined) {
      throw consoleError("INVALID_REQUEST", `project ${projectId}'s finisher.rules.json cannot be read`, { projectId, reason: "unreadable" }, "fix or remove it, then trust the project again");
    }
    const file = rules.filter((x) => x.layer === "project");
    for (const rule of file) ctx.prompt.show(ruleLine(rule));
    // The project's saved prompts (prompts/*.json) are trusted as shown too, when every file loads.
    const saved = await call<{ prompts: ListedSavedPrompt[]; ignored?: ListedSavedPrompt[]; invalid?: unknown[]; promptsHash?: string }>(c, "agent.savedPrompt.list", { projectId });
    const own = [...saved.prompts.filter((p) => p.source === "project"), ...(saved.ignored ?? [])];
    for (const p of own) ctx.prompt.show(`${promptLine(p)}\n${p.template.split("\n").map((l) => `    ${safe(l)}`).join("\n")}`);
    const broken = (saved.invalid ?? []).some((b) => (b as { source: string }).source.startsWith("prompts/"));
    const promptsHash = broken ? undefined : saved.promptsHash;
    if (broken) ctx.output.err("finisherd: a prompt file does not load; its prompts stay untrusted (see `agent prompts --project`)");
    // The project's hooks (finisher.hooks.json, 7F): each with its event and the program it runs.
    const hooks = await call<{ hooks: ListedHook[]; hooksHash?: string; projectHooksReason?: string }>(c, "agent.hooks.list", { projectId });
    const ownHooks = hooks.hooks.filter((h) => h.layer === "project");
    for (const h of ownHooks) ctx.prompt.show(hookLine(h));
    const hooksBroken = hooks.hooksHash === undefined || hooks.projectHooksReason === "invalidHooks";
    const hooksHash = hooksBroken ? undefined : hooks.hooksHash;
    if (hooksBroken) ctx.output.err(`finisherd: ${HOOKS_FILE} does not load; its hooks stay untrusted (fix it, then trust the project again)`);
    const alsoPrompts = promptsHash === undefined || own.length === 0 ? "" : ` and its ${own.length} saved prompts (prompts/*.json, above)`;
    const alsoHooks = hooksHash === undefined || ownHooks.length === 0 ? "" : ` and its ${ownHooks.length} hooks (${HOOKS_FILE}, above: each runs its program on its event)`;
    return confirmed<{ rulesHash: string; rules: number; promptsHash?: string; hooksHash?: string }>(
      c,
      ctx.prompt,
      `This trusts project ${projectId}'s finisher.rules.json as it is now (${file.length} rules above)${alsoPrompts}${alsoHooks}: they apply to agent sessions until a file changes.`,
      typeName("project id's first 8 characters", projectId.slice(0, 8)),
      "agent.project.trust",
      // What was shown: a file changed since is refused.
      { projectId, expectedHash: rulesHash, ...(promptsHash === undefined ? {} : { expectedPromptsHash: promptsHash }), ...(hooksHash === undefined ? {} : { expectedHooksHash: hooksHash }) },
    );
  });
  const extra = (label: string, hash: string | undefined) => (hash === undefined || hash === "none" ? "" : `, ${label} ${hash}`);
  ctx.output.out(`Trusted project ${projectId}: ${r.rules} rules (${r.rulesHash})${extra("saved prompts", r.promptsHash)}${extra("hooks", r.hooksHash)}.`);
  return 0;
}
