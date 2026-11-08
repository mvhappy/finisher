// Server paths in errors (8A Task 9b review, L2). fin-registry's I/O errors, a corrupt
// `registry.json` and a registry that cannot be opened name absolute paths under the server
// data or the registry's folder. Only a person (`human` principal) is told where those are
// (owner decision, 2026-11-08): everyone else gets the code, fixed words and the details that
// name no location.
import type { FinError } from "@finisher/contracts";
import { finError } from "../errors.ts";

/** An absolute POSIX path of two or more parts, a drive path or a UNC path inside a text. */
const PATH_LIKE = /(?:^|[\s"'(=:,])(?:\/[\w.@~+-]+(?:\/[\w.@~+-]*)+|[A-Za-z]:[\\/][^\s"']*|\\\\[\w.$-]+\\)/;
/** Detail keys that hold locations whatever their value looks like. */
const PATH_KEYS = new Set(["path", "dir", "file", "archive", "registryPath", "folder"]);

export const hasPath = (text: string): boolean => PATH_LIKE.test(text);

function cleanValue(v: unknown): unknown {
  if (typeof v === "string") return hasPath(v) ? undefined : v;
  if (Array.isArray(v)) return v.map(cleanValue).filter((x) => x !== undefined);
  if (v !== null && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
      if (PATH_KEYS.has(k)) continue;
      const c = cleanValue(x);
      if (c !== undefined) out[k] = c;
    }
    return out;
  }
  return v;
}

/** The words an error that named a location gets instead. */
export const scrubbedMessage = (code: string): string => `${code}: the server could not complete this; where it failed is shown only to the server's owner`;

/** `e` without server locations: its code stays. */
export function scrubParts(e: { readonly code: string; readonly message: string; readonly details?: unknown; readonly fix?: string | undefined }): { code: string; message: string; details: unknown; fix?: string } {
  const details = cleanValue(e.details ?? {}) ?? {};
  const message = hasPath(e.message) ? scrubbedMessage(e.code) : e.message;
  const fix = e.fix !== undefined && !hasPath(e.fix) ? e.fix : undefined;
  return { code: e.code, message, details, ...(fix === undefined ? {} : { fix }) };
}

export function scrubError(e: FinError): FinError {
  const p = scrubParts(e);
  return finError(p.code, p.message, p.details, p.fix);
}
