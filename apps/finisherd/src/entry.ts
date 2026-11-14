// Whether a module is the process's entry point, through symlinks too (review #33: the package
// bin `finisherd` is a link to `src/main.ts`, which compared unresolved made `main()` never run).
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

/**
 * `p` without Windows' verbatim prefix (`\\?\C:\…`, `\\?\UNC\host\…`): Rust's `canonicalize`
 * gives one, Node keeps it through `realpathSync`, and a verbatim path is never normalised, so
 * `\\?\C:\repo\apps/finisherd/src/main.ts` would never equal the module's own path.
 */
export function withoutVerbatim(p: string, platform: NodeJS.Platform = process.platform): string {
  if (platform !== "win32") return p;
  if (p.startsWith("\\\\?\\UNC\\")) return `\\\\${p.slice(8)}`;
  if (p.startsWith("\\\\?\\")) return p.slice(4);
  return p;
}

const real = (p: string): string => withoutVerbatim(realpathSync(withoutVerbatim(p)));

/** `moduleUrl` (`import.meta.url`) and `argv1` (`process.argv[1]`) name one file, links resolved. */
export function isEntry(moduleUrl: string, argv1: string | undefined): boolean {
  if (argv1 === undefined || argv1 === "") return false;
  try {
    return real(fileURLToPath(moduleUrl)) === real(argv1);
  } catch {
    return false;
  }
}
