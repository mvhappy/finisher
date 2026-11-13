// When a process started, as the OS records it (plan 7D Decision 17): the orphan sweep
// (`agent/provider-pid.ts`) tells a recycled pid by it. It starts nothing for a session: on
// macOS it asks `ps` about one pid.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

/**
 * Linux: `/proc/<pid>/stat`'s start time (clock ticks since boot); macOS: `ps -o lstart=` (to
 * the second). Null when the process is gone or a zombie, and on Windows (TODO(7D Task 13/18):
 * the job object; nothing is swept there yet).
 */
export function processStart(pid: number, platform: NodeJS.Platform = process.platform): string | null {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  try {
    if (platform === "linux") {
      const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
      // After the command name (which may hold spaces and parentheses): the state is field 3, the start time field 22.
      const f = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
      return f[0] === "Z" || f[0] === "X" || f[19] === undefined ? null : `linux:${f[19]}`;
    }
    if (platform === "darwin") {
      const out = execFileSync("/bin/ps", ["-o", "stat=,lstart=", "-p", String(pid)], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] }).trim();
      return out === "" || out.startsWith("Z") ? null : `darwin:${out.slice(out.indexOf(" ") + 1).trim()}`;
    }
  } catch {
    // Gone (or `ps` found nothing).
  }
  return null;
}
