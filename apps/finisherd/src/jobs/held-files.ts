// The warm contract's check (5C-1 plan Decisions 6, 36), in test-hooks builds only: after a warm
// worker's `ready`, the paths under the job directory it still holds open or mapped. Linux reads
// `/proc/<pid>/fd` and `/proc/<pid>/maps`; macOS asks fin-node (`proc_pidinfo`; no process is
// started, Task 11 review L3); Windows renames the job directory and back, which a handle held
// inside it makes fail. A check that cannot be made throws: it never passes by default.
import { readdir, readFile, readlink, realpath, rename } from "node:fs/promises";
import { join, sep } from "node:path";
import { testHooks } from "@finisher/fin-node";

/** Whether `path` is `dir` or lies under one of `dirs`. */
function under(path: string, dirs: ReadonlyArray<string>): boolean {
  return dirs.some((d) => path === d || path.startsWith(d.endsWith(sep) ? d : d + sep));
}

const code = (e: unknown): unknown => (e as { code?: unknown }).code;

async function linuxHeld(pid: number): Promise<string[]> {
  const out: string[] = [];
  const fdDir = `/proc/${pid}/fd`;
  for (const fd of await readdir(fdDir)) {
    let target: string;
    try {
      target = await readlink(join(fdDir, fd));
    } catch (e) {
      if (code(e) === "ENOENT") continue; // closed since the listing
      throw e;
    }
    out.push(target.replace(/ \(deleted\)$/, ""));
  }
  const maps = await readFile(`/proc/${pid}/maps`, "utf8");
  for (const line of maps.split("\n")) {
    const at = line.indexOf("/");
    if (at >= 0) out.push(line.slice(at).replace(/ \(deleted\)$/, ""));
  }
  return out;
}

function macHeld(pid: number): string[] {
  if (testHooks === undefined) throw new Error("the held-files check needs a test-hooks build");
  return testHooks.heldPaths(pid);
}

async function windowsHeld(dir: string): Promise<string[]> {
  const probe = `${dir}~held`;
  try {
    await rename(dir, probe);
  } catch (e) {
    const c = code(e);
    if (c === "EBUSY" || c === "EPERM" || c === "EACCES") return [dir];
    throw e;
  }
  await rename(probe, dir);
  return [];
}

/**
 * The paths under `dir` the process `pid` holds open or mapped (sorted, without duplicates).
 * Throws when they cannot be read (a gone process included).
 */
export async function heldFiles(pid: number | undefined, dir: string, platform: NodeJS.Platform = process.platform): Promise<string[]> {
  if (pid === undefined) throw new Error("the worker's pid is unknown");
  if (platform === "win32") return windowsHeld(dir);
  const dirs = [dir, await realpath(dir).catch(() => dir)];
  let paths: string[];
  if (platform === "linux") paths = await linuxHeld(pid);
  else if (platform === "darwin") paths = macHeld(pid);
  else throw new Error(`no held-files check on ${platform}`);
  return [...new Set(paths.filter((p) => under(p, dirs)))].sort();
}
