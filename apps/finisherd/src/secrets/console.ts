// `finisherd provider key set|ls|rm` and `provider local set|show|test|rm` (plan 7A Decision 24;
// sub-project 7 §10): the owner's OpenRouter key and local model endpoint. Every change needs a
// terminal and a confirmation (`agent.admin` is human-only; owner decision of 2026-11-08): `set` a
// `y`; removing a key its id's first 8 characters typed back, removing the endpoint the word `local`. **The key is read from the
// terminal without echo, never from the command line**: `provider key set openrouter <key>` is
// refused before anything is read or sent, so a key never lands in shell history or `ps`.
// Listing names keys by id, label and project only. `provider signin|signout claudeCode` prints
// the CLI's own sign-in commands and runs nothing (plan 7D Decision 23, `agent/claude/detect.ts`);
// `signin --confirm` also lets the next account change through, and `provider account confirm
// claudeCode` takes a changed account (Decision 19, `agent/claude/account.ts`): both y/N.
import { type AgentWarning, FinError, type ProviderKey } from "@finisher/contracts";
import { KEY_RULE, KEY_RULE_TEXT } from "../agent/admin.ts";
import { signinLines } from "../agent/claude/detect.ts";
import { projectArg } from "../agent/console.ts";
import { type ConsoleConnection, consoleError, valueOf } from "../console/client.ts";
import { type CommandContext, withConnection } from "../console/commands/context.ts";
import { confirmed, proceed, requireTty, typeName } from "../console/confirm.ts";
import { has, one, onlyFlags, usage } from "../console/format.ts";
import { safe } from "../console/safe.ts";

const call = async <A>(c: ConsoleConnection, tag: string, payload: unknown) => valueOf<A>(await c.call(tag, payload));

export async function provider(ctx: CommandContext): Promise<number> {
  const [group, sub, ...rest] = ctx.args.words;
  if (group === "key") {
    if (sub === "set") return keySet(ctx, rest);
    if (sub === "ls" || sub === "list") return keyList(ctx, rest);
    if (sub === "rm" || sub === "remove") return keyRemove(ctx, rest);
    return usage("provider key needs a command: set, ls or rm");
  }
  if (group === "local") {
    if (sub === "set") return localSet(ctx, rest);
    if (sub === "show") return localShow(ctx, rest);
    if (sub === "test") return localTest(ctx, rest);
    if (sub === "rm" || sub === "remove") return localRemove(ctx, rest);
    return usage("provider local needs a command: set, show, test or rm");
  }
  if (group === "signin" || group === "signout") return signin(ctx, group, sub, rest);
  if (group === "account") {
    if (sub === "confirm") return accountConfirm(ctx, rest);
    return usage("provider account needs a command: confirm");
  }
  return usage(group === undefined ? "provider needs key, local, account, signin or signout" : `unknown provider command: ${group}`);
}

type ProviderLine = { harness: string; accountHash?: string; problem?: { code: string } };

/** `provider account confirm claudeCode`: the account the CLI is signed in to now becomes the one sessions run on. */
async function accountConfirm(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider account confirm", []);
  if (rest.length !== 1 || rest[0] !== "claudeCode") usage("provider account confirm takes claudeCode");
  requireTty(ctx.prompt, "provider account confirm");
  const r = await withConnection(ctx, async (c) => {
    const { providers } = await call<{ providers: ProviderLine[] }>(c, "agent.providers", {});
    const line = providers.find((p) => p.harness === "claudeCode");
    if (line?.problem?.code !== "PROVIDER_ACCOUNT_CHANGED") return undefined;
    return confirmed<{ confirmed: boolean; accountHash?: string; previous?: string }>(
      c,
      ctx.prompt,
      `Claude Code is signed in to another account (${safe(line.accountHash ?? "unknown")}) than the one this server's sessions ran on. This makes it the account new sessions run on.`,
      proceed,
      "agent.account.confirm",
      { harness: "claudeCode" },
    );
  });
  if (r === undefined || !r.confirmed) ctx.output.out("No Claude Code account change waits for a confirmation.");
  else ctx.output.out(`Claude Code sessions now run on account ${r.accountHash}${r.previous === undefined ? "" : ` (was ${r.previous})`}.`);
  return 0;
}

/**
 * Prints the CLI's sign-in (or sign-out) commands for a person to run; runs nothing, needs no
 * server. `signin --confirm` then asks the server (y/N) to take the next account change as expected.
 */
async function signin(ctx: CommandContext, verb: "signin" | "signout", harness: string | undefined, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, `provider ${verb}`, verb === "signin" ? ["confirm"] : []);
  if (harness !== "claudeCode" || rest.length > 0) usage(`provider ${verb} takes claudeCode`);
  const expect = has(ctx.args, "confirm");
  if (expect) requireTty(ctx.prompt, "provider signin --confirm");
  let lines: string[];
  try {
    lines = signinLines(ctx.serverData, verb === "signin" ? "login" : "logout");
  } catch (e) {
    if (e instanceof FinError) throw consoleError(e.code, e.message, e.details as Record<string, unknown>, e.fix);
    throw e;
  }
  for (const l of lines) ctx.output.out(l);
  if (!expect) return 0;
  await withConnection(ctx, (c) =>
    confirmed(c, ctx.prompt, "This lets the next change of Claude Code's account through without a confirmation: sign in to the new account with the commands above.", proceed, "agent.account.confirm", { harness: "claudeCode", next: true }),
  );
  ctx.output.out("The next Claude Code account is taken as expected.");
  return 0;
}

function warn(ctx: CommandContext, warnings: ReadonlyArray<AgentWarning> | undefined): void {
  for (const w of warnings ?? []) ctx.output.err(`finisherd: warning: ${safe(w.code)}: ${safe(w.message)}`);
}

/** The provider a key is for: v1 stores OpenRouter's only (Owner decision 7). */
function keyProvider(name: string | undefined): "openrouter" {
  if (name === undefined) usage("provider key set needs the provider: openrouter");
  if (name === "local") {
    throw consoleError("PROVIDER_UNAVAILABLE", "a local model server needs no key", { provider: "local" }, "Set its address with: finisherd provider local set <ollama|lmstudio|url>");
  }
  if (name !== "openrouter") {
    throw consoleError("PROVIDER_UNAVAILABLE", `Finisher stores no ${name} key`, { provider: name }, "Only an OpenRouter key can be set; Claude Code and Codex use their own sign-in.");
  }
  return name;
}

async function keySet(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider key set", ["project", "label"]);
  const [name, ...more] = rest;
  const providerName = keyProvider(name);
  if (more.length > 0) {
    usage("provider key set reads the key from the terminal, never from the command line", "Run it without the key; paste the key when it asks (it is not shown).");
  }
  requireTty(ctx.prompt, "provider key set");
  const project = one(ctx.args, "project");
  const projectId = project === undefined ? undefined : await projectArg(project);
  const label = one(ctx.args, "label");
  // Never the echoing `ask`: a pasted key would be shown (and kept in the terminal's scrollback).
  if (ctx.prompt.secret === undefined) {
    throw consoleError("CONSOLE_NEEDS_TTY", "this terminal cannot hide what you type, so the key was not asked for", { reason: "noSecretPrompt" }, "run `finisherd provider key set` in a terminal that supports hidden input");
  }
  const key = (await ctx.prompt.secret("OpenRouter key (not shown): ")).trim();
  if (key === "") throw consoleError("CONFIRMATION_REQUIRED", "no key was given, so nothing was changed", { reason: "cancelled" });
  // The store's own rule, checked before anything is sent (and never quoted back).
  if (!KEY_RULE.test(key)) throw consoleError("SECRET_INVALID", KEY_RULE_TEXT, { field: "key" }, "Copy the key again from OpenRouter's keys page.");
  const saved = await withConnection(ctx, (c) =>
    confirmed<{ key: ProviderKey }>(
      c,
      ctx.prompt,
      `This stores the OpenRouter key ${projectId === undefined ? "as the server's default (for every project without its own)" : `for project ${projectId}`}, replacing the one set there before. Built-in agent sessions spend from it.`,
      proceed,
      "agent.key.set",
      { provider: providerName, key, ...(label === undefined ? {} : { label }), ...(projectId === undefined ? {} : { projectId }) },
    ),
  );
  ctx.output.out(`Stored key ${saved.key.id} ("${safe(saved.key.label)}")${saved.key.projectId === undefined ? " as the server's default" : ` for project ${saved.key.projectId}`}.`);
  return 0;
}

export function keyLine(k: ProviderKey): string {
  return [k.id, k.provider, JSON.stringify(safe(k.label)), k.projectId === undefined ? "server default" : `project ${k.projectId}`, new Date(k.createdAt).toISOString()].join("  ");
}

async function keyList(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider key ls", ["json"]);
  if (rest.length > 0) usage("provider key ls takes --json only");
  const { keys } = await withConnection(ctx, (c) => call<{ keys: ProviderKey[] }>(c, "agent.key.list", {}));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ keys }));
  else if (keys.length === 0) ctx.output.out("No keys. Set one with: finisherd provider key set openrouter");
  else for (const k of keys) ctx.output.out(keyLine(k));
  return 0;
}

async function keyRemove(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider key rm", []);
  const [id, ...more] = rest;
  if (id === undefined || more.length > 0) usage("provider key rm needs one key id (provider key ls lists them)");
  requireTty(ctx.prompt, "provider key rm");
  const { removed } = await withConnection(ctx, async (c) => {
    const { keys } = await call<{ keys: ProviderKey[] }>(c, "agent.key.list", {});
    const found = keys.find((k) => k.id === id);
    if (found === undefined) return { removed: false };
    return confirmed<{ removed: boolean }>(c, ctx.prompt, `This removes the key ${id} ("${safe(found.label)}"): sessions that used it get no model key until another is set.`, typeName("key id's first 8 characters", id.slice(0, 8)), "agent.key.remove", { id });
  });
  ctx.output.out(removed ? `Removed key ${id}.` : `No key ${id}: nothing changed.`);
  return 0;
}

async function localSet(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider local set", []);
  const [endpoint, ...more] = rest;
  if (endpoint === undefined || more.length > 0) usage("provider local set needs one endpoint: ollama, lmstudio or a base URL");
  requireTty(ctx.prompt, "provider local set");
  const r = await withConnection(ctx, (c) =>
    confirmed<{ preset: string; warnings?: AgentWarning[] }>(
      c,
      ctx.prompt,
      `This points the built-in agent's local preset at ${safe(endpoint)}: sessions started with --provider local send their prompts and the project's content there.`,
      proceed,
      "agent.local.set",
      { endpoint },
    ),
  );
  warn(ctx, r.warnings);
  ctx.output.out(`The local endpoint is set (${r.preset}). Check it with: finisherd provider local test`);
  return 0;
}

async function localShow(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider local show", ["json"]);
  if (rest.length > 0) usage("provider local show takes --json only");
  const r = await withConnection(ctx, (c) => call<{ configured: boolean; preset?: string; baseUrl?: string }>(c, "agent.local.show", {}));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else ctx.output.out(r.configured ? `${r.preset}  ${safe(r.baseUrl ?? "")}` : "No local endpoint is set.");
  return 0;
}

async function localTest(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider local test", ["json"]);
  if (rest.length > 0) usage("provider local test takes --json only");
  const r = await withConnection(ctx, (c) =>
    call<{ configured: boolean; reachable: boolean; models?: string[]; problem?: { code: string; message: string; fix?: string } }>(c, "agent.local.test", {}),
  );
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else if (r.reachable) ctx.output.out(`The local model server answers. Models: ${(r.models ?? []).map(safe).join(", ") || "(none listed)"}`);
  if (!r.reachable) throw consoleError(r.problem?.code ?? "PROVIDER_UNAVAILABLE", r.problem?.message ?? "the local model server does not answer", { provider: "local" }, r.problem?.fix);
  return 0;
}

async function localRemove(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "provider local rm", []);
  if (rest.length > 0) usage("provider local rm takes no arguments");
  requireTty(ctx.prompt, "provider local rm");
  const { removed } = await withConnection(ctx, (c) =>
    confirmed<{ removed: boolean }>(c, ctx.prompt, "This removes the local model endpoint: sessions with --provider local cannot start until one is set again.", typeName("word", "local"), "agent.local.remove", {}),
  );
  ctx.output.out(removed ? "Removed the local endpoint." : "No local endpoint was set: nothing changed.");
  return 0;
}
