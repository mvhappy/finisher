import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const strict = process.env.FIN_STRICT_TIMING === "1";

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
    },
  },
});
