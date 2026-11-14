// Where the bundled Codex binary is (sub-project 7E plan Decision 1; spec §4.6).
//
// Finisher runs only the Codex it pins (`vendor/codex/pin.json`, fetched by `pnpm codex:pin`);
// there is no `agents.codex.path`. In order:
//  1. `FIN_CODEX_BIN`, **only in a `test-hooks` build of fin-node** (so `fake-codex` or a
//     fetched real binary can stand in); a release build never reads it;
//  2. `<resources>/codex/<target>/codex[.exe]`, beside `finisherd` (`FIN_RESOURCES_DIR`,
//     else the repository's `resources/`; sub-project 9 packages the binary there);
//  3. the dev cache `.cache/codex/<version>/<target>/codex[.exe]`.
// None of them: `PROVIDER_UNAVAILABLE {provider: "codex", reason: "notBundled"}`.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { testHooks } from "@finisher/fin-node";
import { finError } from "../../errors.ts";

export interface PinDeps {
  readonly env: Readonly<Record<string, string | undefined>>;
  /** Whether fin-node is a `test-hooks` build. */
  readonly testHooks: boolean;
  readonly platform: string;
  readonly arch: string;
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
    platform: process.platform,
    arch: process.arch,
    resourcesDir: process.env.FIN_RESOURCES_DIR || join(REPO, "resources"),
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
    if (!deps.exists(override)) throw notBundled(`FIN_CODEX_BIN names ${override}, which does not exist`);
    return override;
  }
  const target = codexTarget(deps.platform, deps.arch);
  if (target === undefined) throw notBundled(`no Codex build for ${deps.platform}-${deps.arch}`);
  const exe = deps.platform === "win32" ? "codex.exe" : "codex";
  for (const path of [join(deps.resourcesDir, "codex", target, exe), join(deps.cacheRoot, deps.version, target, exe)]) {
    if (deps.exists(path)) return path;
  }
  throw notBundled(`no ${exe} for ${target}`);
}
