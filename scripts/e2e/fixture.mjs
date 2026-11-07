import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { ROOT } from "./lib.mjs";

/** The fin-node `test-hooks` build (fin-probe's fixture writers, generated test fonts). */
export const TEST_NATIVE_DIR = join(ROOT, "packages", "fin-node", "test", "native");

async function hooks() {
  process.env.FIN_NODE_BINARY_DIR = TEST_NATIVE_DIR;
  const { testHooks } = await import(pathToFileURL(join(ROOT, "packages", "fin-node", "index.js")).href);
  if (!testHooks) throw new Error("fin-node test-hooks build is missing (pnpm --filter @finisher/fin-node build:test)");
  return testHooks;
}

/** Writes a real MP4 (30 fps, 48 kHz stereo audio; 10 s unless `seconds`) with fin-probe's fixture writer. */
export async function writeFixture(path, seconds = 10) {
  (await hooks()).writeFixtureMp4(path, { seconds, fps: 30, audio: true, sampleRate: 48000 });
}

/** Writes a fixture of another kind (`png`, `dotlottie`, `wav:<seconds>`, …; see fin-node's `testHooks.writeFixture`). */
export async function writeFixtureOf(kind, path) {
  (await hooks()).writeFixture(kind, path);
}

/** A generated test font's bytes (`sans`, `rtl`, `han`, `latinBold`). */
export async function testFont(name) {
  return (await hooks()).testFont(name);
}
