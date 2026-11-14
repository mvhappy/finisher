// Builds `dist/plugin.mjs` (the manifest's `runtime.main`): `src/plugin.ts` and the SDK joined
// into ONE ES module that imports only `node:` builtins. `dist/` is git-ignored; the SDK's tests
// run this before they spawn the plugin. The repository uses the SDK's dependency-free bundler;
// a plugin of your own uses esbuild (packages/plugin-sdk/README.md).
//   node examples/plugins/ts-tools/build.mjs
import { fileURLToPath } from "node:url";
import { bundle } from "../../../packages/plugin-sdk/tools/bundle.mjs";

const here = (p) => fileURLToPath(new URL(p, import.meta.url));

export const OUTFILE = here("./dist/plugin.mjs");

export function build() {
  bundle({
    entry: here("./src/plugin.ts"),
    outfile: OUTFILE,
    aliases: { "@finisher/plugin-sdk": here("../../../packages/plugin-sdk/src/index.ts") },
  });
  return OUTFILE;
}

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === process.argv[1]) console.log(build());
