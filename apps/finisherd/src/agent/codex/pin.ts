// Where the bundled Codex binary is (sub-project 7E plan Decision 1; spec §4.6).
//
// Finisher runs only the Codex it pins (`vendor/codex/pin.json`, fetched by `pnpm codex:pin`);
// there is no `agents.codex.path`. In order:
//  1. `FIN_CODEX_BIN`, **only in a `test-hooks` build of fin-node** (so `fake-codex` or a
//     fetched real binary can stand in); a release build never reads it;
//  2. `<resources>/codex/<target>/codex[.exe]`, where `<resources>` is the installation's
//     `resources/`, found from this module's own location (never the working directory, never the
//     environment; sub-project 9 packages the binary there);
//  3. the dev cache `.cache/codex/<version>/<target>/codex[.exe]` (beside the module tree too).
// None of them: `PROVIDER_UNAVAILABLE {provider: "codex", reason: "notBundled"}`.
//
// **What an installed build trusts** (review of Task 2): only (2). `FIN_RESOURCES_DIR` (which
// replaces (2)'s directory, and must be absolute) and the dev cache (3) are read only when the
// run is a development one: fin-node is a `test-hooks` build, or the module tree is a repository
// checkout (a `.git` entry at its root, which an installation never has). So neither variable
// redirects an installed build — but this rests on fin-node's own `testHooks`, and fin-node
// loads its native binding from `FIN_NODE_BINARY_DIR` in any build (a `test-hooks` binding
// there turns (1) on). That variable, like `NODE_OPTIONS`, already means arbitrary native code
// in finisherd; gating it is a follow-up (TASKS.md 7E). `REPO` below assumes the installed tree
// keeps this module five levels under the root that holds `resources/` and `vendor/codex/`
// (sub-project 9's packaging must keep it, or pass the root in).
import { existsSync, readFileSync } from "node:fs";
import { isAbsolute, join } from "node:path";
import { fileURLToPath } from "node:url";
import { testHooks } from "@finisher/fin-node";
import { finError } from "../../errors.ts";

export interface PinDeps {
  readonly env: Readonly<Record<string, string | undefined>>;
  /** Whether fin-node is a `test-hooks` build. */
  readonly testHooks: boolean;
  /** Whether the module tree is a repository checkout (not an installation). */
  readonly devCheckout: boolean;
  readonly platform: string;
  readonly arch: string;
  /** The installation's `resources/`, beside this module tree (never cwd- or env-derived). */
  readonly resourcesDir: string;
  /** The dev cache's root (`.cache/codex`). */
  readonly cacheRoot: string;
  /** The pinned version (`vendor/codex/pin.json`). */
  readonly version: string;
  readonly exists: (path: string) => boolean;
}

const TARGETS = new Set(["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "win32-x64", "win32-arm64"]);

/** The npm suffix of a platform's package (`linux-x64`), or `undefined` where Codex is not bundled. */
export function codexTarget(platform: string, arch: string): string | undefined {
  const t = `${platform}-${arch}`;
  return TARGETS.has(t) ? t : undefined;
}

const REPO = fileURLToPath(new URL("../../../../../", import.meta.url));

/** The pinned version, read from the committed `vendor/codex/pin.json`. */
export function pinnedCodexVersion(): string {
  return (JSON.parse(readFileSync(join(REPO, "vendor/codex/pin.json"), "utf8")) as { version: string }).version;
}

export function defaultPinDeps(): PinDeps {
  return {
    env: process.env,
    testHooks: testHooks !== undefined,
    devCheckout: existsSync(join(REPO, ".git")),
    platform: process.platform,
    arch: process.arch,
    resourcesDir: join(REPO, "resources"),
    cacheRoot: join(REPO, ".cache/codex"),
    version: pinnedCodexVersion(),
    exists: existsSync,
  };
}

const notBundled = (why: string) =>
  finError(
    "PROVIDER_UNAVAILABLE",
    `Codex is not bundled with this build: ${why}`,
    { provider: "codex", reason: "notBundled" },
    "install a Finisher build that includes Codex (a development checkout runs `pnpm codex:pin`)",
  );

/** The path of the bundled `codex` executable; throws `PROVIDER_UNAVAILABLE {reason: notBundled}`. */
export function codexBinary(deps: PinDeps = defaultPinDeps()): string {
  const override = deps.testHooks ? deps.env.FIN_CODEX_BIN : undefined;
  if (override !== undefined && override !== "") {
    if (!isAbsolute(override)) throw notBundled(`FIN_CODEX_BIN must be an absolute path, not ${override}`);
    if (!deps.exists(override)) throw notBundled(`FIN_CODEX_BIN names ${override}, which does not exist`);
    return override;
  }
  const target = codexTarget(deps.platform, deps.arch);
  if (target === undefined) throw notBundled(`no Codex build for ${deps.platform}-${deps.arch}`);
  const exe = deps.platform === "win32" ? "codex.exe" : "codex";
  const dev = deps.testHooks || deps.devCheckout;
  let resources = deps.resourcesDir;
  const named = dev ? deps.env.FIN_RESOURCES_DIR : undefined;
  if (named !== undefined && named !== "") {
    if (!isAbsolute(named)) throw notBundled(`FIN_RESOURCES_DIR must be an absolute path, not ${named}`);
    resources = named;
  }
  const paths = [join(resources, "codex", target, exe), ...(dev ? [join(deps.cacheRoot, deps.version, target, exe)] : [])];
  for (const path of paths) {
    if (deps.exists(path)) return path;
  }
  throw notBundled(`no ${exe} for ${target}`);
}
