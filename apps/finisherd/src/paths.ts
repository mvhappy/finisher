// Paths clients send (plan 5A Decision 35): `project.create.dir`, `project.open.dir`,
// `asset.probe.path` and `project.recover.importDir` must be absolute and normalised, or the
// request answers `INVALID_PATH {field, path}` (exit 3). The server never resolves a path
// against its own working directory, and `.`/`..` segments never reach a check (Task 5's
// resolvers call this before `realpath`).
//
// - POSIX: starts with `/`; no `.` or `..` segment and no doubled `/` (a trailing `/` is fine).
// - Windows: a drive (`C:\…`, `\\?\C:\…`) or UNC (`\\server\share\…`, `\\?\UNC\server\share\…`)
//   root; drive-relative (`C:foo`), rooted-without-drive (`\foo`) and device-namespace
//   (`\\.\…`, `\\?\GLOBALROOT\…`) forms are refused; `/` and `\` both separate; no `.` or `..`
//   segment, no doubled separator, no `:` after the drive (alternate data streams), no
//   reserved device name (`CON`, `NUL.txt`, `COM1`, …) and no segment ending in a dot or space.
import { posix } from "node:path";
import type { FinError } from "@finisher/contracts";
import { Effect } from "effect";
import { finError } from "./errors.ts";

function normalisedPosix(p: string): boolean {
  if (!posix.isAbsolute(p)) return false;
  const n = posix.normalize(p);
  return n === p || `${n}/` === p;
}

/** Reserved DOS device names: a segment named so (with any extension) opens the device. */
const RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;

/** A path segment Windows would read as written: no `:` (streams, drives), no device name, no trailing dot or space. */
function plainWindowsSegment(seg: string): boolean {
  if (seg.length === 0 || seg === "." || seg === "..") return false;
  if (seg.includes(":")) return false;
  if (seg.endsWith(".") || seg.endsWith(" ")) return false;
  const stem = seg.split(".")[0]!.trimEnd();
  return !RESERVED.test(stem);
}

/**
 * The root and the rest: `C:\`, `\\?\C:\`, `\\server\share`, `\\?\UNC\server\share`.
 * Device namespaces (`\\.\…`, `\\?\GLOBALROOT\…`, any other `\\?\`) have no root here.
 */
function windowsRoot(q: string): { rest: string; share?: [string, string] } | undefined {
  const drive = /^(?:\\\\\?\\)?[A-Za-z]:\\/.exec(q);
  if (drive) return { rest: q.slice(drive[0].length) };
  const unc = /^(?:\\\\\?\\UNC\\|\\\\(?![.?]\\))([^\\]+)\\([^\\]+)(?:\\|$)/i.exec(q);
  if (unc) return { rest: q.slice(unc[0].length), share: [unc[1]!, unc[2]!] };
  return undefined;
}

function normalisedWindows(p: string): boolean {
  const q = p.replaceAll("/", "\\");
  const root = windowsRoot(q);
  if (root === undefined) return false;
  if (root.share !== undefined && !root.share.every(plainWindowsSegment)) return false;
  if (root.rest.length === 0) return true;
  const segments = root.rest.split("\\");
  // One trailing separator is fine; any other empty segment is a doubled one.
  if (segments[segments.length - 1] === "") segments.pop();
  return segments.every(plainWindowsSegment);
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
