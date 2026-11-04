// vitest globalSetup: builds the release napi binary with the `test-hooks` feature into
// packages/fin-node/test/native/; vitest.config.ts points FIN_NODE_BINARY_DIR there.
import { execFileSync } from "node:child_process";

export default function setup(): void {
  execFileSync("pnpm", ["--filter", "@finisher/fin-node", "build:test"], {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
}
