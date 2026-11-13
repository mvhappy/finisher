// Regenerates schema/fin-core.schema.json (fin-core's types plus the envelope types of fin-store and
// fin-probe: fin-store's `export_schema` example), src/generated/index.ts (from the schema) and the
// contracts' Effect schemas of the RPC results (`packages/contracts/src/generated/schemas.ts`, by
// `src/codegen/effect.ts`). `--out <dir>` writes all three under <dir> (the contracts file at
// `<dir>/../contracts/...`) instead of the packages, for the drift gate.
// Deterministic: no timestamps, sorted keys, fixed banner. Needs no network.
import { spawnSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { compile } from "json-schema-to-typescript";
import { effectSchemas } from "../src/codegen/effect.ts";
import { forTypeScript } from "../src/codegen/jstt-input.ts";

const pkg = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(pkg, "../..");
const i = process.argv.indexOf("--out");
const out = i === -1 ? pkg : resolve(process.argv[i + 1]);

/**
 * The RPC results contracts validate with generated Effect schemas (with what they reference),
 * and the definitions passed through unvalidated: `CommittedTx` (the record, its `Tx`, ops and
 * diff: ~100 fin-core model definitions that fin-core has validated on commit; the contracts
 * never decode one) stays `Schema.Unknown`, typed as the generated `CommittedTx`.
 */
const ROOTS = [
  "ProjectStatus",
  "ProjectStatusView",
  "VerifyReport",
  "RebuildReport",
  "CompactReport",
  "BranchCreated",
  "BranchView",
  "Conflict",
  "HistoryLog",
  "HistoryResult",
  "MergeReport",
  "RecoverReport",
  "Probe",
  // 7F: the hook protocol (the contracts' `HookEvent` is this generated schema).
  "HookEvent",
  "HookReply",
  "HookRequest",
];
const OPAQUE = ["CommittedTx"];

const cargo = spawnSync("cargo", ["run", "-q", "-p", "fin-store", "--example", "export_schema"], {
  cwd: repo,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});
if (cargo.status !== 0) {
  process.stderr.write(cargo.stderr);
  process.exit(cargo.status ?? 1);
}

const banner =
  "/* eslint-disable */\n/**\n * Generated from schema/fin-core.schema.json by scripts/generate.mjs.\n * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.\n */";
const schema = JSON.parse(cargo.stdout);
const ts = await compile(forTypeScript(schema), "FinCore", {
  bannerComment: banner,
  unreachableDefinitions: true,
  additionalProperties: false,
  cwd: pkg,
});
const effect = effectSchemas(schema, {
  roots: ROOTS,
  opaque: OPAQUE,
  typesModule: "@finisher/core-types",
  typesSource: ts,
  banner: banner.replace(
    "by scripts/generate.mjs.",
    "by packages/core-types/scripts/generate.mjs\n * (src/codegen/effect.ts): the Effect schemas of the RPC results, each checked against its generated type.",
  ),
});

const contracts = join(out, "../contracts/src/generated");
mkdirSync(join(out, "schema"), { recursive: true });
mkdirSync(join(out, "src/generated"), { recursive: true });
mkdirSync(contracts, { recursive: true });
writeFileSync(join(out, "schema/fin-core.schema.json"), cargo.stdout);
writeFileSync(join(out, "src/generated/index.ts"), ts);
writeFileSync(join(contracts, "schemas.ts"), effect);
