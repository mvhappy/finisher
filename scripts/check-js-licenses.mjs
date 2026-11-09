import { readFileSync } from "node:fs";
import { report } from "./walk.mjs";
import { discover, violations } from "./js-licenses.mjs";

function arg(name) {
  const i = process.argv.indexOf(name);
  return i === -1 ? undefined : process.argv[i + 1];
}

const input = arg("--input");
const json = input ? readFileSync(input, "utf8") : discover();
const allowlist = JSON.parse(readFileSync(arg("--allowlist") ?? new URL("./license-allowlist.json", import.meta.url), "utf8"));

try {
  report(violations(JSON.parse(json), allowlist));
} catch (e) {
  report([e.message]);
}
