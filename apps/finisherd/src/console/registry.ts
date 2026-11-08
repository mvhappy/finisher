// `finisherd registry add|ls|status|refresh|rm|forget` (plan 8A Task 12b; spec §5.1, Decision
// 14 and 16): the owner's registries. Listing, status and refreshing read or only fetch (no
// terminal needed); adding, removing and forgetting change what the server trusts, so each
// needs a terminal and a typed code. No registry is ever trusted on first use: `add` takes a
// root file or its SHA-256, or shows the root's hash and key ids and waits for the owner to type
// the hash back (the owner compares it with what the registry's publisher published, not with
// this folder).
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { RefreshResult, RegistryEntry, RegistryListResult, RegistryRefreshResult } from "@finisher/contracts";
import { consoleError, valueOf } from "./client.ts";
import { confirmed, requireTty } from "./confirm.ts";
import type { CommandContext } from "./commands/context.ts";
import { withConnection } from "./commands/context.ts";
import { has, one, onlyFlags, printFailure, usage } from "./format.ts";
import { registryLines } from "./plugin-render.ts";

const SHA256 = /^[0-9a-fA-F]{64}$/;
const REGISTRY_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;

export async function registry(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  switch (sub) {
    case "add":
      return add(ctx, rest);
    case "ls":
      return ls(ctx, rest);
    case "status":
      return status(ctx, rest);
    case "refresh":
      return refresh(ctx, rest);
    case "rm":
      return rm(ctx, rest);
    case "forget":
      return forget(ctx, rest);
    default:
      return usage(sub === undefined ? "registry needs a command" : `unknown registry command: ${sub}`);
  }
}

function idArg(command: string, rest: ReadonlyArray<string>): string {
  const [id, ...more] = rest;
  if (id === undefined || more.length > 0) usage(`${command} takes one registry id`);
  if (!REGISTRY_ID.test(id)) usage(`${id} is not a registry id (lowercase letters, digits and hyphens)`);
  return id;
}

/** What the console shows of a folder's first root, from the file itself (never from the server). */
export interface RootCandidate {
  readonly sha256: string;
  readonly version: number;
  readonly keyIds: ReadonlyArray<string>;
  readonly threshold: number;
}

/** The folder's `metadata/1.root.json` as the owner is asked to confirm it. */
export function rootCandidate(dir: string): RootCandidate {
  const file = join(dir, "metadata", "1.root.json");
  let bytes: Buffer;
  try {
    bytes = readFileSync(file);
  } catch {
    throw consoleError("REGISTRY_INVALID", "the folder has no metadata/1.root.json", { reason: "noRoot" }, "Point --dir at the registry's folder (the one holding metadata/ and targets/).");
  }
  try {
    const root = (JSON.parse(bytes.toString("utf8")) as { signed?: { roles?: { root?: { keyids?: unknown; threshold?: unknown } } } }).signed?.roles?.root;
    const keyIds = Array.isArray(root?.keyids) ? root.keyids.filter((k): k is string => typeof k === "string").sort() : [];
    if (keyIds.length === 0 || typeof root?.threshold !== "number") throw new Error("no root keys");
    return { sha256: createHash("sha256").update(bytes).digest("hex"), version: 1, keyIds, threshold: root.threshold };
  } catch {
    throw consoleError("REGISTRY_INVALID", "metadata/1.root.json is not a TUF root", { reason: "parse" });
  }
}

type RootArg = { kind: "file"; path: string } | { kind: "sha256"; sha256: string };

async function add(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry add", ["dir", "root", "root-sha256"]);
  const id = idArg("registry add", rest);
  const dirText = one(ctx.args, "dir");
  if (dirText === undefined) usage("registry add needs --dir <path>", "Point --dir at your clone of the registry; run `git pull` there to update it.");
  const dir = resolve(dirText);
  const file = one(ctx.args, "root");
  const hash = one(ctx.args, "root-sha256");
  if (file !== undefined && hash !== undefined) usage("registry add takes --root <file> or --root-sha256 <hex>, not both");
  if (hash !== undefined && !SHA256.test(hash)) usage("--root-sha256 needs 64 hexadecimal characters");
  requireTty(ctx.prompt, "registry add");
  let root: RootArg;
  let trusting: string;
  if (file !== undefined) {
    let bytes: Buffer;
    try {
      bytes = readFileSync(resolve(file));
    } catch {
      throw consoleError("REGISTRY_INVALID", `the root file ${resolve(file)} cannot be read`, { reason: "noRootFile" });
    }
    root = { kind: "file", path: resolve(file) };
    trusting = `the root file ${resolve(file)} (SHA-256 ${createHash("sha256").update(bytes).digest("hex")})`;
  } else if (hash !== undefined) {
    root = { kind: "sha256", sha256: hash.toLowerCase() };
    trusting = `the root with SHA-256 ${hash.toLowerCase()}`;
  } else {
    const c = rootCandidate(dir);
    ctx.prompt.show(`The registry in ${dir} is not trusted yet. Its first root is:`);
    ctx.prompt.show(`  SHA-256:   ${c.sha256}`);
    ctx.prompt.show(`  root keys: ${c.keyIds.join(", ")} (${c.threshold} must sign)`);
    ctx.prompt.show("Compare the SHA-256 with the one the registry's publisher gave you, on a page or in a message that does not come from this folder. If they differ, stop.");
    const typed = (await ctx.prompt.ask("Type the SHA-256 to trust this root: ")).trim().toLowerCase();
    if (typed.length === 0) throw consoleError("CONFIRMATION_REQUIRED", "nothing was typed, so no registry was added", { reason: "cancelled" });
    if (typed !== c.sha256) throw consoleError("CONFIRMATION_REQUIRED", "what you typed is not that root's SHA-256, so no registry was added", { reason: "mismatch" });
    root = { kind: "sha256", sha256: typed };
    trusting = `the root with SHA-256 ${typed}`;
  }
  const entry = await withConnection(ctx, async (c) => {
    const added = await confirmed<RegistryEntry>(
      c,
      ctx.prompt,
      `This adds the registry ${id} from ${dir}, and trusts ${trusting}: whatever it signs under that root can be installed here, with your approval each time.`,
      "registry.add",
      { cfg: { id, path: dir }, root },
    );
    // Fetch its listing now; the registry stays added even when this fails.
    const refreshed = await c.call("registry.refresh", { id });
    return { added, refreshed };
  });
  ctx.output.out(`Added registry ${id} (root ${entry.added.rootSha256}).`);
  if (entry.refreshed.ok) {
    const result = (entry.refreshed.value as RegistryRefreshResult).results[0];
    if (result?.ok === true) ctx.output.out(`It lists ${result.status.plugins} plugins.`);
    else if (result !== undefined && !result.ok) ctx.output.out(`Its listing could not be fetched yet: ${result.error.code}. Try \`finisherd registry refresh ${id}\`.`);
  } else ctx.output.out(`Its listing could not be fetched yet: ${entry.refreshed.error.code}. Try \`finisherd registry refresh ${id}\`.`);
  return 0;
}

async function ls(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry ls", ["json"]);
  if (rest.length > 0) usage("registry ls takes no arguments");
  const list = await withConnection(ctx, async (c) => valueOf<RegistryListResult>(await c.call("registry.list", {})));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(list));
  else {
    if (list.registries.length === 0 && list.problems.length === 0) ctx.output.out("No registries. Add one with `finisherd registry add <id> --dir <path>`.");
    for (const e of list.registries) for (const line of registryLines(e)) ctx.output.out(line);
    for (const p of list.problems) ctx.output.out(`${p.id}  damaged: ${p.code}`);
  }
  return 0;
}

async function status(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry status", ["json"]);
  const id = idArg("registry status", rest);
  const e = await withConnection(ctx, async (c) => valueOf<RegistryEntry>(await c.call("registry.status", { id })));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(e));
  else for (const line of registryLines(e)) ctx.output.out(line);
  return 0;
}

async function refresh(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry refresh", ["json"]);
  if (rest.length > 1) usage("registry refresh takes at most one registry id");
  const id = rest.length === 1 ? idArg("registry refresh", rest) : undefined;
  const r = await withConnection(ctx, async (c) => valueOf<RegistryRefreshResult>(await c.call("registry.refresh", id === undefined ? {} : { id })));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else {
    for (const x of r.results) ctx.output.out(refreshLine(x));
    for (const v of r.applied.revoked) ctx.output.out(`Revoked ${v.id} ${v.version}: advisory ${v.advisory} says to disable it.`);
    for (const u of r.applied.updated) ctx.output.out(`Updated ${u.id} ${u.from} → ${u.to} automatically (it widens nothing).`);
  }
  // A refresh that failed for a registry exits as its error would.
  const failed = r.results.find((x): x is Extract<RefreshResult, { ok: false }> => !x.ok);
  return failed === undefined ? 0 : printFailure(ctx.output, failed.error);
}

function refreshLine(x: RefreshResult): string {
  if (!x.ok) return `${x.id}: refresh failed (${x.error.code})`;
  return `${x.id}: refreshed, ${x.status.plugins} plugins`;
}

async function rm(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry rm", []);
  const id = idArg("registry rm", rest);
  requireTty(ctx.prompt, "registry rm");
  await withConnection(ctx, (c) =>
    confirmed(c, ctx.prompt, `This removes the registry ${id} from this server. Plugins installed from it stay; its trusted data is kept, so adding it again with the same root resumes where it left off.`, "registry.remove", { id }),
  );
  ctx.output.out(`Removed registry ${id}.`);
  return 0;
}

async function forget(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "registry forget", ["root"]);
  const flag = one(ctx.args, "root");
  if ((flag === undefined) === (rest.length === 0)) usage("registry forget takes a root's SHA-256: --root <sha256>");
  const sha = flag ?? rest[0]!;
  if (rest.length > 1 || !SHA256.test(sha)) usage("registry forget takes one root SHA-256 (64 hexadecimal characters)");
  requireTty(ctx.prompt, "registry forget");
  const rootSha256 = sha.toLowerCase();
  await withConnection(ctx, (c) =>
    confirmed(c, ctx.prompt, `This deletes everything this server trusted under the root ${rootSha256}. A registry added with that root starts over.`, "registry.forget", { rootSha256 }),
  );
  ctx.output.out(`Forgot the trust held under root ${rootSha256}.`);
  return 0;
}
