// Paths clients send (plan 5A Decision 35): `project.create.dir`, `project.open.dir`,
// `asset.probe.path` and `project.recover.importDir` must be absolute and normalised, or the
// request answers `INVALID_PATH {field, path}` (exit 3). The server never resolves a path
// against its own working directory, and `.`/`..` segments never reach a check (Task 5's
// resolvers call this before `realpath`).
//
// - POSIX: starts with `/`; no `.` or `..` segment and no doubled `/` (a trailing `/` is fine).
// - Windows: a drive (`C:\…`) or UNC (`\\server\share\…`, `\\?\…`) root; drive-relative
//   (`C:foo`) and rooted-without-drive (`\foo`) forms are refused; `/` and `\` both separate;
//   no `.` or `..` segment and no doubled separator after the root.
import { posix, win32 } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "./errors.ts";

function normalisedPosix(p: string): boolean {
  if (!posix.isAbsolute(p)) return false;
  const n = posix.normalize(p);
  return n === p || `${n}/` === p;
}

const DRIVE = /^[A-Za-z]:\\/;
const UNC = /^\\\\[^\\]+\\[^\\]+/;

function normalisedWindows(p: string): boolean {
  const q = p.replaceAll("/", "\\");
  if (!DRIVE.test(q) && !UNC.test(q)) return false;
  const n = win32.normalize(q);
  return n === q || `${n}\\` === q || n === `${q}\\`;
}

/** True when `path` is absolute and normalised for `platform`. */
export function isAbsoluteNormalised(path: string, platform: NodeJS.Platform = process.platform): boolean {
  if (path.length === 0 || path.includes("\0")) return false;
  return platform === "win32" ? normalisedWindows(path) : normalisedPosix(path);
}

/** `path` itself, or `INVALID_PATH {field, path}` when it is relative or not normalised. */
export function requireAbsolute(field: string, path: string, platform: NodeJS.Platform = process.platform): Effect.Effect<string, FinError> {
  return isAbsoluteNormalised(path, platform)
    ? Effect.succeed(path)
    : Effect.fail(
        finError(
          "INVALID_PATH",
          `${field} must be an absolute path without . or .. segments`,
          { field, path },
          "send the full path from the root of the file system, with no . or .. segments",
        ),
      );
}

/** `requireAbsolute` for an optional field: absent stays absent. */
export function requireAbsoluteOpt(field: string, path: string | undefined, platform?: NodeJS.Platform): Effect.Effect<string | undefined, FinError> {
  return path === undefined ? Effect.succeed(undefined) : requireAbsolute(field, path, platform);
}
