// Gate: the Effect schemas that validate RPC envelopes at runtime must describe exactly the
// Rust types they carry (generated into @finisher/core-types). Their TS types come from the
// generated ones; this checks the runtime half, field by field.
import { readFileSync } from "node:fs";
import { Schema } from "effect";
import { expect, test } from "vitest";
import { GenerationSchema, ProbeSchema, ProjectEventSchema, TxSetAsideSchema } from "./rpc.ts";

type Json = { [k: string]: unknown };
const defs = JSON.parse(readFileSync(new URL("../../core-types/schema/fin-core.schema.json", import.meta.url), "utf8"))
  .$defs as Record<string, Json>;

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

test("ProbeSchema is fin-core's Probe, with its audio and video info", () => {
  const ours = Schema.toJsonSchemaDocument(ProbeSchema).schema as { properties: Json; required?: string[] };
  const theirs = defs.Probe as { properties: Json; required: string[] };
  expect(Object.keys(ours.properties).sort()).toEqual(Object.keys(theirs.properties).sort());
  expect([...(ours.required ?? [])].sort()).toEqual([...theirs.required].sort());
  const info = (v: unknown) => {
    const s = (v as { anyOf: Json[] }).anyOf.find((a) => a.properties) as { properties: Json; required: string[] };
    return { keys: Object.keys(s.properties).sort(), required: [...s.required].sort() };
  };
  for (const [ours_, rust_] of [
    [ours.properties.audio, "AudioInfo"],
    [ours.properties.video, "VideoInfo"],
  ] as const) {
    const d = defs[rust_] as { properties: Json; required: string[] };
    expect(info(ours_)).toEqual({ keys: Object.keys(d.properties).sort(), required: [...d.required].sort() });
  }
});

test("ProjectEventSchema is fin-store's EngineEvent less what finisherd keeps to itself", () => {
  const engine = byType((defs.EngineEvent!.oneOf as unknown[]).map(shape));
  const ours = byType((Schema.toJsonSchemaDocument(ProjectEventSchema).schema as { anyOf: unknown[] }).anyOf.map(shape));
  // `applied` feeds ops.subscribe; `lagged` ends subscribers. Neither reaches project.events.
  expect(Object.keys(ours).sort()).toEqual(Object.keys(engine).filter((t) => t !== "applied" && t !== "lagged").sort());
  for (const [type, member] of Object.entries(ours)) expect(member, type).toEqual(engine[type]);
});
