// `finisherd plugin search|info|ls|install|update|rm|enable|disable` (plan 8A Task 12b; spec §5.3,
// Decisions 19–21): the owner's plugin lifecycle. Reading needs no terminal. Everything that
// changes something needs one, and installs and updates go: `plugin.plan` (nothing runs; the plan
// is staged and checked) → the plan printed with its risks → the owner answers `y` (or, for
// third-party code, types the plugin's id back) → `plugin.install {planId, digest, accept}` with a
// fresh `console.challenge` code the console fetches itself. Removing with `--purge-data` deletes
// the plugin's data, so it takes the plugin's id typed back too. The digest is the one the server printed, so the owner approves exactly
// what was staged.
import type { InstallPlan } from "@finisher/core-types";
import type { InstalledRow, PluginInfoResult, PluginInstallResult, PluginSearchResult } from "@finisher/contracts";
import { ConsoleError, consoleError, valueOf } from "./client.ts";
import { ask, confirmed, proceed, requireTty, typeName, withChallenge } from "./confirm.ts";
import type { CommandContext } from "./commands/context.ts";
import { withConnection } from "./commands/context.ts";
import { has, one, onlyFlags, usage } from "./format.ts";
import { infoLines, installedLine, listingLine, renderPlan, widens } from "./plugin-render.ts";
import { safe } from "./safe.ts";
import type { ConsoleConnection } from "./client.ts";

const ID = /^@[a-z0-9][a-z0-9-]{1,38}\/[a-z0-9][a-z0-9-]{1,63}$/;
const VERSION = /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/;

export async function plugin(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  switch (sub) {
    case "search":
      return search(ctx, rest);
    case "info":
      return info(ctx, rest);
    case "ls":
      return ls(ctx, rest);
    case "install":
      return install(ctx, rest, false);
    case "update":
      return install(ctx, rest, true);
    case "rm":
      return rm(ctx, rest);
    case "enable":
    case "disable":
      return setEnabled(ctx, sub, rest);
    default:
      return usage(sub === undefined ? "plugin needs a command" : `unknown plugin command: ${sub}`);
  }
}

function idOf(text: string | undefined, command: string): string {
  if (text === undefined || !ID.test(text)) usage(`${command} takes a plugin id such as @publisher/name`);
  return text;
}

/** `@publisher/name` or `@publisher/name@1.2.3`. */
function idAndVersion(command: string, rest: ReadonlyArray<string>, versionAllowed: boolean): { id: string; version?: string } {
  const [arg, ...more] = rest;
  if (arg === undefined || more.length > 0) usage(`${command} takes one plugin id`);
  const at = arg.lastIndexOf("@");
  if (at <= 0) return { id: idOf(arg, command) };
  const version = arg.slice(at + 1);
  if (!versionAllowed) usage(`${command} takes a plugin id without a version`);
  if (!VERSION.test(version)) usage(`${version} is not a version such as 1.2.3`);
  return { id: idOf(arg.slice(0, at), command), version };
}

async function search(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "plugin search", ["kind", "registry", "json"]);
  if (rest.length > 1) usage("plugin search takes one search text");
  const kind = one(ctx.args, "kind");
  if (kind !== undefined && kind !== "content" && kind !== "code") usage("--kind is content or code");
  const registry = one(ctx.args, "registry");
  const query = { ...(rest[0] === undefined ? {} : { text: rest[0] }), ...(kind === undefined ? {} : { kind }), ...(registry === undefined ? {} : { registry }) };
  const hits = await withConnection(ctx, async (c) => valueOf<PluginSearchResult>(await c.call("plugin.search", query)));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(hits));
  else if (hits.length === 0) ctx.output.out("No plugins found. Registries are searched as of their last refresh: `finisherd registry refresh`.");
  else for (const l of hits) ctx.output.out(listingLine(l));
  return 0;
}

async function info(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "plugin info", ["json"]);
  const { id } = idAndVersion("plugin info", rest, false);
  const r = await withConnection(ctx, async (c) => valueOf<PluginInfoResult>(await c.call("plugin.info", { id })));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else for (const line of infoLines(r.id, r.installed, r.registries)) ctx.output.out(line);
  return 0;
}

interface PluginList {
  plugins: InstalledRow[];
  bundled?: { integrity: string; plugins: Array<{ id: string; version: string; ok: boolean; code?: string }> };
  problems?: Array<{ id: string; version: string; reason: string }>;
}

async function ls(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "plugin ls", ["json"]);
  if (rest.length > 0) usage("plugin ls takes no arguments");
  const list = await withConnection(ctx, async (c) => valueOf<PluginList>(await c.call("plugin.list", {})));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(list));
  else {
    if (list.plugins.length === 0) ctx.output.out("No plugins installed.");
    for (const p of list.plugins) ctx.output.out(installedLine(p));
    for (const b of list.bundled?.plugins ?? []) if (!b.ok) ctx.output.out(`!! ${safe(b.id)} ${safe(b.version)}: ${safe(b.code ?? "its files do not match this build")}`);
    for (const p of list.problems ?? []) ctx.output.out(`!! ${safe(p.id)} ${safe(p.version)}: ${safe(p.reason)}; it was switched off`);
  }
  return 0;
}

/** `REGISTRY_AMBIGUOUS` is a question for the person: which registry? (exit 2, like any missing argument). */
function askForRegistry(e: ConsoleError): ConsoleError {
  const d = (e.error.details ?? {}) as { id?: string; reason?: string; registry?: string; registries?: string[] };
  if (d.reason === "rootChanged") {
    return consoleError(
      "USAGE",
      `registry ${safe(d.registry ?? "?")}, which ${safe(d.id ?? "this plugin")} was installed from, now has another root: it is not the same registry`,
      { reason: "rootChanged", registry: d.registry },
      "Check the registry you trust, then say which one to take the update from with --registry <id>.",
    );
  }
  return consoleError(
    "USAGE",
    `${d.registries?.length ?? "several"} registries list ${safe(d.id ?? "this plugin")}: ${(d.registries ?? []).map(safe).join(", ")}`,
    { reason: "ambiguous", registries: d.registries ?? [] },
    "Say which one with --registry <id>.",
  );
}

async function plan(c: ConsoleConnection, ctx: CommandContext, request: Record<string, unknown>, downgrade: boolean): Promise<InstallPlan> {
  try {
    return downgrade
      ? await confirmed<InstallPlan>(c, ctx.prompt, `This plans a DOWNGRADE of ${String(request.id)} to ${String(request.version)}. Nothing is installed until you approve the plan.`, proceed, "plugin.plan", request)
      : valueOf<InstallPlan>(await c.call("plugin.plan", request));
  } catch (e) {
    throw e instanceof ConsoleError && e.error.code === "REGISTRY_AMBIGUOUS" ? askForRegistry(e) : e;
  }
}

async function install(ctx: CommandContext, rest: ReadonlyArray<string>, update: boolean): Promise<number> {
  const command = update ? "plugin update" : "plugin install";
  onlyFlags(ctx.args, command, update ? ["registry", "allow-downgrade", "auto-update", "no-auto-update", "accept-copyleft"] : ["registry", "auto-update", "accept-copyleft"]);
  const { id, version } = idAndVersion(command, rest, true);
  const registry = one(ctx.args, "registry");
  const downgrade = has(ctx.args, "allow-downgrade");
  if (downgrade && version === undefined) usage("--allow-downgrade needs the exact version: plugin update <id>@<version> --allow-downgrade");
  if (has(ctx.args, "auto-update") && has(ctx.args, "no-auto-update")) usage("--auto-update and --no-auto-update contradict each other");
  const autoUpdate = has(ctx.args, "auto-update") ? true : has(ctx.args, "no-auto-update") ? false : undefined;
  requireTty(ctx.prompt, command);
  const request = {
    id,
    ...(version === undefined ? {} : { version }),
    ...(registry === undefined ? {} : { registry }),
    ...(update ? { update: true } : {}),
    ...(downgrade ? { allowDowngrade: true } : {}),
  };
  const done = await withConnection(ctx, async (c) => {
    const p = await plan(c, ctx, request, downgrade);
    // A plan holds one of the maker's few slots (`TOO_MANY_PLANS`) and its staged files until it
    // is installed or expires: every way out of here but a finished install gives it up (review M3).
    let finished = false;
    try {
      const shown = renderPlan(p);
      for (const line of shown.lines) ctx.prompt.show(line);
      if (update && !widens(p) && p.action === "update") ctx.prompt.show("This update asks for nothing the installed version did not.");
      if (shown.copyleft && !has(ctx.args, "accept-copyleft")) {
        throw consoleError("USAGE", "this plugin is under copyleft terms: nothing was installed", { reason: "copyleft" }, "Read the licence, then run the command again with --accept-copyleft.");
      }
      const accepting = shown.thirdPartyCode ? ", and accepts the risk of its third-party code above" : "";
      await ask(ctx.prompt, `This ${p.action}s ${safe(p.plugin.id)} ${safe(p.plugin.version)}, exactly as the plan above${accepting}.`, shown.thirdPartyCode ? typeName("plugin's id", p.plugin.id) : proceed, "plugin.install");
      const body = {
        planId: p.planId,
        digest: p.digest,
        accept: { ...(shown.copyleft ? { copyleft: true } : {}), ...(shown.thirdPartyCode ? { thirdPartyCode: true } : {}) },
        ...(autoUpdate === undefined ? {} : { autoUpdate }),
      };
      const result = await withChallenge<PluginInstallResult>(c, "plugin.install", body);
      finished = true;
      return result;
    } finally {
      if (!finished) await c.call("plugin.discard", { planId: p.planId }).catch(() => undefined); // best effort: it expires anyway
    }
  });
  const row = done.plugin;
  const did = done.action === "install" ? "Installed" : done.action === "update" ? "Updated" : "Downgraded";
  ctx.output.out(`${did} ${safe(row.id)} ${safe(row.version)}${row.enabled ? " (enabled)" : " (switched off: this version of Finisher cannot run code plugins yet)"}.`);
  if (row.autoUpdate) ctx.output.out("It updates itself when a refresh finds a newer version that asks for nothing more.");
  return 0;
}

async function rm(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "plugin rm", ["purge-data"]);
  const { id } = idAndVersion("plugin rm", rest, false);
  const purge = has(ctx.args, "purge-data");
  requireTty(ctx.prompt, "plugin rm");
  await withConnection(ctx, (c) =>
    confirmed(c, ctx.prompt, `This removes ${id} and its files${purge ? ", and deletes the data it stored" : "; the data it stored is kept"}.`, purge ? typeName("plugin's id", id) : proceed, "plugin.remove", { id, ...(purge ? { purgeData: true } : {}) }),
  );
  ctx.output.out(`Removed ${id}.`);
  return 0;
}

async function setEnabled(ctx: CommandContext, sub: "enable" | "disable", rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, `plugin ${sub}`, []);
  const { id } = idAndVersion(`plugin ${sub}`, rest, false);
  requireTty(ctx.prompt, `plugin ${sub}`);
  const { plugin: row } = await withConnection(ctx, (c) =>
    confirmed<{ plugin: InstalledRow }>(c, ctx.prompt, `This ${sub}s ${id} on this server.`, proceed, `plugin.${sub}`, { id }),
  );
  ctx.output.out(`${safe(row.id)} is now ${row.enabled ? "enabled" : "disabled"}.`);
  return 0;
}
