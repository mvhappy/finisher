// `finisherd agent mint|credentials|revoke` and the mint `finisherd mcp setup` shares (spec §4.7, plan 5A
// Task 9): the owner's agent and MCP credentials. Minting and revoking need a terminal and a `y`
// (owner decision of 2026-11-08); listing reads only and runs anywhere. The agent session
// commands (`agent start|ls|show|…`, 7A Task 12) are `agent/console.ts`'s; `agent ls` lists
// sessions since then, and the credentials moved to `agent credentials`.
//
// A minted credential's file is the only copy of its key. With `--out` the console creates the
// file **before** it mints (`createPrivateFile`: new, owner-only from creation, and its open
// file checked: owner and mode 0600 on Unix, owner and DACL on Windows; refused and removed
// otherwise), so a mint never leaves a live credential without its file. The key is written
// whole and fsynced; when that fails the credential is revoked on the same connection (no new
// code: the server lets the connection that minted a credential take it back) and the file
// removed. A cancelled answer, Ctrl-C or any other failure removes the file too. Without
// `--out` (`agent mint` only) the file's contents are shown once on this terminal.
import { resolve } from "node:path";
import type { Credential, CredentialFile, CredentialKind } from "@finisher/contracts";
import { SCOPES } from "@finisher/contracts";
import { createPrivateFile, type PrivateFile } from "@finisher/fin-node";
import { consoleError, valueOf } from "../client.ts";
import { confirmed, proceed, requireTty } from "../confirm.ts";
import { agentSessions, SESSION_COMMANDS } from "../../agent/console.ts";
import { all, has, one, onlyFlags, usage } from "../format.ts";
import { type CommandContext, withConnection } from "./context.ts";

export async function agent(ctx: CommandContext): Promise<number> {
  const [sub, ...rest] = ctx.args.words;
  if (sub === "mint") return mint(ctx, "agent", rest);
  if (sub === "credentials") return list(ctx, rest);
  if (sub === "revoke") return revoke(ctx, rest);
  if (sub !== undefined && SESSION_COMMANDS.has(sub)) return agentSessions(ctx, sub, rest);
  return usage(sub === undefined ? "agent needs a command (finisherd help lists them)" : `unknown agent command: ${sub}`);
}

/** `30s`, `15m`, `24h`, `7d` (or plain seconds) as seconds; `none` as `null` where allowed. */
export function parseDuration(flag: string, value: string, noneAllowed: boolean): number | null {
  if (noneAllowed && value === "none") return null;
  const m = /^(\d+)([smhd]?)$/.exec(value);
  if (m === null) usage(`--${flag} needs a duration such as 90m, 24h or 7d${noneAllowed ? ", or none" : ""}`);
  const unit = { "": 1, s: 1, m: 60, h: 3600, d: 86_400 }[m[2] as "" | "s" | "m" | "h" | "d"];
  return Number(m[1]) * unit;
}

/** Creates the credential's file, private and new (module docs); `undefined` without `--out`. */
function reserve(out: string | undefined, create: (path: string) => PrivateFile = createPrivateFile): PrivateFile | undefined {
  if (out === undefined) return undefined;
  const path = resolve(out);
  try {
    return create(path);
  } catch (e) {
    const err = e as { code?: unknown; details?: { problems?: unknown } };
    const problems = Array.isArray(err.details?.problems) ? (err.details.problems as unknown[]).map(String) : [];
    throw consoleError(
      "USAGE",
      err.code === "ALREADY_EXISTS"
        ? `${path} already exists`
        : err.code === "PRIVATE_FILE_UNSAFE"
          ? `${path} would not be private (${problems.join(", ")}), so it was removed`
          : `cannot create ${path}`,
      { path, ...(typeof err.code === "string" ? { cause: err.code } : {}), ...(problems.length === 0 ? {} : { problems }) },
      "Choose a new file in a folder only you can read; the credential is written there once.",
    );
  }
}

/** What a mint grants, in words; `*` projects said plainly (review of Tasks 8–9, L6). */
export function describeMint(kind: CredentialKind, label: string, scopes: ReadonlyArray<string>, projects: ReadonlyArray<string>, branches: ReadonlyArray<string>): string {
  return (
    `This creates ${kind === "agent" ? "an agent" : "an MCP"} credential "${label}" with ` +
    `${scopes.length === 0 ? "the default scopes" : `the scopes ${scopes.join(", ")}`}, ` +
    `${projects.length === 0 ? "ALL projects (*): every project on this server, and any folder path it names, to open or create projects in" : `the projects ${projects.join(", ")}`} and ` +
    `${branches.length === 0 ? "every branch" : `the branches ${branches.join(", ")}`}. ` +
    "Whoever holds its file can use Finisher with these rights until it expires or you revoke it."
  );
}

/** What `fin` needs beside an agent credential (7A Decision 14b and the 8b review): it reads no server data. */
const FIN_UNDER_AGENT = "Under it fin also needs FIN_SERVER (the server's socket, in finisherd.json) and FIN_PROJECT_DIR (the project's folder).";

export async function mint(ctx: CommandContext, kind: CredentialKind, rest: ReadonlyArray<string>): Promise<number> {
  const command = kind === "agent" ? "agent mint" : "mcp setup";
  onlyFlags(ctx.args, command, ["label", "scope", "project", "branch", "idle", "absolute", "out"]);
  if (rest.length > 0) usage(`${command} takes flags only`);
  const label = one(ctx.args, "label") ?? (kind === "mcp" ? "MCP adapter" : undefined);
  if (label === undefined) usage("agent mint needs --label <name>: what you call the harness that will use it");
  const scopes = all(ctx.args, "scope");
  for (const s of scopes) if (!(SCOPES as ReadonlyArray<string>).includes(s)) usage(`unknown scope: ${s}`);
  const projects = all(ctx.args, "project");
  const branches = all(ctx.args, "branch");
  const idle = one(ctx.args, "idle");
  const absolute = one(ctx.args, "absolute");
  const payload = {
    kind,
    label,
    ...(scopes.length === 0 ? {} : { scopes }),
    ...(projects.length === 0 ? {} : { projects }),
    ...(branches.length === 0 ? {} : { branches }),
    ...(idle === undefined ? {} : { idleS: parseDuration("idle", idle, false) }),
    ...(absolute === undefined ? {} : { absoluteS: parseDuration("absolute", absolute, true) }),
  };
  requireTty(ctx.prompt, command);
  const file = reserve(one(ctx.args, "out"), ctx.createFile);
  // Ctrl-C away from the question (while the server answers) removes the file too.
  const interrupted = () => {
    file?.discard();
    process.exit(130);
  };
  if (file !== undefined) process.once("SIGINT", interrupted);
  let minted: { credential: Credential; file: CredentialFile };
  try {
    minted = await withConnection(ctx, async (c) => {
      const m = await confirmed<{ credential: Credential; file: CredentialFile }>(c, ctx.prompt, describeMint(kind, label, scopes, projects, branches), proceed, "auth.credential.mint", payload);
      if (file === undefined) return m;
      try {
        file.commit(`${JSON.stringify(m.file)}\n`);
      } catch (e) {
        // Its only copy is lost: take the credential back at once, on this connection.
        const undone = await c.call("auth.credential.revoke", { id: m.credential.id }).catch(() => undefined);
        const revoked = undone?.ok === true && (undone.value as { revoked?: unknown }).revoked === true;
        file.discard();
        const why = (e as { message?: unknown }).message;
        throw consoleError(
          "IO_ERROR",
          `cannot write ${file.path}${typeof why === "string" ? `: ${why}` : ""}; credential ${m.credential.id} was ${revoked ? "revoked" : "NOT revoked"}`,
          { path: file.path, credentialId: m.credential.id, revoked },
          revoked ? "Free some space or choose another folder, then mint it again." : `Revoke credential ${m.credential.id} yourself, then mint it again.`,
        );
      }
      return m;
    });
  } catch (e) {
    file?.discard();
    throw e;
  } finally {
    if (file !== undefined) process.off("SIGINT", interrupted);
  }
  const { credential } = minted;
  if (file === undefined) {
    ctx.output.out(`Minted ${credential.kind} credential ${credential.id} ("${credential.label}"). Its file, shown this once:`);
    ctx.output.out(JSON.stringify(minted.file));
    ctx.output.out("Save it as a file only you can read, and point the harness's FIN_CREDENTIAL variable at that file.");
    if (kind === "agent") ctx.output.out(FIN_UNDER_AGENT);
    return 0;
  }
  ctx.output.out(`Minted ${credential.kind} credential ${credential.id} ("${credential.label}") into ${file.path}.`);
  ctx.output.out(`Point the ${kind === "agent" ? "harness's" : "MCP adapter's"} FIN_CREDENTIAL variable at that file.`);
  if (kind === "agent") ctx.output.out(FIN_UNDER_AGENT);
  return 0;
}

function when(ms: number | undefined): string {
  return ms === undefined ? "-" : new Date(ms).toISOString();
}

function line(c: Credential): string {
  const projects = c.projects === "*" ? "every project" : c.projects.join(",");
  const branches = c.branches === "*" ? "every branch" : c.branches.join(",");
  const parts = [c.id, c.kind, c.status, JSON.stringify(c.label), `scopes ${c.scopes.join(",")}`, projects, branches];
  parts.push(`used ${when(c.lastUsedAt)}`, `idle ${Math.round(c.idleExpiryS / 3600)}h`, `expires ${when(c.absoluteExpiryAt)}`);
  return parts.join("  ");
}

async function list(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent credentials", ["all", "json"]);
  if (rest.length > 0) usage("agent credentials takes --all and --json only");
  const { credentials } = await withConnection(ctx, async (c) =>
    valueOf<{ credentials: Credential[] }>(await c.call("auth.credential.list", has(ctx.args, "all") ? { all: true } : {})),
  );
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ credentials }));
  else if (credentials.length === 0) ctx.output.out(has(ctx.args, "all") ? "No credentials." : "No live credentials.");
  else for (const c of credentials) ctx.output.out(line(c));
  return 0;
}

async function revoke(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent revoke", []);
  const [id, ...more] = rest;
  if (id === undefined || more.length > 0) usage("agent revoke needs one credential id (agent credentials lists them)");
  requireTty(ctx.prompt, "agent revoke");
  const { revoked } = await withConnection(ctx, async (c) => {
    const { credentials } = valueOf<{ credentials: Credential[] }>(await c.call("auth.credential.list", {}));
    const found = credentials.find((x) => x.id === id);
    if (found === undefined) return { revoked: false };
    return confirmed<{ revoked: boolean }>(
      c,
      ctx.prompt,
      `This revokes the ${found.kind} credential ${id} ("${found.label}"): its connections end now, and its file stops working.`,
      proceed,
      "auth.credential.revoke",
      { id },
    );
  });
  ctx.output.out(revoked ? `Revoked credential ${id}.` : `No live credential ${id}: nothing changed.`);
  return 0;
}
