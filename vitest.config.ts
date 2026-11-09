import { existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const strict = process.env.FIN_STRICT_TIMING === "1";
// Tests make a server data in a temp dir, and its `run/fin.sock` must fit a socket path (103
// bytes on macOS): macOS's per-user `TMPDIR` (`/var/folders/…/T`, 48 bytes) leaves too little
// for a test's nested dirs, so they go under `/tmp` there, as the e2e scenarios do
// (`scripts/e2e/lib.mjs` `scenarioTmp`). `/tmp` is a link too, so resolved paths still differ.
const shortTmp = process.platform === "darwin" && Buffer.byteLength(tmpdir()) > 40 && existsSync("/tmp");

export default defineConfig({
  test: {
    // Every test starts a process, a database or a server somewhere (many spawn `finisherd`); at
    // load average 30 vitest's 5 s default fails tests that take 1 s alone. A timeout only guards
    // against a hang, so it is generous unless `FIN_STRICT_TIMING=1` (a quiet machine); a test
    // that needs longer (or wants a shorter guard) says so itself.
    testTimeout: strict ? 5_000 : 30_000,
    hookTimeout: strict ? 10_000 : 60_000,
    include: ["packages/*/src/**/*.test.ts", "packages/*/test/**/*.test.ts", "apps/*/test/**/*.test.ts"],
    globalSetup: ["packages/fin-node/test/build.ts"],
    env: {
      // fin-node tests load the `test-hooks` build, never the production binary.
      FIN_NODE_BINARY_DIR: fileURLToPath(new URL("packages/fin-node/test/native/", import.meta.url)),
      ...(shortTmp ? { TMPDIR: "/tmp" } : {}),
      // Servers under test keep their secrets in files, never in the OS keychain (5B Decision 4).
      FIN_SECRETS: "file",
    },
  },
});
