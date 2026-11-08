// The warm contract's check (5C-1 plan Decisions 6, 36), in test-hooks builds only: after a warm
// worker's `ready`, the paths under the job directory it still holds open or mapped. Linux reads
// `/proc/<pid>/fd` and `/proc/<pid>/maps`; macOS lists the process's files with `lsof` (test
// builds only: `finisherd` itself may start processes, workers may not); Windows renames the
// job directory and back, which a handle held inside it makes fail.
import { execFile } from "node:child_process";
import { readdir, readFile, readlink, realpath, rename } from "node:fs/promises";
import { join, sep } from "node:path";

/** Whether `path` is `dir` or lies under one of `dirs`. */
function under(path: string, dirs: ReadonlyArray<string>): boolean {
  return dirs.some((d) => path === d || path.startsWith(d.endsWith(sep) ? d : d + sep));
}

async function linuxHeld(pid: number): Promise<string[]> {
  const out: string[] = [];
  const fdDir = `/proc/${pid}/fd`;
  for (const fd of await readdir(fdDir).catch(() => [] as string[])) {
    const target = await readlink(join(fdDir, fd)).catch(() => undefined);
    if (target !== undefined) out.push(target.replace(/ \(deleted\)$/, ""));
  }
  const maps = await readFile(`/proc/${pid}/maps`, "utf8").catch(() => "");
  for (const line of maps.split("\n")) {
    const at = line.indexOf("/");
    if (at >= 0) out.push(line.slice(at).replace(/ \(deleted\)$/, ""));
  }
  return out;
}

function macHeld(pid: number): Promise<string[]> {
  return new Promise((resolve) => {
    execFile("lsof", ["-n", "-P", "-Fn", "-p", String(pid)], { timeout: 5000 }, (_err, stdout) => {
      resolve(
        String(stdout ?? "")
          .split("\n")
          .filter((l) => l.startsWith("n/"))
          .map((l) => l.slice(1)),
      );
    });
  });
}

async function windowsHeld(dir: string): Promise<string[]> {
  const probe = `${dir}~held`;
  try {
    await rename(dir, probe);
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    return code === "EBUSY" || code === "EPERM" || code === "EACCES" ? [dir] : [];
  }
  await rename(probe, dir);
  return [];
}

/** The paths under `dir` the process `pid` holds open or mapped (sorted, without duplicates). */
export async function heldFiles(pid: number | undefined, dir: string, platform: NodeJS.Platform = process.platform): Promise<string[]> {
  if (pid === undefined) return [];
  if (platform === "win32") return windowsHeld(dir);
  const dirs = [dir, await realpath(dir).catch(() => dir)];
  const paths = platform === "linux" ? await linuxHeld(pid) : platform === "darwin" ? await macHeld(pid) : [];
  return [...new Set(paths.filter((p) => under(p, dirs)))].sort();
}
