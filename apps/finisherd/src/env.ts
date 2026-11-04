// Child-process environment allow-list (spec §27 item 7). Every child that finisherd
// spawns (workers, agents, plugins) must start from `childEnv(...)`, never `process.env`.

/** Variables copied verbatim from the parent environment when present. */
const ALLOWED = ["PATH", "HOME", "LANG", "TMPDIR"] as const;

/** Windows spells some of these differently; keep the case-insensitive equivalents. */
const ALLOWED_WINDOWS = ["Path", "USERPROFILE", "SYSTEMROOT", "SystemRoot", "TEMP", "TMP", "LOCALAPPDATA"] as const;

/** Keys the caller may add on top of the allow-list. */
export interface ChildEnvExtra {
  readonly FIN_SERVER?: string | undefined;
  readonly FIN_SESSION?: string | undefined;
}

/** Never passed through, even if a future allow-list change would admit them. */
function denied(key: string): boolean {
  return key === "ELECTRON_RUN_AS_NODE" || key === "CLAUDECODE" || key.startsWith("CLAUDE_CODE_");
}

/**
 * Builds a child environment from the §27.7 allow-list: `PATH`, `HOME`, `LANG`, `LC_*`,
 * `TMPDIR` (plus the Windows system equivalents on win32), then `FIN_SERVER` and
 * `FIN_SESSION` from `extra`. Everything else — notably `ELECTRON_RUN_AS_NODE`,
 * `CLAUDECODE` and `CLAUDE_CODE_*` — is dropped.
 */
export function childEnv(
  extra: ChildEnvExtra = {},
  parent: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): Record<string, string> {
  const allowed = new Set<string>(platform === "win32" ? [...ALLOWED, ...ALLOWED_WINDOWS] : ALLOWED);
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(parent)) {
    if (value === undefined || denied(key)) continue;
    if (allowed.has(key) || key.startsWith("LC_")) env[key] = value;
  }
  if (extra.FIN_SERVER !== undefined) env.FIN_SERVER = extra.FIN_SERVER;
  if (extra.FIN_SESSION !== undefined) env.FIN_SESSION = extra.FIN_SESSION;
  return env;
}
