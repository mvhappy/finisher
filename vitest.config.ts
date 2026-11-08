import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["packages/*/src/**/*.test.ts", "packages/*/test/**/*.test.ts", "apps/*/test/**/*.test.ts"],
    globalSetup: ["packages/fin-node/test/build.ts"],
    // Windows starts processes slower (the spawn broker, Defender scanning each new image): under
    // the full suite, tests that start finisherd or workers passed alone but not in 5 s there
    // (2026-11-08 run). Elsewhere vitest's defaults.
    ...(process.platform === "win32" ? { testTimeout: 20_000, hookTimeout: 30_000 } : {}),
    env: {
      // fin-node tests load the `test-hooks` build, never the production binary.
      FIN_NODE_BINARY_DIR: fileURLToPath(new URL("packages/fin-node/test/native/", import.meta.url)),
    },
  },
});
