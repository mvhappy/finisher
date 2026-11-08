// The built-in agent's workspace file tools (plan 7A Decision 12; spec §4.3): `workspace_read`,
// `workspace_write` and `workspace_list`, confined to `workspace/<sessionId>/`.
//
// - **Paths** are relative, never absolute, never `..`; the real path (symlinks resolved;
//   case-folded on Windows and on macOS, whose volumes are case-insensitive by default) must
//   stay under the workspace's real path, else `PATH_NOT_ALLOWED`. A link inside the workspace
//   that points out of it (to `<server-data>/auth/local.key`, say) is refused like any other
//   path outside.
// - **Sizes:** a read returns at most 256 KiB; a file is at most 8 MiB; a session's workspace at
//   most 256 MiB.
// - **Writes** are temp-then-rename in the target's folder (arch §6.7), so a reader never sees
//   half a file; `create` refuses an existing file, `append` rewrites the whole file.
import { randomBytes } from "node:crypto";
import { lstat, mkdir, open, readdir, readFile, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, sep } from "node:path";
import { finError } from "../../errors.ts";
import type { ToolOutcome } from "../runtime-deps.ts";

export const WORKSPACE_TOOLS = ["workspace_read", "workspace_write", "workspace_list"] as const;
export type WorkspaceToolName = (typeof WORKSPACE_TOOLS)[number];

export const READ_MAX = 256 * 1024;
export const FILE_MAX = 8 * 1024 * 1024;
export const WORKSPACE_MAX = 256 * 1024 * 1024;
const LIST_MAX = 1000;

export interface WorkspaceLimits {
  readonly readMax?: number;
  readonly fileMax?: number;
  readonly workspaceMax?: number;
  /** Compare paths case-insensitively (default: on Windows and macOS). */
  readonly fold?: boolean;
}

const notAllowed = (path: string, reason: string) =>
  finError("PATH_NOT_ALLOWED", `${path}: the workspace tools reach this session's workspace only`, { path, reason }, "name a path relative to the workspace, without `..`");

/** The normalised segments of a relative path; `PATH_NOT_ALLOWED` for an absolute one or `..`. */
export function segments(path: unknown): string[] {
  if (typeof path !== "string") throw finError("USAGE", "path must be a string", {});
  if (path.includes("\0")) throw notAllowed(path, "nul");
  // Windows forms too, on every OS: `C:\x`, `C:x`, `\\server\x`, `\x`.
  if (isAbsolute(path) || /^[A-Za-z]:/.test(path) || path.startsWith("\\") || path.startsWith("/")) throw notAllowed(path, "absolute");
  const parts = path.split(/[\\/]+/).filter((p) => p !== "" && p !== ".");
  if (parts.some((p) => p === "..")) throw notAllowed(path, "dotdot");
  return parts;
}

const fold = (p: string, on: boolean) => (on ? p.toLowerCase() : p);

function inside(real: string, root: string, on: boolean): boolean {
  const r = fold(real, on);
  const base = fold(root, on);
  return r === base || r.startsWith(base.endsWith(sep) ? base : base + sep);
}

/** The real path of `p`: its deepest existing ancestor resolved, the rest appended. */
async function realOf(p: string): Promise<string> {
  const rest: string[] = [];
  let cur = p;
  for (;;) {
    try {
      return join(await realpath(cur), ...rest.reverse());
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code !== "ENOENT") throw e;
      const parent = dirname(cur);
      if (parent === cur) return p;
      rest.push(cur.slice(parent.length).replace(/^[\\/]+/, ""));
      cur = parent;
    }
  }
}

/** The confined real path of `path` under `root` (both resolved). */
export async function confine(path: unknown, root: string, limits: WorkspaceLimits = {}): Promise<{ real: string; root: string; rel: string }> {
  const parts = segments(path);
  const on = limits.fold ?? (process.platform === "win32" || process.platform === "darwin");
  await mkdir(root, { recursive: true });
  const realRoot = await realpath(root);
  const real = await realOf(join(realRoot, ...parts));
  if (!inside(real, realRoot, on)) throw notAllowed(String(path), "outside");
  return { real, root: realRoot, rel: parts.join("/") };
}

async function size(dir: string): Promise<number> {
  let total = 0;
  for (const e of await readdir(dir, { withFileTypes: true }).catch(() => [])) {
    const p = join(dir, e.name);
    if (e.isDirectory()) total += await size(p);
    else if (e.isFile()) total += (await lstat(p)).size;
  }
  return total;
}

const str = (v: unknown, name: string): string => {
  if (typeof v !== "string") throw finError("USAGE", `${name} must be a string`, { field: name });
  return v;
};
const count = (v: unknown, name: string): number | undefined => {
  if (v === undefined) return undefined;
  if (typeof v !== "number" || !Number.isInteger(v) || v < 0) throw finError("USAGE", `${name} must be a whole number`, { field: name });
  return v;
};

async function read(args: Record<string, unknown>, root: string, l: WorkspaceLimits): Promise<ToolOutcome> {
  const { real, rel } = await confine(args.path, root, l);
  const max = l.readMax ?? READ_MAX;
  const offset = count(args.offset, "offset") ?? 0;
  const limit = Math.min(count(args.limit, "limit") ?? max, max);
  const st = await stat(real).catch(() => undefined);
  if (st === undefined) throw finError("NOT_FOUND", `${rel}: no such file in the workspace`, { path: rel });
  if (!st.isFile()) throw finError("USAGE", `${rel} is not a file`, { path: rel });
  const fh = await open(real, "r");
  try {
    const buf = Buffer.alloc(Math.max(0, Math.min(limit, st.size - offset)));
    const { bytesRead } = await fh.read(buf, 0, buf.length, offset);
    const end = offset + bytesRead;
    return { ok: true, output: { path: rel, size: st.size, offset, content: buf.subarray(0, bytesRead).toString("utf8"), ...(end < st.size ? { truncated: true, next: end } : {}) } };
  } finally {
    await fh.close();
  }
}

async function write(args: Record<string, unknown>, root: string, l: WorkspaceLimits): Promise<ToolOutcome> {
  const { real, root: realRoot, rel } = await confine(args.path, root, l);
  if (rel === "") throw finError("USAGE", "path names the workspace itself", {});
  const content = str(args.content, "content");
  const mode = args.mode ?? "create";
  if (mode !== "create" && mode !== "overwrite" && mode !== "append") throw finError("USAGE", "mode must be create, overwrite or append", { mode });
  const existing = await lstat(real).catch(() => undefined);
  if (existing?.isDirectory()) throw finError("USAGE", `${rel} is a folder`, { path: rel });
  if (mode === "create" && existing !== undefined) throw finError("ALREADY_EXISTS", `${rel} already exists`, { path: rel }, "write it with mode overwrite or append");
  const before = existing?.isFile() ? existing.size : 0;
  const bytes = mode === "append" && existing?.isFile() ? Buffer.concat([await readFile(real), Buffer.from(content, "utf8")]) : Buffer.from(content, "utf8");
  const fileMax = l.fileMax ?? FILE_MAX;
  if (bytes.length > fileMax) throw finError("FILE_TOO_LARGE", `${rel} would be ${bytes.length} bytes; a workspace file is at most ${fileMax}`, { path: rel, size: bytes.length, limit: fileMax });
  const wsMax = l.workspaceMax ?? WORKSPACE_MAX;
  const used = await size(realRoot);
  if (used - before + bytes.length > wsMax) {
    throw finError("WORKSPACE_FULL", `the workspace would hold more than ${wsMax} bytes`, { used, limit: wsMax }, "remove files you no longer need, or keep outputs smaller");
  }
  await mkdir(dirname(real), { recursive: true });
  // The parent made: check again that nothing on the way out was a link made meanwhile.
  const parent = await realpath(dirname(real));
  if (!inside(parent, realRoot, l.fold ?? (process.platform === "win32" || process.platform === "darwin"))) throw notAllowed(rel, "outside");
  const tmp = join(parent, `.${randomBytes(6).toString("hex")}.tmp`);
  try {
    await writeFile(tmp, bytes, { flag: "wx", mode: 0o600 });
    await rename(tmp, join(parent, basename(real)));
  } catch (e) {
    await rm(tmp, { force: true });
    throw e;
  }
  return { ok: true, output: { path: rel, size: bytes.length, mode } };
}

async function list(args: Record<string, unknown>, root: string, l: WorkspaceLimits): Promise<ToolOutcome> {
  const { real, rel } = await confine(args.path ?? ".", root, l);
  const st = await stat(real).catch(() => undefined);
  if (st === undefined) throw finError("NOT_FOUND", `${rel || "."}: no such folder in the workspace`, { path: rel });
  if (!st.isDirectory()) throw finError("USAGE", `${rel} is not a folder`, { path: rel });
  const all = await readdir(real, { withFileTypes: true });
  const entries = [];
  for (const e of all.slice(0, LIST_MAX)) {
    const kind = e.isDirectory() ? "dir" : e.isSymbolicLink() ? "link" : e.isFile() ? "file" : "other";
    entries.push({ name: e.name, kind, ...(kind === "file" ? { size: (await lstat(join(real, e.name))).size } : {}) });
  }
  return { ok: true, output: { path: rel || ".", entries, ...(all.length > LIST_MAX ? { truncated: true, total: all.length } : {}) } };
}

/** Runs one workspace tool against `root` (`workspace/<sessionId>/`). Errors are thrown as `FinError`s. */
export async function workspaceTool(name: WorkspaceToolName, args: unknown, root: string, limits: WorkspaceLimits = {}): Promise<ToolOutcome> {
  const a = (args ?? {}) as Record<string, unknown>;
  if (typeof a !== "object" || Array.isArray(a)) throw finError("USAGE", `${name} takes an object`, {});
  switch (name) {
    case "workspace_read":
      return read(a, root, limits);
    case "workspace_write":
      return write(a, root, limits);
    case "workspace_list":
      return list(a, root, limits);
  }
}
