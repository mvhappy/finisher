// Whether a module is the process's entry point, through symlinks too (review #33: the package
// bin `finisherd` is a link to `src/main.ts`, which compared unresolved made `main()` never run).
import { realpathSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** `moduleUrl` (`import.meta.url`) and `argv1` (`process.argv[1]`) name one file, links resolved. */
export function isEntry(moduleUrl: string, argv1: string | undefined): boolean {
  if (argv1 === undefined || argv1 === "") return false;
  try {
    return realpathSync(fileURLToPath(moduleUrl)) === realpathSync(argv1);
  } catch {
    return false;
  }
}
