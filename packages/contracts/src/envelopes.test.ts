// Gate (`pnpm gates` runs it through scripts/check-envelopes.mjs; `pnpm test` runs it too): the Effect schemas that validate RPC envelopes at runtime must describe exactly the
// Rust types they carry (generated into @finisher/core-types). Their TS types come from the
// generated ones; this checks the runtime half, field by field.
import { readFileSync } from "node:fs";
import { Schema } from "effect";
import { expect, test } from "vitest";
import { BranchViewSchema, ConflictSchema } from "./history.ts";
import * as Generated from "./generated/schemas.ts";
import { GenerationSchema, ProbeSchema, ProjectEventSchema, TxSetAsideSchema } from "./rpc.ts";

type Json = { [k: string]: unknown };
// `FIN_CORE_SCHEMA` points the check at another schema file (scripts/check-envelopes.test.mjs seeds drift this way).
const schemaFile = process.env.FIN_CORE_SCHEMA ?? new URL("../../core-types/schema/fin-core.schema.json", import.meta.url);
const defs = JSON.parse(readFileSync(schemaFile, "utf8")).$defs as Record<string, Json>;

/**
 * A comparable shape: `{ field: kind }` plus the required fields. Kinds: `string`, `number`,
 * `boolean`, `literal:<v>`, a nested shape, or `unknown` (a value the contract leaves to Rust:
 * a `$ref` to a fin-core type, `Schema.Unknown`).
 */
function shape(s: unknown): unknown {
  if (s === true || s === null || typeof s !== "object") return "unknown";
  const j = s as Json;
  if ("$ref" in j) return "unknown";
  if ("const" in j) return `literal:${String(j.const)}`;
  if (Array.isArray(j.enum) && j.enum.length === 1) return `literal:${String(j.enum[0])}`;
  // Effect writes a number as `number | "Infinity" | "-Infinity" | "NaN"`.
  if (Array.isArray(j.anyOf) && (j.anyOf as Json[]).some((a) => a.type === "number")) return "number";
  if (j.type === "integer" || j.type === "number") return "number";
  if (j.type === "string" || j.type === "boolean") return j.type;
  if (j.type === "object" && j.properties) {
    const props = Object.fromEntries(
      Object.entries(j.properties as Json)
        .map(([k, v]) => [k, shape(v)] as const)
        .sort(([a], [b]) => a.localeCompare(b)),
    );
    return { props, required: [...((j.required as string[]) ?? [])].sort() };
  }
  if (Object.keys(j).length === 0) return "unknown";
  return j;
}

const effect = (s: Schema.Top) => shape(Schema.toJsonSchemaDocument(s).schema);
const rust = (name: string) => shape(defs[name]);
/** The members of a tagged union, by their `type` literal. */
const byType = (members: unknown[]) =>
  Object.fromEntries(members.map((m) => [((m as { props: Json }).props.type as string).slice("literal:".length), m]));

test("the gate tells a drifted schema apart", () => {
  expect(effect(Schema.Struct({ incarnation: Schema.String }))).not.toEqual(rust("Generation"));
  expect(effect(Schema.Struct({ incarnation: Schema.String, n: Schema.String }))).not.toEqual(rust("Generation"));
});

test("GenerationSchema is fin-store's Generation", () => {
  expect(effect(GenerationSchema)).toEqual(rust("Generation"));
});

test("TxSetAsideSchema is fin-store's TxSetAside", () => {
  const ours = effect(TxSetAsideSchema) as { props: Json };
  expect(ours.props.setAside).toEqual(rust("SetAside"));
  expect({ ...ours, props: { setAside: "unknown" } }).toEqual(rust("TxSetAside"));
});

test("ProbeSchema is fin-probe's Probe, with each kind's info", () => {
  const ours = Schema.toJsonSchemaDocument(ProbeSchema).schema as { properties: Json; required?: string[] };
  const theirs = defs.Probe as { properties: Json; required: string[] };
  expect(Object.keys(ours.properties).sort()).toEqual(Object.keys(theirs.properties).sort());
  expect([...(ours.required ?? [])].sort()).toEqual([...theirs.required].sort());
  // Absent fields are omitted, never `null`: neither side admits `null`.
  for (const [k, v] of Object.entries(theirs.properties)) expect(JSON.stringify(v), k).not.toContain('"null"');
  for (const [k, v] of Object.entries(ours.properties)) expect(JSON.stringify(v), k).not.toContain('"null"');
  const info = (v: unknown) => {
    const s = v as { properties: Json; required: string[] };
    return { keys: Object.keys(s.properties).sort(), required: [...s.required].sort() };
  };
  for (const [ours_, rust_] of [
    [ours.properties.audio, "AudioInfo"],
    [ours.properties.video, "VideoInfo"],
    [ours.properties.image, "ImageInfo"],
    [ours.properties.lottie, "LottieInfo"],
    [ours.properties.font, "FontFile"],
    [ours.properties.lut, "LutInfo"],
  ] as const) {
    const d = defs[rust_] as { properties: Json; required: string[] };
    expect(info(ours_)).toEqual({ keys: Object.keys(d.properties).sort(), required: [...d.required].sort() });
  }
});

test("ProjectEventSchema is fin-store's EngineEvent less what finisherd keeps to itself", () => {
  const engine = byType((defs.EngineEvent!.oneOf as unknown[]).map(shape));
  const ours = byType((Schema.toJsonSchemaDocument(ProjectEventSchema).schema as { anyOf: unknown[] }).anyOf.map(shape));
  // `applied` feeds ops.subscribe; `lagged` ends subscribers; `collect` is the journal's. None
  // reaches project.events.
  const kept = (t: string) => t !== "applied" && t !== "lagged" && t !== "collect";
  expect(Object.keys(ours).sort()).toEqual(Object.keys(engine).filter(kept).sort());
  // The status is the generated `ProjectStatusView` (a `$ref` on the Rust side).
  const status = ours.status as { props: Json };
  expect(status.props.status).toEqual(effect(Generated.ProjectStatusViewSchema));
  expect(engine.status).toEqual({ ...status, props: { ...status.props, status: "unknown" } });
  for (const [type, member] of Object.entries(ours)) if (type !== "status") expect(member, type).toEqual(engine[type]);
});

/** The generated schemas' definitions (`<Name>Schema`), by name. */
const generated = Object.entries(Generated).flatMap(([k, v]) => (k.endsWith("Schema") ? [[k.slice(0, -"Schema".length), v as Schema.Top] as const] : []));

test("every generated schema lists its definition's fields, required as Rust has them", () => {
  expect(generated.length).toBeGreaterThan(50);
  let objects = 0;
  for (const [name, schema] of generated) {
    const def = defs[name] as { properties?: Json; required?: string[] } | undefined;
    expect(def, name).toBeDefined();
    // `CommittedTx` is passed through (`Schema.Unknown`); every other object is a `Struct`.
    if (def!.properties === undefined || name === "CommittedTx") continue;
    objects++;
    const ours = Schema.toJsonSchemaDocument(schema).schema as { properties: Json; required?: string[] };
    expect(Object.keys(ours.properties).sort(), name).toEqual(Object.keys(def!.properties).sort());
    expect([...(ours.required ?? [])].sort(), name).toEqual([...(def!.required ?? [])].sort());
  }
  expect(objects).toBeGreaterThan(40);
});

test("BranchViewSchema is fin-store's BranchView, with its states", () => {
  const ours = Schema.toJsonSchemaDocument(BranchViewSchema).schema as { properties: Json; required?: string[] };
  const theirs = defs.BranchView as { properties: Json; required: string[] };
  expect(Object.keys(ours.properties).sort()).toEqual(Object.keys(theirs.properties).sort());
  expect([...(ours.required ?? [])].sort()).toEqual([...theirs.required].sort());
  // Effect writes a union of literals as one `enum`, or as `anyOf` of `const`s.
  const state = ours.properties.state as { enum?: string[]; anyOf?: { const?: string; enum?: string[] }[] };
  const states = state.enum ?? state.anyOf!.flatMap((a) => a.enum ?? [a.const]);
  expect(states.sort()).toEqual([...(defs.BranchState as { enum: string[] }).enum].sort());
});

test("ConflictSchema is fin-core's Conflict", () => {
  const ours = Schema.toJsonSchemaDocument(ConflictSchema).schema as { properties: Json; required?: string[] };
  const theirs = defs.Conflict as { properties: Json; required: string[] };
  expect(Object.keys(ours.properties).sort()).toEqual(Object.keys(theirs.properties).sort());
  expect([...(ours.required ?? [])].sort()).toEqual([...theirs.required].sort());
});
