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
  /**
   * An agent session's project folder (plan 7A Task 8b review B1): under an agent credential the
   * only project folder `fin` trusts (a `--project` must name it), set by the runtime.
   */
  readonly FIN_PROJECT_DIR?: string | undefined;
  /**
   * The path of an agent or MCP credential file (spec §4.1; arch §8.4 as amended): a path, not
   * a secret, so it may be passed (5A Decision 34). `fin` in the child proves with it.
   */
  readonly FIN_CREDENTIAL?: string | undefined;
  /** The Finisher call id of one tool call (spec §12.3): the actor header's join, never a secret. */
  readonly FIN_CALL?: string | undefined;
}

/**
 * Never passed to a child, from the parent or from `extra` (sub-project 7 §8.5; plan 7A
 * Decisions 11 and 14b; plan 7C Task 11): server data (a child proves with its own credential,
 * never the local key), the author (the server sets an agent's), the training and diagnostics
 * switches (the server's own), and the variables that name an agent socket or a display (the
 * launcher strips its longer `SOCKET_ENV` list as a second layer). The allow-list already drops
 * them; the list makes the intent tested and holds even if the allow-list grows.
 */
export const NEVER_PASSED = [
  "FIN_SERVER_DATA",
  "FIN_AUTHOR",
  "FIN_TRAINING",
  "FIN_DIAGNOSTICS",
  // 5B: the server's own listener, certificate files and secret store.
  "FIN_LISTEN",
  "FIN_TLS_CERT",
  "FIN_TLS_KEY",
  "FIN_SECRETS",
  "FIN_SECRETS_KEY_FILE",
  "SSH_AUTH_SOCK",
  "GPG_AGENT_INFO",
  "DBUS_SESSION_BUS_ADDRESS",
  "DOCKER_HOST",
  "DISPLAY",
  "WAYLAND_DISPLAY",
] as const;

/** A provider secret by name (§8.5): every `*_API_KEY`, and Claude Code's own variables (`CLAUDE_CODE_OAUTH_TOKEN`). */
export const secretName = (key: string) => /_API_KEY$/i.test(key) || key === "CLAUDECODE" || key.startsWith("CLAUDE_CODE_");

/** Never passed through, even if a future allow-list change would admit them. */
function denied(key: string): boolean {
  return (NEVER_PASSED as ReadonlyArray<string>).includes(key) || key === "ELECTRON_RUN_AS_NODE" || secretName(key);
}

/**
 * Builds a child environment from the §27.7 allow-list: `PATH`, `HOME`, `LANG`, `LC_*`,
 * `TMPDIR` (plus the Windows system equivalents on win32), then `FIN_SERVER`, `FIN_SESSION`,
 * `FIN_PROJECT_DIR`, `FIN_CREDENTIAL` and `FIN_CALL` from `extra`. Everything else — notably `ELECTRON_RUN_AS_NODE`,
 * `CLAUDECODE`, `CLAUDE_CODE_*`, `*_API_KEY` and [`NEVER_PASSED`] — is dropped.
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
  if (extra.FIN_PROJECT_DIR !== undefined) env.FIN_PROJECT_DIR = extra.FIN_PROJECT_DIR;
  if (extra.FIN_CREDENTIAL !== undefined) env.FIN_CREDENTIAL = extra.FIN_CREDENTIAL;
  if (extra.FIN_CALL !== undefined) env.FIN_CALL = extra.FIN_CALL;
  return env;
}

/** The listed variables of `parent` that are set (a family's `env`, `jobs/kinds.ts`). */
export function passEnv(names: ReadonlyArray<string> | undefined, parent: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const env: Record<string, string> = {};
  for (const key of names ?? []) {
    const value = parent[key];
    if (value !== undefined && !denied(key)) env[key] = value;
  }
  return env;
}
