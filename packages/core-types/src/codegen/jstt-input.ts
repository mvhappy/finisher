// The schema as json-schema-to-typescript reads it (the committed schema stays as schemars
// wrote it). Two draft 2020-12 forms it does not know are rewritten, so the generated types
// say what the Effect schemas (`effect.ts`) check:
// - a tuple's `prefixItems` (a Rust `(String, String)`) becomes the draft-07 `items` array
//   (it typed it `[unknown, unknown]`);
// - an empty schema or one of annotations only (any JSON value: a `Box<Value>` field with a doc) is
//   typed `unknown` (it typed it `{ [k: string]: unknown }`, an object).

type Json = { [k: string]: unknown };

const ANNOTATIONS = new Set(["description", "title", "default", "examples", "$comment", "deprecated", "readOnly", "writeOnly"]);

/** A copy of `schema` rewritten for json-schema-to-typescript. */
export function forTypeScript<S>(schema: S): S {
  return rewrite(schema, false) as S;
}

/** `inSchema`: `v` is a schema (not a `properties` map, a `$defs` map or an annotation's value). */
function rewrite(v: unknown, inSchema: boolean): unknown {
  if (Array.isArray(v)) return v.map((x) => rewrite(x, inSchema));
  if (v === null || typeof v !== "object") return v;
  const j = v as Json;
  const out: Json = {};
  for (const [k, x] of Object.entries(j)) {
    if (ANNOTATIONS.has(k) || k === "enum" || k === "const" || k.startsWith("x-")) out[k] = x;
    else if (k === "properties" || k === "$defs") out[k] = map(x);
    else out[k] = rewrite(x, true);
  }
  if (inSchema && Array.isArray(out.prefixItems)) {
    out.items = out.prefixItems;
    delete out.prefixItems;
    out.additionalItems = false;
  }
  // (`x-` keywords are left out: an `allOf` member of `x-fin-class` alone tags its sibling.)
  if (inSchema && Object.keys(j).every((k) => ANNOTATIONS.has(k))) {
    out.tsType = "unknown";
  }
  return out;
}

function map(x: unknown): unknown {
  if (x === null || typeof x !== "object" || Array.isArray(x)) return x;
  return Object.fromEntries(Object.entries(x as Json).map(([k, s]) => [k, rewrite(s, true)]));
}
