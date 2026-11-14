// Where the CLI's own files are (plan 7D Decision 7, Task 14): the install tree a process reads,
// which the per-OS `toolchain-claudeCode.txt` never lists because it moves with the person's
// install. Recorded layout (Task 0, Linux, 2.1.321): one file `<data>/claude/versions/<v>` and
// `~/.local/bin/claude`, a link to it. So the grant comes from the **resolved** binary: the
// link's own directory (`~/.local/bin`) holds other tools and is not the CLI's.
//
// Only that layout grants a directory (`…/claude/versions`, where a newer version is a new file
// beside the old one). Any other binary is granted as the one file it is: the path is the
// person's setting, and its directory may be their home, `/`, a temp root or their Downloads.
// Even the layout's directory is refused when it is a filesystem root, or holds the home or a
// temp root. The sandbox refuses a toolchain root over the home or a denied path as well
// (`fin-sandbox`, profile `agent`), so a slip here fails the launch rather than widening it.
import { realpathSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { posix, win32 } from "node:path";

/** A version file of the native layout: `2.1.321`, `2.1.321-beta.1`, `2.1.321.exe`. */
const VERSION = /^\d+\.\d+\.\d+([-+.][0-9A-Za-z.-]*)?$/;

export interface InstallTreeOptions {
  /** The user's home (default `os.homedir()`, the sandbox's `home`). */
  readonly home?: string;
  /** The temp roots (default the OS's and `os.tmpdir()`). */
  readonly tmp?: readonly string[];
  /** Resolves links (default `realpathSync`; a binary it cannot resolve is granted as the one file). */
  readonly realpath?: (p: string) => string;
}

function tempRoots(platform: NodeJS.Platform): string[] {
  const own = platform === process.platform ? [tmpdir()] : [];
  if (platform === "win32") return own;
  return [...own, "/tmp", "/var/tmp", "/private/tmp", "/private/var/tmp", "/dev/shm"];
}

/** The read roots the CLI's process needs for its install, from the binary at `binary`. */
export function installTreeRead(binary: string, platform: NodeJS.Platform = process.platform, o: InstallTreeOptions = {}): string[] {
  const p = platform === "win32" ? win32 : posix;
  const fold = (s: string) => (platform === "win32" ? s.toLowerCase() : s);
  const resolve = o.realpath ?? realpathSync;
  let real: string;
  try {
    real = p.normalize(resolve(binary));
  } catch {
    // Unresolved: never promote to a directory (review of Task 14).
    return [p.normalize(binary)];
  }
  const dir = p.dirname(real);
  const layout = VERSION.test(p.basename(real)) && fold(p.basename(dir)) === "versions" && fold(p.basename(p.dirname(dir))) === "claude";
  // `a` is `b` or one of its ancestors. Folded on macOS too (its default volumes ignore case),
  // which only makes the guard stricter.
  const guardFold = (s: string) => (platform === "win32" || platform === "darwin" ? s.toLowerCase() : s);
  const holds = (a: string, b: string) => {
    const [x, y] = [guardFold(p.normalize(a)), guardFold(p.normalize(b))];
    return x === y || y.startsWith(x.endsWith(p.sep) ? x : x + p.sep);
  };
  // Each guarded root as given and resolved: a temp root or home that is a link must not hide
  // that the layout's directory holds it (review of Task 14: `TMPDIR=/alias` → `…/versions`).
  const given = [o.home ?? homedir(), ...(o.tmp ?? tempRoots(platform))].filter((g) => g !== "" && p.isAbsolute(g));
  const guarded = given.flatMap((g) => {
    try {
      return [g, p.normalize(resolve(g))];
    } catch {
      return [g];
    }
  });
  const broad = p.parse(dir).root === dir || guarded.some((g) => holds(dir, g));
  return [layout && !broad ? dir : real];
}
