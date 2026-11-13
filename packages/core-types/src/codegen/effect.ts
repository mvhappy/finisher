// Effect schemas from the generated JSON Schema (`schema/fin-core.schema.json`), for the RPC
// results the contracts validate at runtime (`scripts/generate.mjs` writes them to
// `packages/contracts/src/generated/schemas.ts`). Never hand-written mirrors: Effect's `Struct`
// drops keys it does not list, so a schema that missed a field would lose data silently. Every
// construct schemars emits in the closure of the roots is translated exactly; anything else
// throws, naming the definition and the keyword.
//
// - an object (`properties`, `required`; `additionalProperties: false` or absent) is a `Struct`,
//   its optional fields `optionalKey`; an object of `additionalProperties` alone a `Record`;
// - `anyOf`/`oneOf` a `Union` (a `null` member: `NullOr`), `allOf` of one member that member;
// - `type: [T, "null"]` is `NullOr(T)`; `const`/`enum` are `Literal`/`Literals`;
// - arrays are `Array` (mutable, as the generated types), or a `Tuple`: `prefixItems`, or
//   `items` with `minItems` = `maxItems` (json-schema-to-typescript writes these as tuples);
// - `integer` is a `Number` checked `isInt`, `minimum`/`maximum` checks, string `pattern`s and
//   lengths checks too;
// - `true`/`{}` (any JSON) is `Unknown`, as is each `opaque` definition: a pass-through Rust
//   has validated, typed as its generated type.
//
// The output is deterministic: definitions in dependency order, then by name.

type Json = { [k: string]: unknown };

export interface EffectOptions {
  /** The definitions to generate, with every definition they reference. */
  readonly roots: readonly string[];
  /** Definitions passed through as `Schema.Unknown` typed as their generated type (not walked). */
  readonly opaque: readonly string[];
  /** The module the generated TypeScript types are imported from. */
  readonly typesModule: string;
  /** The generated TypeScript (`src/generated/index.ts`): every definition must be exported there. */
  readonly typesSource: string;
  /** The file's banner comment. */
  readonly banner: string;
}

/** Keywords that only annotate: they change no value's shape. */
const ANNOTATIONS = new Set(["description", "title", "default", "examples", "$comment", "deprecated", "readOnly", "writeOnly"]);
/** The `format`s schemars writes on integers and numbers (informational). */
const NUMBER_FORMATS = new Set(["int8", "int16", "int32", "int64", "int", "uint8", "uint16", "uint32", "uint64", "uint", "float", "double"]);
/** json-schema-to-typescript writes `items` with `minItems` = `maxItems` up to this as a tuple. */
const TUPLE_MAX = 20;
const REF = "#/$defs/";

const isAnnotation = (k: string) => ANNOTATIONS.has(k) || k.startsWith("x-");

/** A schema's shape keywords (annotations left out). */
function shapeKeys(s: Json): string[] {
  return Object.keys(s).filter((k) => !isAnnotation(k));
}

class Unhandled extends Error {}

/** Emits the Effect schemas of `roots` and their `$ref` closure. */
export function effectSchemas(doc: { $defs?: Record<string, unknown> }, o: EffectOptions): string {
  const defs = doc.$defs ?? {};
  const opaque = new Set(o.opaque);
  const deps = new Map<string, Set<string>>();
  const code = new Map<string, string>();

  const visit = (name: string) => {
    if (code.has(name)) return;
    if (!(name in defs)) throw new Error(`effect schemas: no definition ${name}`);
    if (!new RegExp(`^export (interface|type) ${name}\\b`, "m").test(o.typesSource)) {
      throw new Error(`effect schemas: ${name} is not exported by the generated types`);
    }
    const used = new Set<string>();
    code.set(name, ""); // guards against revisiting while walking
    const expr = opaque.has(name) ? `opaque<T.${name}>()` : translate(defs[name], name, used);
    code.set(name, expr);
    deps.set(name, used);
    for (const d of used) visit(d);
  };
  for (const r of o.roots) visit(r);

  const order = topological(deps);
  const out = [
    o.banner,
    `import type * as T from ${JSON.stringify(o.typesModule)};`,
    `import { Schema } from "effect";`,
    "",
    "type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;",
    "/** A value Rust has validated, passed through: `Schema.Unknown` typed as its generated type. */",
    "const opaque = <A>() => Schema.Unknown as unknown as Schema.Codec<A>;",
    "",
  ];
  for (const name of order) {
    out.push(`export const ${name}Schema = ${code.get(name)};`);
    out.push(`export const ${lower(name)}Matches: Same<typeof ${name}Schema.Type, T.${name}> = true;`);
  }
  return `${out.join("\n")}\n`;
}

const lower = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

/** Dependencies first, ties by name; a cycle throws (recursive types need `Schema.suspend`). */
function topological(deps: Map<string, Set<string>>): string[] {
  const out: string[] = [];
  const state = new Map<string, "open" | "done">();
  const walk = (n: string, path: string[]) => {
    if (state.get(n) === "done") return;
    if (state.get(n) === "open") throw new Error(`effect schemas: recursive definition ${[...path, n].join(" -> ")} is not supported`);
    state.set(n, "open");
    for (const d of [...(deps.get(n) ?? [])].sort()) walk(d, [...path, n]);
    state.set(n, "done");
    out.push(n);
  };
  for (const n of [...deps.keys()].sort()) walk(n, []);
  return out;
}

/** The Effect expression of `schema` (inside definition `where`), recording its `$ref`s in `used`. */
export function translate(schema: unknown, where: string, used: Set<string>): string {
  try {
    return expr(schema, used);
  } catch (e) {
    if (e instanceof Unhandled) throw new Error(`effect schemas: ${where}: ${e.message}`);
    throw e;
  }
}

function fail(msg: string, s: unknown): never {
  throw new Unhandled(`${msg} in ${JSON.stringify(s).slice(0, 200)}`);
}

/** Throws unless `s` holds only the shape keywords `allowed` (annotations are always allowed). */
function only(s: Json, allowed: readonly string[]) {
  const extra = shapeKeys(s).filter((k) => !allowed.includes(k));
  if (extra.length > 0) fail(`unhandled keyword ${extra.join(", ")}`, s);
}

function expr(schema: unknown, used: Set<string>): string {
  if (schema === true) return "Schema.Unknown";
  if (schema === false || schema === null || typeof schema !== "object" || Array.isArray(schema)) fail("unhandled schema", schema);
  const s = schema as Json;
  const keys = shapeKeys(s);
  if (keys.length === 0) return "Schema.Unknown";
  if ("$ref" in s) {
    only(s, ["$ref"]);
    const ref = s.$ref;
    if (typeof ref !== "string" || !ref.startsWith(REF)) fail("unhandled $ref", s);
    const name = ref.slice(REF.length);
    used.add(name);
    return `${name}Schema`;
  }
  if ("allOf" in s) {
    only(s, ["allOf"]);
    const all = s.allOf as unknown[];
    if (!Array.isArray(all) || all.length !== 1) fail("allOf of other than one member", s);
    return expr(all[0], used);
  }
  for (const k of ["anyOf", "oneOf"] as const) {
    if (k in s) {
      only(s, [k]);
      const members = s[k] as unknown[];
      if (!Array.isArray(members) || members.length === 0) fail(`empty ${k}`, s);
      const isNull = (m: unknown) => typeof m === "object" && m !== null && shapeKeys(m as Json).length === 1 && (m as Json).type === "null";
      const rest = members.filter((m) => !isNull(m)).map((m) => expr(m, used));
      if (rest.length === 0) return "Schema.Null";
      const inner = rest.length === 1 ? rest[0]! : `Schema.Union([${rest.join(", ")}])`;
      return rest.length === members.length ? inner : `Schema.NullOr(${inner})`;
    }
  }
  if ("const" in s || "enum" in s) return literals(s);
  if (Array.isArray(s.type)) {
    const types = s.type as unknown[];
    const nonNull = types.filter((t) => t !== "null");
    if (nonNull.length === 0) return "Schema.Null";
    const inner = nonNull.map((t) => typed({ ...s, type: t }, used));
    const union = inner.length === 1 ? inner[0]! : `Schema.Union([${inner.join(", ")}])`;
    return nonNull.length === types.length ? union : `Schema.NullOr(${union})`;
  }
  return typed(s, used);
}

function literals(s: Json): string {
  only(s, ["const", "enum", "type"]);
  if ("const" in s && "enum" in s) fail("const beside enum", s);
  const values = "const" in s ? [s.const] : (s.enum as unknown[]);
  if (!Array.isArray(values) || values.length === 0) fail("empty enum", s);
  const types = s.type === undefined ? null : Array.isArray(s.type) ? s.type : [s.type];
  for (const v of values) {
    const t = v === null ? "null" : typeof v === "number" ? (Number.isInteger(v) ? "integer" : "number") : typeof v;
    if (!["string", "number", "integer", "boolean", "null"].includes(t)) fail("non-scalar enum value", s);
    if (types && !types.includes(t) && !(t === "integer" && types.includes("number"))) fail(`enum value ${JSON.stringify(v)} outside its type`, s);
  }
  const nonNull = values.filter((v) => v !== null);
  const lit =
    nonNull.length === 0 ? "Schema.Null" : nonNull.length === 1 ? `Schema.Literal(${JSON.stringify(nonNull[0])})` : `Schema.Literals([${nonNull.map((v) => JSON.stringify(v)).join(", ")}])`;
  return nonNull.length === values.length || nonNull.length === 0 ? lit : `Schema.NullOr(${lit})`;
}

const checks = (base: string, cs: string[]) => (cs.length === 0 ? base : `${base}.check(${cs.join(", ")})`);

function typed(s: Json, used: Set<string>): string {
  switch (s.type) {
    case "null":
      only(s, ["type"]);
      return "Schema.Null";
    case "boolean":
      only(s, ["type"]);
      return "Schema.Boolean";
    case "integer":
    case "number": {
      only(s, ["type", "format", "minimum", "maximum"]);
      if (s.format !== undefined && !NUMBER_FORMATS.has(s.format as string)) fail(`unhandled format ${String(s.format)}`, s);
      const cs = s.type === "integer" ? ["Schema.isInt()"] : [];
      if (typeof s.minimum === "number") cs.push(`Schema.isGreaterThanOrEqualTo(${s.minimum})`);
      if (typeof s.maximum === "number") cs.push(`Schema.isLessThanOrEqualTo(${s.maximum})`);
      return checks("Schema.Number", cs);
    }
    case "string": {
      only(s, ["type", "pattern", "minLength", "maxLength"]);
      const cs: string[] = [];
      if (typeof s.pattern === "string") {
        new RegExp(s.pattern, "u"); // a pattern JavaScript cannot read fails here, not at load
        cs.push(`Schema.isPattern(new RegExp(${JSON.stringify(s.pattern)}, "u"))`);
      }
      if (typeof s.minLength === "number") cs.push(`Schema.isMinLength(${s.minLength})`);
      if (typeof s.maxLength === "number") cs.push(`Schema.isMaxLength(${s.maxLength})`);
      return checks("Schema.String", cs);
    }
    case "array":
      return array(s, used);
    case "object":
      return object(s, used);
    default:
      return fail(`unhandled type ${JSON.stringify(s.type)}`, s);
  }
}

function array(s: Json, used: Set<string>): string {
  only(s, ["type", "items", "prefixItems", "minItems", "maxItems"]);
  const { minItems: min, maxItems: max } = s;
  if (Array.isArray(s.prefixItems)) {
    const n = s.prefixItems.length;
    // schemars writes a Rust tuple as exactly its elements (no `items`).
    if ("items" in s || min !== n || max !== n) fail("prefixItems other than a fixed tuple", s);
    return `Schema.mutable(Schema.Tuple([${s.prefixItems.map((i) => expr(i, used)).join(", ")}]))`;
  }
  if (s.items === undefined || typeof s.items !== "object" || Array.isArray(s.items)) fail("array without an items schema", s);
  const item = expr(s.items, used);
  if (min === undefined && max === undefined) return `Schema.mutable(Schema.Array(${item}))`;
  if (typeof min === "number" && min === max && min <= TUPLE_MAX) {
    return `Schema.mutable(Schema.Tuple([${Array.from({ length: min }, () => item).join(", ")}]))`;
  }
  return fail("unhandled minItems/maxItems", s);
}

const KEY = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

function object(s: Json, used: Set<string>): string {
  only(s, ["type", "properties", "required", "additionalProperties"]);
  const props = s.properties as Json | undefined;
  const required = (s.required ?? []) as unknown[];
  if (!Array.isArray(required) || required.some((r) => typeof r !== "string")) fail("unhandled required", s);
  const extra = s.additionalProperties;
  if (props === undefined) {
    if (required.length > 0) fail("required without properties", s);
    if (extra === undefined || extra === false) fail("an object with no properties", s);
    return `Schema.Record(Schema.String, ${expr(extra, used)})`;
  }
  if (extra !== undefined && extra !== false) fail("properties beside additionalProperties", s);
  for (const r of required as string[]) if (!(r in props)) fail(`required ${r} is not a property`, s);
  const fields = Object.keys(props)
    .sort()
    .map((k) => {
      const v = expr(props[k], used);
      return `${KEY.test(k) ? k : JSON.stringify(k)}: ${required.includes(k) ? v : `Schema.optionalKey(${v})`}`;
    });
  return `Schema.Struct({\n  ${fields.join(",\n  ")},\n})`;
}
