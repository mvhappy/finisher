// Regenerates schema/fin-core.schema.json (from fin-core) and src/generated.ts (from the schema).
// `--out <dir>` writes both files under <dir> instead of the package, for the drift gate.
// Deterministic: no timestamps, sorted keys, fixed banner. Needs no network.
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";

const pkg = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(pkg, "../..");
const i = process.argv.indexOf("--out");
const out = i === -1 ? pkg : resolve(process.argv[i + 1]);

const cargo = spawnSync("cargo", ["run", "-q", "-p", "fin-core", "--example", "export_schema"], {
  cwd: repo,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});
if (cargo.status !== 0) {
  process.stderr.write(cargo.stderr);
  process.exit(cargo.status ?? 1);
}

const banner =
  "/* eslint-disable */\n/**\n * Generated from schema/fin-core.schema.json by scripts/generate.mjs.\n * Do not edit by hand: change fin-core and run `pnpm --filter @finisher/core-types generate`.\n */";
const schema = JSON.parse(cargo.stdout);
const ts = await compile(schema, "FinCore", {
  bannerComment: banner,
  unreachableDefinitions: true,
  additionalProperties: false,
  cwd: pkg,
});

mkdirSync(join(out, "schema"), { recursive: true });
mkdirSync(join(out, "src"), { recursive: true });
writeFileSync(join(out, "schema/fin-core.schema.json"), cargo.stdout);
writeFileSync(join(out, "src/generated.ts"), ts);
