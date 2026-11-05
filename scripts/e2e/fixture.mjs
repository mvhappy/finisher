import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT } from "./lib.mjs";

/** Writes a real 10 s MP4 with fin-probe's fixture writer (fin-node `test-hooks` build). */
export async function writeFixture(path) {
  process.env.FIN_NODE_BINARY_DIR = join(ROOT, "packages", "fin-node", "test", "native");
  const { testHooks } = await import(pathToFileURL(join(ROOT, "packages", "fin-node", "index.js")).href);
  if (!testHooks) throw new Error("fin-node test-hooks build is missing (pnpm --filter @finisher/fin-node build:test)");
  testHooks.writeFixtureMp4(path, { seconds: 10, fps: 30, audio: true, sampleRate: 48000 });
}

