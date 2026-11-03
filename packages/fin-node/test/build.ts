// vitest globalSetup: builds the release napi binary the fin-node tests load.
import { execFileSync } from "node:child_process";

export default function setup(): void {
  execFileSync("pnpm", ["--filter", "@finisher/fin-node", "build"], {
    stdio: "inherit",
    shell: process.platform === "win32",
  });
}
