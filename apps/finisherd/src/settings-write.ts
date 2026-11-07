// The settings writer (plan D, Decision 8): finisherd is the only program that writes
// `<server-data>/settings.json`. It changes only the patched keys under `privacy` and keeps
// every other key (unknown ones and `fonts` included), writing temp-then-rename (0600,
// fsynced). A file that does not parse is never overwritten: a hand edit is not lost.
import { closeSync, fsyncSync, openSync, readFileSync, renameSync, rmSync, writeSync } from "node:fs";
import { dirname } from "node:path";
import type { FinError } from "@finisher/contracts";
import { settingsPath } from "./config.ts";
import { finError } from "./errors.ts";

/** One purpose's value: set, or `null` to remove the key (back to the default). */
export type PrivacyPatch = { readonly [key in "diagnostics" | "training"]?: "on" | "off" | null };

function invalid(file: string, why: string): FinError {
  return finError(
    "SETTINGS_INVALID",
    `${file} cannot be updated: ${why}`,
    { file, reason: why },
    `fix or remove ${file} by hand, then retry`,
  );
}

/** fsyncs a directory where that is possible (not on Windows). */
function syncDir(dir: string): void {
  if (process.platform === "win32") return;
  const fd = openSync(dir, "r");
  try {
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

/**
 * Applies `patch` to `privacy` in `settings.json` and returns the new `privacy` section
 * (`undefined` when it ended up empty and was removed). Throws `SETTINGS_INVALID` when the
 * file does not parse, is not an object, or its `privacy` is not an object.
 */
export function mergeSettings(serverData: string, patch: PrivacyPatch): Record<string, unknown> | undefined {
  const file = settingsPath(serverData);
  let raw: unknown = {};
  let text: string | undefined;
  try {
    text = readFileSync(file, "utf8");
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw invalid(file, String(e));
  }
  if (text !== undefined) {
    try {
      raw = JSON.parse(text);
    } catch (e) {
      throw invalid(file, `it is not valid JSON (${(e as Error).message})`);
    }
  }
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) throw invalid(file, "it is not a JSON object");
  const settings = { ...(raw as Record<string, unknown>) };
  const current = settings.privacy;
  if (current !== undefined && (current === null || typeof current !== "object" || Array.isArray(current))) {
    throw invalid(file, "privacy is not an object");
  }
  const privacy: Record<string, unknown> = { ...((current as Record<string, unknown> | undefined) ?? {}) };
  for (const [key, value] of Object.entries(patch)) {
    if (value === null || value === undefined) delete privacy[key];
    else privacy[key] = value;
  }
  if (Object.keys(privacy).length === 0) delete settings.privacy;
  else settings.privacy = privacy;

  const tmp = `${file}.${process.pid}.tmp`;
  rmSync(tmp, { force: true });
  const fd = openSync(tmp, "wx", 0o600);
  try {
    writeSync(fd, `${JSON.stringify(settings, null, 2)}\n`);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  renameSync(tmp, file);
  syncDir(dirname(file));
  return settings.privacy as Record<string, unknown> | undefined;
}
