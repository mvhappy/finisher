import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { effectSchemas, translate } from "./effect.ts";
import { forTypeScript } from "./jstt-input.ts";

const t = (s: unknown) => translate(s, "X", new Set());

test("scalars, integers with their bounds, string checks", () => {
  expect(t({ type: "string" })).toBe("Schema.String");
  expect(t({ type: "boolean", description: "doc" })).toBe("Schema.Boolean");
  expect(t({ type: "null" })).toBe("Schema.Null");
  expect(t({ type: "number", format: "double" })).toBe("Schema.Number");
  expect(t({ type: "integer", format: "uint16", minimum: 0, maximum: 65535 })).toBe(
    "Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535))",
  );
  expect(t({ type: "integer", format: "int64" })).toBe("Schema.Number.check(Schema.isInt())");
  expect(t({ type: "string", pattern: "^a\\d$", minLength: 1, maxLength: 9 })).toBe(
    'Schema.String.check(Schema.isPattern(new RegExp("^a\\\\d$", "u")), Schema.isMinLength(1), Schema.isMaxLength(9))',
  );
});

test("any JSON value: true, {} and annotations only", () => {
  for (const s of [true, {}, { description: "anything" }, { "x-fin-class": "content" }]) expect(t(s)).toBe("Schema.Unknown");
});

test("nullable: a type list with null, anyOf with a null member", () => {
  expect(t({ type: ["string", "null"] })).toBe("Schema.NullOr(Schema.String)");
  expect(t({ type: ["integer", "null"], format: "uint64", minimum: 0 })).toBe(
    "Schema.NullOr(Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)))",
  );
  expect(t({ type: ["string", "boolean"] })).toBe("Schema.Union([Schema.String, Schema.Boolean])");
  expect(t({ anyOf: [{ $ref: "#/$defs/A" }, { type: "null" }] })).toBe("Schema.NullOr(ASchema)");
});

test("$ref, allOf of one member, unions", () => {
  const used = new Set<string>();
  expect(translate({ allOf: [{ $ref: "#/$defs/A" }], description: "field doc" }, "X", used)).toBe("ASchema");
  expect(translate({ oneOf: [{ $ref: "#/$defs/B" }, { type: "string" }] }, "X", used)).toBe("Schema.Union([BSchema, Schema.String])");
  expect([...used].sort()).toEqual(["A", "B"]);
  expect(t({ oneOf: [{ $ref: "#/$defs/A" }] })).toBe("ASchema");
});

test("const and enum: literals, integer ones, null among them", () => {
  expect(t({ const: "held", type: "string" })).toBe('Schema.Literal("held")');
  expect(t({ enum: ["a", "b"], type: "string" })).toBe('Schema.Literals(["a", "b"])');
  expect(t({ enum: [0, 90, 180, 270], type: "integer" })).toBe("Schema.Literals([0, 90, 180, 270])");
  expect(t({ enum: ["a", null] })).toBe('Schema.NullOr(Schema.Literal("a"))');
  expect(t({ enum: [true] })).toBe("Schema.Literal(true)");
  // A tagged enum as schemars writes it (`oneOf` of string groups and consts).
  expect(t({ oneOf: [{ enum: ["a", "b"], type: "string" }, { const: "c", type: "string" }] })).toBe(
    'Schema.Union([Schema.Literals(["a", "b"]), Schema.Literal("c")])',
  );
});

test("arrays: mutable, fixed-length items and prefixItems as tuples", () => {
  expect(t({ type: "array", items: { type: "string" } })).toBe("Schema.mutable(Schema.Array(Schema.String))");
  expect(t({ type: "array", items: { type: "integer" }, minItems: 2, maxItems: 2 })).toBe(
    "Schema.mutable(Schema.Tuple([Schema.Number.check(Schema.isInt()), Schema.Number.check(Schema.isInt())]))",
  );
  expect(t({ type: "array", prefixItems: [{ type: "string" }, { type: "boolean" }], minItems: 2, maxItems: 2 })).toBe(
    "Schema.mutable(Schema.Tuple([Schema.String, Schema.Boolean]))",
  );
});

test("objects: required and optional fields, quoted keys, maps", () => {
  expect(
    t({
      type: "object",
      additionalProperties: false,
      properties: { b: { type: "string" }, a: { type: ["string", "null"] }, "x-y": { type: "boolean" } },
      required: ["b"],
    }),
  ).toBe('Schema.Struct({\n  a: Schema.optionalKey(Schema.NullOr(Schema.String)),\n  b: Schema.String,\n  "x-y": Schema.optionalKey(Schema.Boolean),\n})');
  expect(t({ type: "object", additionalProperties: { type: "integer", format: "uint32", minimum: 0 } })).toBe(
    "Schema.Record(Schema.String, Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)))",
  );
});

test("anything else fails loudly, naming the definition", () => {
  for (const s of [
    { type: "string", format: "date-time" },
    { type: "integer", exclusiveMinimum: 0 },
    { type: "integer", format: "u128" },
    { type: "array", items: { type: "string" }, maxItems: 3 },
    { type: "array", items: { type: "string" }, minItems: 1 },
    { type: "array", prefixItems: [{ type: "string" }] },
    { type: "array" },
    { type: "object" },
    { type: "object", properties: { a: { type: "string" } }, additionalProperties: { type: "string" } },
    { type: "object", properties: {}, required: ["a"] },
    { type: "object", properties: {}, patternProperties: {} },
    { allOf: [{ $ref: "#/$defs/A" }, { properties: {} }] },
    { oneOf: [{ type: "string" }], properties: {} },
    { $ref: "other.json#/A" },
    { $ref: "#/$defs/A", type: "string" },
    { const: "a", enum: ["a"] },
    { enum: [1], type: "string" },
    { enum: [{ a: 1 }] },
    { not: { type: "string" } },
    { if: {}, then: {} },
    { type: "string", pattern: "(?<" },
    false,
    null,
  ]) {
    expect(() => t(s), JSON.stringify(s)).toThrow(/effect schemas: X: |Invalid regular expression/);
  }
});

const doc = {
  $defs: {
    A: { type: "object", properties: { b: { $ref: "#/$defs/B" }, tx: { $ref: "#/$defs/Tx" } }, required: ["b", "tx"] },
    B: { enum: ["x", "y"], type: "string" },
    Tx: { type: "object", properties: { op: { $ref: "#/$defs/Op" } } },
    Op: { not: {} },
    Unused: { type: "string" },
  },
};
const typesSource = "export interface A {}\nexport type B = 1;\nexport interface Tx {}\nexport type Op = 1;\n";
const options = { roots: ["A"], opaque: ["Tx"], typesModule: "@finisher/core-types", typesSource, banner: "/* banner */" };

test("a module: the closure in dependency order, opaque pass-throughs, each checked against its type", () => {
  const out = effectSchemas(doc, options);
  expect(out).toContain('import type * as T from "@finisher/core-types";');
  expect(out).toContain("export const TxSchema = opaque<T.Tx>();");
  expect(out).not.toContain("OpSchema"); // an opaque definition is not walked
  expect(out).not.toContain("Unused");
  expect(out.indexOf("export const BSchema")).toBeLessThan(out.indexOf("export const ASchema"));
  expect(out.indexOf("export const TxSchema")).toBeLessThan(out.indexOf("export const ASchema"));
  expect(out).toContain("export const aMatches: Same<typeof ASchema.Type, T.A> = true;");
  expect(effectSchemas(doc, options)).toBe(out);
});

test("a definition the generated types lack, a missing one or a cycle throws", () => {
  expect(() => effectSchemas(doc, { ...options, typesSource: "export interface A {}\n" })).toThrow(/B is not exported/);
  expect(() => effectSchemas(doc, { ...options, roots: ["Nope"] })).toThrow(/no definition Nope/);
  const cyclic = { $defs: { A: { type: "array", items: { $ref: "#/$defs/A" } } } };
  expect(() => effectSchemas(cyclic, { ...options, roots: ["A"], typesSource: "export type A = 1;" })).toThrow(/recursive definition A -> A/);
});

test("json-schema-to-typescript's input: prefixItems as an items array, any-value schemas typed unknown", () => {
  const input = {
    $defs: {
      R: { type: "object", properties: { pair: { type: "array", prefixItems: [{ type: "string" }, { type: "string" }], minItems: 2, maxItems: 2 }, any: { description: "d" }, empty: {} } },
      C: { allOf: [{ $ref: "#/$defs/R" }, { "x-fin-class": "content" }], default: { description: "kept" } },
    },
  };
  const out = forTypeScript(input) as typeof input & { $defs: { R: { properties: Record<string, unknown> } } };
  expect(out.$defs.R.properties.pair).toEqual({ type: "array", items: [{ type: "string" }, { type: "string" }], additionalItems: false, minItems: 2, maxItems: 2 });
  expect(out.$defs.R.properties.any).toEqual({ description: "d", tsType: "unknown" });
  expect(out.$defs.R.properties.empty).toEqual({ tsType: "unknown" });
  expect(out.$defs.C).toEqual(input.$defs.C);
  expect(input.$defs.R.properties.pair).toHaveProperty("prefixItems"); // the input is not changed
});

test("the committed contracts schemas cover the RPC results (the drift gate compares the whole file)", () => {
  const schema = JSON.parse(readFileSync(new URL("../../schema/fin-core.schema.json", import.meta.url), "utf8"));
  const committed = readFileSync(new URL("../../../contracts/src/generated/schemas.ts", import.meta.url), "utf8");
  for (const name of ["ProjectStatusView", "VerifyReport", "RebuildReport", "CompactReport", "HistoryResult", "MergeReport", "RecoverReport", "BranchCreated", "Probe"]) {
    expect(committed).toContain(`export const ${name}Schema = `);
    expect(schema.$defs[name]).toBeDefined();
  }
  expect(committed).toContain("export const CommittedTxSchema = opaque<T.CommittedTx>();");
});
