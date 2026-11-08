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

function writeFailed(file: string, e: unknown): FinError {
  const cause = (e as { code?: unknown } | null)?.code;
  return finError(
    "SETTINGS_WRITE_FAILED",
    `${file} could not be written: ${(e as Error)?.message ?? String(e)}`,
    { file, ...(typeof cause === "string" ? { cause } : {}) },
    `check that ${file} and its folder are writable, then retry`,
  );
}

/**
 * Applies `patch` to `privacy` in `settings.json` and returns the new `privacy` section
 * (`undefined` when it ended up empty and was removed). `validate` checks the merged section
 * (the journal's own rules: `validatePrivacy`) before anything is written, so a change is
 * never stored beside a key the journal would refuse (it would keep the last good settings and
 * the change would not apply). Throws `SETTINGS_INVALID` when the file does not parse, is not an
 * object, its `privacy` is not an object, or the merged section is refused (naming the key);
 * `SETTINGS_WRITE_FAILED` when the write fails (the temp file is removed, the file unchanged).
 */
export function mergeSettings(
  serverData: string,
  patch: PrivacyPatch,
  validate: (privacy: Record<string, unknown> | undefined) => string | undefined = () => undefined,
): Record<string, unknown> | undefined {
  return patchSection(serverData, "privacy", patch, (privacy, file) => {
    const refused = validate(privacy);
    if (refused === undefined) return;
    const key = /^(privacy\.[A-Za-z]+)/.exec(refused)?.[1] ?? "privacy";
    throw finError("SETTINGS_INVALID", `${file} cannot be updated: ${refused}`, { file, key, reason: refused }, `fix or remove ${key} in ${file} by hand, then retry`);
  });
}

/**
 * Sets (or with `null` removes) keys of one top-level `section` of `settings.json`, keeping
 * every other key, and returns the new section (`undefined` once empty and removed). The rules
 * of `mergeSettings` hold: a file that does not parse is never overwritten (`SETTINGS_INVALID`),
 * the write is temp-then-rename, 0600, fsynced (`SETTINGS_WRITE_FAILED`). `check` may refuse
 * the merged section (throw) before anything is written. The agent settings (`agents.local`,
 * `training.closedModelTargets`; 7A Task 12) are written through it, by the owner only.
 */
export function patchSection(
  serverData: string,
  section: string,
  patch: Readonly<Record<string, unknown>>,
  check: (merged: Record<string, unknown> | undefined, file: string) => void = () => {},
): Record<string, unknown> | undefined {
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
  const current = settings[section];
  if (current !== undefined && (current === null || typeof current !== "object" || Array.isArray(current))) {
    throw invalid(file, `${section} is not an object`);
  }
  const merged: Record<string, unknown> = { ...((current as Record<string, unknown> | undefined) ?? {}) };
  for (const [key, value] of Object.entries(patch)) {
    if (value === null || value === undefined) delete merged[key];
    else merged[key] = value;
  }
  if (Object.keys(merged).length === 0) delete settings[section];
  else settings[section] = merged;
  check(settings[section] as Record<string, unknown> | undefined, file);

  const tmp = `${file}.${process.pid}.tmp`;
  try {
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
  } catch (e) {
    rmSync(tmp, { force: true });
    throw writeFailed(file, e);
  }
  return settings[section] as Record<string, unknown> | undefined;
}
