import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["packages/*/src/**/*.test.ts", "packages/*/test/**/*.test.ts"],
    globalSetup: ["packages/fin-node/test/build.ts"],
    env: {
      // fin-node tests load the `test-hooks` build, never the production binary.
      FIN_NODE_BINARY_DIR: fileURLToPath(new URL("packages/fin-node/test/native/", import.meta.url)),
    },
  },
});
