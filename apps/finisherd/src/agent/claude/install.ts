// Where the CLI's own files are (plan 7D Decision 7, Task 14): the install tree a process reads,
// which the per-OS `toolchain-claudeCode.txt` never lists because it moves with the person's
// install. Recorded layout (Task 0, Linux, 2.1.321): one file `<data>/claude/versions/<v>` and
// `~/.local/bin/claude`, a link to it. So the directory to grant is the **resolved** binary's:
// the link's own directory (`~/.local/bin`) holds other tools and is not the CLI's.
import { posix, win32 } from "node:path";

/** The read roots the CLI's process needs for its install: the directory of `real`, links resolved. */
export function installTreeRead(real: string, platform: NodeJS.Platform = process.platform): string[] {
  return [(platform === "win32" ? win32 : posix).dirname(real)];
}
