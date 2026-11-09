// The `node` runtime of code plugins (sp8 spec §3.2; sp8b plan Task 4): finisherd's own Node
// (`process.execPath`) under its permission model, one path per flag. The model is a seat
// belt, not the sandbox (7C's `plugin` profile is, once it is enforced): a plugin reads its
// package and its temp, writes only its temp, and gets no child processes, addons, workers
// or WASI. Under Electron the child runs as plain Node (`ELECTRON_RUN_AS_NODE=1`, for that
// child only).
//
// Residual before 7C: Node 24.22's permission model has no network control, so a `node` plugin
// can open TCP and unix sockets until 7C's `plugin` profile applies. Owner decision 1 accepts
// plugins running unconfined until then; finisherd's local socket still needs the key file,
// which the child cannot read.
import { finError } from "../../errors.ts";

/** Flags a plugin process never gets. */
export const DENIED_NODE_FLAGS = ["--allow-child-process", "--allow-addons", "--allow-worker", "--allow-wasi"] as const;

export interface NodeArgv {
  /** The package directory (read). */
  readonly pluginDir: string;
  /** The bundled `.mjs`, inside `pluginDir`. */
  readonly main: string;
  /** The key's private temp (read and write). */
  readonly tmp: string;
  /**
   * The per-project data directory when `fs pluginData` is granted (spec §5.1): not granted
   * before 8B Task 16 (7C's mapping accepts it under `plugins/data/` only from then on).
   */
  readonly data?: { readonly path: string; readonly write: boolean };
}

/**
 * A path as one `--allow-fs-*` value: Node expands `*` in it (a wildcard would widen the grant)
 * and no longer splits on `,` (it warns), so both are refused.
 */
function flagPath(path: string): string {
  if (path.includes("*") || path.includes(",")) {
    throw finError("PLUGIN_RUNTIME_UNAVAILABLE", "a plugin path holds a character Node's permission flags cannot take", { runtime: "node", reason: "pathCharacter" });
  }
  return path;
}

/** The arguments before the engine's `--warm`: the permission flags, then `main`. */
export function nodeArgs(a: NodeArgv): string[] {
  const args = ["--permission", `--allow-fs-read=${flagPath(a.pluginDir)}`];
  if (a.data !== undefined) {
    args.push(`--allow-fs-read=${flagPath(a.data.path)}`);
    if (a.data.write) args.push(`--allow-fs-write=${flagPath(a.data.path)}`);
  }
  args.push(`--allow-fs-read=${flagPath(a.tmp)}`, `--allow-fs-write=${flagPath(a.tmp)}`, a.main);
  return args;
}

/** What the `node` runtime adds to the child's environment: `ELECTRON_RUN_AS_NODE=1` under Electron only. */
export function nodeRuntimeEnv(versions: NodeJS.ProcessVersions | Readonly<Record<string, string | undefined>> = process.versions): Record<string, string> {
  return (versions as Readonly<Record<string, string | undefined>>)["electron"] === undefined ? {} : { ELECTRON_RUN_AS_NODE: "1" };
}
