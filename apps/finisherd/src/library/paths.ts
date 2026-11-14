// The paths the library RPCs take (plan 5D Task 13): relative to a library's root, `/`
// separated (`\` too under a Windows-style root), in any Unicode form. Nothing here touches a
// filesystem: a path becomes the components and lookup key the catalog is asked for, so `..`,
// `.`, a leading separator, a drive, a UNC prefix or a NUL are `INVALID_PATH` and nothing is
// ever opened from a user's string (files are opened from the catalogued raw chain).
import { FinError } from "@finisher/contracts";
import { foldKey } from "./locate.ts";
import type { LibraryRow } from "./registry.ts";

const invalidPath = (why: string): FinError =>
  new FinError({
    code: "INVALID_PATH",
    message: `the path is not usable: ${why}`,
    details: {},
    fix: "Give a path below the library's folder, without leading slashes, drive letters or .. parts.",
  });

const windowsRoot = (lib: Pick<LibraryRow, "root">): boolean => lib.root.includes("\\") || /^[A-Za-z]:/.test(lib.root);

/**
 * The components of `path` below the library's root; `""` is the root itself (no components).
 * `INVALID_PATH` for an absolute path, `.` or `..` components, an empty component in the
 * middle (`a//b` is fine: separators collapse) or a NUL.
 */
export function relComponents(lib: Pick<LibraryRow, "root">, path: string): string[] {
  if (path.includes("\0")) throw invalidPath("it holds a NUL");
  const win = windowsRoot(lib);
  if (path.startsWith("/") || path.startsWith("\\") || /^[A-Za-z]:/.test(path)) throw invalidPath("it is absolute");
  const parts = path.split(win ? /[\\/]+/ : /\/+/).filter((c) => c !== "");
  if (parts.some((c) => c === "." || c === "..")) throw invalidPath("it has . or .. parts");
  return parts.map((c) => c.normalize("NFC"));
}

/** The catalog's lookup key of a path's components (`relKey`: NFC, folded on a case-insensitive volume). */
export function relKeyOf(lib: Pick<LibraryRow, "volume">, components: ReadonlyArray<string>): string {
  return components.map((c) => foldKey(c, lib.volume.caseSensitive)).join("/");
}
