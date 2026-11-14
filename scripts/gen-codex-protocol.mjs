// Generates `apps/finisherd/src/agent/codex/generated/protocol.ts` (sub-project 7E plan Task 3,
// Decision 2): Effect Schemas for exactly the Codex app-server methods, notifications and server
// requests the adapter uses, and every definition they reach, from the pinned version's committed
// JSON Schema (`vendor/codex/<pin>/schema/`, plus the `--experimental` bundle for `thread/start`'s
// `dynamicTools` and `item/tool/call`). TypeScript model types are generated, never hand-written.
//
// The decode rules (Review Focus 6, drift fails closed):
// - unknown object keys are dropped (Effect's default);
// - an enum value, a tagged-union member (`{type: …}`) or an externally tagged member (`"x"` or
//   `{x: …}`) this pin does not know decodes as an `UnknownMember` (the mapper's notice), never
//   an error; a known member with a field missing or mistyped is an error (`PROVIDER_PROTOCOL`);
// - a single-valued enum is a constant (a tag), closed.
//
// `node scripts/gen-codex-protocol.mjs` writes the file; `scripts/check-codex-protocol.mjs`
// (in `pnpm gates`) regenerates it and fails on any difference.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

export const OUT = "apps/finisherd/src/agent/codex/generated/protocol.ts";

/**
 * The client requests the adapter sends: `codex/rpc.ts`'s `ALLOWED_METHODS` must be exactly this
 * list (the gate checks). `thread/inject_items` is not here (Decision 17: only if Task 0 adopts it).
 */
export const CLIENT_METHODS = [
  "initialize",
  "thread/start",
  "thread/resume",
  "thread/revert",
  "thread/read",
  "turn/start",
  "turn/steer",
  "turn/interrupt",
  "model/list",
  "account/read",
  "config/read",
  "configRequirements/read",
  "skills/list",
  "hooks/list",
  "plugin/installed",
  "mcpServerStatus/list",
  "app/installed",
  "experimentalFeature/list",
];

/** The client notifications the adapter sends. */
export const CLIENT_NOTIFICATIONS = ["initialized"];

/**
 * The server notifications the adapter decodes (Decisions 9, 13–19). Every other method is an
 * unknown notification (a notice once per name): realtime, goals, guardian reviews, MCP, fs,
 * processes, archives and the rest are switched off or never asked for.
 */
export const SERVER_NOTIFICATIONS = [
  // the turn's items
  "item/started",
  "item/completed",
  "item/agentMessage/delta",
  "item/plan/delta",
  "item/reasoning/summaryTextDelta",
  "item/reasoning/summaryPartAdded",
  "item/reasoning/textDelta",
  "item/commandExecution/outputDelta",
  "item/commandExecution/terminalInteraction",
  "item/fileChange/outputDelta",
  "item/fileChange/patchUpdated",
  "item/mcpToolCall/progress",
  // turns
  "turn/started",
  "turn/completed",
  "turn/diff/updated",
  "turn/plan/updated",
  "turn/moderationMetadata",
  // the thread
  "thread/started",
  "thread/status/changed",
  "thread/closed",
  "thread/reverted",
  "thread/name/updated",
  "thread/section/changed",
  "thread/settings/updated",
  "thread/tokenUsage/updated",
  "thread/compacted",
  // notices, account and the rest the mapper names
  "error",
  "warning",
  "configWarning",
  "deprecationNotice",
  "model/rerouted",
  "serverRequest/resolved",
  "account/updated",
  "remoteControl/status/changed",
];

/** The server requests (all ten of the stable schema): their answers are typed to these only. */
export const SERVER_REQUESTS = [
  "item/commandExecution/requestApproval",
  "item/fileChange/requestApproval",
  "item/tool/requestUserInput",
  "mcpServer/elicitation/request",
  "item/permissions/requestApproval",
  "item/tool/call",
  "account/chatgptAuthTokens/refresh",
  "attestation/generate",
  "applyPatchApproval",
  "execCommandApproval",
];

/** Methods whose params come from the `--experimental` bundle (Decision 2). */
const EXPERIMENTAL = new Set(["thread/start", "item/tool/call"]);

/** Results whose name is not `<Params minus "Params">Response`. */
const RESULT_NAMES = { "configRequirements/read": "ConfigRequirementsReadResponse" };

// Names the generated file's own code uses: a definition may not shadow one.
const RESERVED = new Set(["Schema", "SchemaTransformation", "UnknownMember", "Set", "Object", "Array", "String", "Number", "Boolean", "Error", "Record", "Closed"]);

const BUNDLE = "codex_app_server_protocol.schemas.json";

export function readPinVersion(root) {
  return JSON.parse(readFileSync(join(root, "vendor/codex/pin.json"), "utf8")).version;
}

function load(root, version) {
  const read = (dir) => JSON.parse(readFileSync(join(root, "vendor/codex", version, dir, BUNDLE), "utf8"));
  return { stable: read("schema"), experimental: read("schema-experimental") };
}

const bare = (node) => {
  // a definition without its documentation, to compare two bundles' copies
  if (Array.isArray(node)) return node.map(bare);
  if (node === null || typeof node !== "object") return node;
  const out = {};
  for (const k of Object.keys(node).sort()) if (!["title", "description", "$schema", "default"].includes(k)) out[k] = bare(node[k]);
  return out;
};

const methodOf = (member) => member.properties.method.enum[0];

/**
 * The JSON Schema keywords the generator understands. Any other keyword (`pattern`, `maximum`,
 * `const`, `maxLength`, `not`, `if`, …) fails the generation, so a re-pin that adds a constraint
 * can never be decoded more loosely than its schema says. `description`, `title`, `default` and
 * `$schema` are documentation; `format` is allowed only with the numeric widths in `FORMATS`,
 * which are not range-checked beyond `minimum` (the pins so far pair every unsigned width with
 * `minimum: 0`).
 */
export const KEYWORDS = new Set(["type", "properties", "required", "items", "additionalProperties", "enum", "oneOf", "anyOf", "allOf", "$ref", "minimum", "minLength", "format", "description", "title", "default", "$schema", "definitions"]);
export const FORMATS = new Set(["int32", "int64", "uint", "uint16", "uint32", "uint64", "double"]);
function understood(node, where) {
  if (node === null || typeof node !== "object" || Array.isArray(node)) return;
  for (const k of Object.keys(node)) if (!KEYWORDS.has(k)) throw new Error(`${where}: the schema keyword ${JSON.stringify(k)} is not understood by this generator`);
  if (node.minLength !== undefined && !([node.type].flat().includes("string") && Number.isSafeInteger(node.minLength)))
    throw new Error(`${where}: minLength outside a string schema`);
  if (node.format !== undefined && !(FORMATS.has(node.format) && [node.type].flat().some((t) => t === "integer" || t === "number")))
    throw new Error(`${where}: the format ${JSON.stringify(node.format)} is not understood by this generator`);
}

/** The generated file's text. Throws on a schema construct it does not handle. */
export function generate(root) {
  const version = readPinVersion(root);
  const bundles = load(root, version);
  const lookup = (bundle, name) => bundle.definitions.v2?.[name] ?? bundle.definitions[name];
  const refName = (ref) => {
    const m = /^#\/definitions\/(?:v2\/)?([A-Za-z0-9_]+)$/.exec(ref);
    if (!m) throw new Error(`unsupported $ref ${ref}`);
    return m[1];
  };

  // ---- the roots ----
  const union = (bundle, name) => {
    const u = bundle.definitions[name];
    return new Map(u.oneOf.map((m) => [methodOf(m), m]));
  };
  const stableClient = union(bundles.stable, "ClientRequest");
  const expClient = union(bundles.experimental, "ClientRequest");
  const allClientMethods = [...new Set([...stableClient.keys(), ...expClient.keys()])].sort();
  const notifications = union(bundles.stable, "ServerNotification");
  const serverRequests = union(bundles.stable, "ServerRequest");
  const expServerRequests = union(bundles.experimental, "ServerRequest");

  // name → {bundle, node}: a definition is the stable bundle's unless only the experimental one
  // has it or a root that needs the experimental bundle reaches it first with a different body.
  const defs = new Map();
  const need = (name, bundle) => {
    if (defs.has(name)) {
      const had = defs.get(name);
      if (had.bundle !== bundle) {
        const other = lookup(bundle, name);
        if (other !== undefined && JSON.stringify(bare(other)) !== JSON.stringify(bare(had.node)))
          throw new Error(`definition ${name} differs between the stable and experimental bundles`);
      }
      return;
    }
    let node = lookup(bundle, name);
    let from = bundle;
    if (bundle === bundles.experimental && !EXPERIMENTAL_ROOT_NAMES.has(name) && lookup(bundles.stable, name) !== undefined) {
      node = lookup(bundles.stable, name);
      from = bundles.stable;
      const exp = lookup(bundles.experimental, name);
      if (JSON.stringify(bare(exp)) !== JSON.stringify(bare(node)))
        throw new Error(`definition ${name} (reached from an experimental root) differs from the stable one`);
    }
    if (node === undefined) throw new Error(`no definition ${name}`);
    if (RESERVED.has(name)) throw new Error(`definition ${name} shadows a name the generated file uses`);
    defs.set(name, { bundle: from, node });
    visitRefs(node, (r) => need(r, from));
  };
  const visitRefs = (node, f) => {
    if (Array.isArray(node)) return node.forEach((n) => visitRefs(n, f));
    if (node === null || typeof node !== "object") return;
    if (typeof node.$ref === "string") f(refName(node.$ref));
    for (const [k, v] of Object.entries(node)) {
      if (k === "properties" || k === "definitions") Object.values(v).forEach((n) => visitRefs(n, f));
      else if (k !== "$ref" && k !== "enum" && k !== "required" && k !== "default") visitRefs(v, f);
    }
  };
  const EXPERIMENTAL_ROOT_NAMES = new Set();

  const paramsRef = (member) => {
    const p = member.properties.params;
    if (p === undefined || p.type === "null") return null;
    if (typeof p.$ref !== "string") throw new Error(`${methodOf(member)}: params are not a $ref`);
    return refName(p.$ref);
  };
  const resultName = (method, params) => {
    const name = RESULT_NAMES[method] ?? (params === null ? null : params.replace(/Params$/, "Response"));
    if (name === null) throw new Error(`${method}: no result name`);
    return name;
  };

  const clientRows = [];
  for (const m of CLIENT_METHODS) {
    const exp = EXPERIMENTAL.has(m);
    const member = (exp ? expClient : stableClient).get(m);
    if (!member) throw new Error(`client method ${m} is not in the schema`);
    const p = paramsRef(member);
    const r = resultName(m, p);
    if (exp && p !== null) EXPERIMENTAL_ROOT_NAMES.add(p);
    clientRows.push([m, p, r, exp]);
  }
  const serverRows = [];
  for (const m of SERVER_REQUESTS) {
    const exp = EXPERIMENTAL.has(m);
    const member = (exp ? expServerRequests : serverRequests).get(m);
    if (!member) throw new Error(`server request ${m} is not in the schema`);
    const p = paramsRef(member);
    if (p === null) throw new Error(`server request ${m} has no params`);
    if (exp) EXPERIMENTAL_ROOT_NAMES.add(p);
    serverRows.push([m, p, resultName(m, p), exp]);
  }
  const notificationRows = [];
  for (const m of SERVER_NOTIFICATIONS) {
    const member = notifications.get(m);
    if (!member) throw new Error(`server notification ${m} is not in the schema`);
    const p = paramsRef(member);
    if (p === null) throw new Error(`server notification ${m} has no params`);
    notificationRows.push([m, p]);
  }
  for (const n of [...CLIENT_NOTIFICATIONS]) {
    const cn = bundles.stable.definitions.ClientNotification.oneOf.map(methodOf);
    if (!cn.includes(n)) throw new Error(`client notification ${n} is not in the schema`);
  }
  if ([...notifications.keys(), ...serverRequests.keys()].includes("unknown")) throw new Error(`a method is called "unknown"`);

  for (const [, p, r, exp] of clientRows) {
    const b = exp ? bundles.experimental : bundles.stable;
    if (p !== null) need(p, b);
    need(r, bundles.stable);
  }
  for (const [, p, r, exp] of serverRows) {
    need(p, exp ? bundles.experimental : bundles.stable);
    need(r, bundles.stable);
  }
  for (const [, p] of notificationRows) need(p, bundles.stable);

  // ---- emission order: dependencies first; a reference back into the current path is suspended ----
  const order = [];
  const state = new Map(); // name → "visiting" | "done"
  const back = new Set(); // "from→to" edges that close a cycle
  const directRefs = (node) => {
    const out = [];
    visitRefs(node, (r) => out.push(r));
    return out;
  };
  const visit = (name) => {
    state.set(name, "visiting");
    for (const r of [...new Set(directRefs(defs.get(name).node))].sort()) {
      const s = state.get(r);
      if (s === "visiting") back.add(`${name}→${r}`);
      else if (s === undefined) visit(r);
    }
    state.set(name, "done");
    order.push(name);
  };
  for (const name of [...defs.keys()].sort()) if (!state.has(name)) visit(name);

  // ---- schema nodes to Effect Schema source ----
  const deref = (node) => {
    let n = node;
    for (let i = 0; i < 32; i++) {
      if (typeof n?.$ref === "string") n = defs.get(refName(n.$ref)).node;
      else if (Array.isArray(n?.allOf) && n.allOf.length === 1 && !n.properties) n = n.allOf[0];
      else return n;
    }
    throw new Error("a $ref chain does not end");
  };
  const stringEnum = (n) =>
    n !== null && typeof n === "object" && Array.isArray(n.enum) && n.enum.every((v) => typeof v === "string") && !n.properties && (n.type === undefined || n.type === "string");
  const isObject = (n) => n !== null && typeof n === "object" && n.properties !== undefined && (n.type === undefined || n.type === "object");
  const isNull = (n) => n !== null && typeof n === "object" && n.type === "null" && Object.keys(n).every((k) => ["type", "description", "title"].includes(k));
  const key = (k) => (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(k) ? k : JSON.stringify(k));
  const lit = (values) => `[${values.map((v) => JSON.stringify(v)).join(", ")}]`;

  // `member`: the node is a member of an open union, so a union it is itself has no fallback of its
  // own (the outer union's decides what is unknown; an inner one would swallow its siblings' tags).
  const emit = (node, from, member = false) => {
    understood(node, from);
    if (node === true || (node !== null && typeof node === "object" && Object.keys(node).every((k) => ["description", "title", "default", "$schema"].includes(k))))
      return "Schema.Unknown";
    if (node === false) throw new Error("a `false` schema");
    if (typeof node.$ref === "string") {
      const to = refName(node.$ref);
      return back.has(`${from}→${to}`) ? `Schema.suspend((): Schema.Codec<unknown> => ${to})` : to;
    }
    if (node.allOf !== undefined) {
      if (node.allOf.length !== 1 || node.properties) throw new Error("allOf with more than one member");
      return emit(node.allOf[0], from);
    }
    const alts = node.oneOf ?? node.anyOf;
    if (alts !== undefined) {
      if (node.properties) {
        // common properties (ThreadItem's timestamps) join each object member
        const merged = alts.map((a) => {
          const d = deref(a);
          if (!isObject(d)) throw new Error("common properties on a union with a non-object member");
          return { ...d, properties: { ...node.properties, ...d.properties }, required: [...(node.required ?? []), ...(d.required ?? [])] };
        });
        return emitUnion(merged, from, member);
      }
      return emitUnion(alts, from, member);
    }
    if (node.enum !== undefined) {
      const values = node.enum.filter((v) => v !== null);
      if (!values.every((v) => typeof v === "string")) throw new Error(`a non-string enum ${JSON.stringify(node.enum)}`);
      const nullable = node.enum.includes(null) || (Array.isArray(node.type) && node.type.includes("null"));
      const s = values.length === 1 ? `Schema.Literal(${JSON.stringify(values[0])})` : `openEnum(${lit(values)})`;
      return nullable ? `Schema.NullOr(${s})` : s;
    }
    if (Array.isArray(node.type)) {
      const types = node.type.filter((t) => t !== "null");
      const parts = types.map((t) => emit({ ...node, type: t }, from));
      const s = parts.length === 1 ? parts[0] : `Schema.Union([${parts.join(", ")}])`;
      return node.type.includes("null") ? `Schema.NullOr(${s})` : s;
    }
    switch (node.type) {
      case "string":
        return typeof node.minLength === "number" ? `Schema.String.check(Schema.isMinLength(${node.minLength}))` : "Schema.String";
      case "boolean":
        return "Schema.Boolean";
      case "null":
        return "Schema.Null";
      case "integer":
      case "number": {
        const base = node.type === "integer" ? "Schema.Int" : "Schema.Finite";
        return typeof node.minimum === "number" ? `${base}.check(Schema.isGreaterThanOrEqualTo(${node.minimum}))` : base;
      }
      case "array": {
        if (Array.isArray(node.items)) throw new Error("tuple arrays");
        return `Schema.Array(${node.items === undefined ? "Schema.Unknown" : emit(node.items, from)})`;
      }
      case "object":
      case undefined: {
        if (node.properties !== undefined) {
          if (node.additionalProperties !== undefined && typeof node.additionalProperties !== "boolean")
            throw new Error("an object with both properties and an additionalProperties schema");
          const req = new Set(node.required ?? []);
          const fields = Object.keys(node.properties)
            .sort()
            .map((k) => {
              const v = emit(node.properties[k], from);
              return `${key(k)}: ${req.has(k) ? v : `Schema.optionalKey(${v})`}`;
            });
          return `Schema.Struct({ ${fields.join(", ")} })`;
        }
        if (node.type === undefined) throw new Error(`a schema without a type: ${JSON.stringify(node).slice(0, 200)}`);
        const ap = node.additionalProperties;
        return `Schema.Record(Schema.String, ${ap === undefined || ap === true ? "Schema.Unknown" : emit(ap, from)})`;
      }
      default:
        throw new Error(`unsupported type ${JSON.stringify(node.type)}`);
    }
  };

  const emitUnion = (alts, from, member = false) => {
    // members read without `emit` (enum values, tags) are held to the same keywords
    for (const a of alts) understood(deref(a), from);
    const nullable = alts.some(isNull);
    const rest = alts.filter((a) => !isNull(a));
    const wrap = (s) => (nullable ? `Schema.NullOr(${s})` : s);
    if (rest.length === 0) return "Schema.Null";
    if (rest.length === 1) return wrap(emit(rest[0], from));
    // a member that admits any string or object (CodexErrorInfo's tail): the union is open by design
    const catchAll = (a) =>
      a !== null && typeof a === "object" && Array.isArray(a.type) && Object.keys(a).every((k) => ["type", "description", "title"].includes(k));
    if (rest.some(catchAll)) {
      const known = rest.filter((a) => !catchAll(a));
      const shapes = known.map(deref);
      const single = (s) => isObject(s) && Object.keys(s.properties).length === 1 && (s.required ?? []).includes(Object.keys(s.properties)[0]);
      const types = rest.filter(catchAll).flatMap((a) => a.type);
      if (!shapes.every((s) => stringEnum(s) || single(s)) || types.some((t) => t !== "string" && t !== "object"))
        throw new Error(`${from}: a catch-all member outside an externally tagged union`);
      return wrap(emitExternal(known, shapes, from, true));
    }
    const shapes = rest.map(deref);
    // string enums only: one open enum
    if (shapes.every(stringEnum)) {
      const values = [...new Set(shapes.flatMap((s) => s.enum))];
      return wrap(values.length === 1 ? `Schema.Literal(${JSON.stringify(values[0])})` : member ? `Schema.Literals(${lit(values)})` : `openEnum(${lit(values)})`);
    }
    // a tagged union: every member an object whose required `tag` is a single string
    if (shapes.every(isObject)) {
      const tags = Object.keys(shapes[0].properties).filter((p) =>
        shapes.every((s) => (s.required ?? []).includes(p) && stringEnum(s.properties[p] && deref(s.properties[p])) && deref(s.properties[p]).enum.length === 1),
      );
      const tag = tags.includes("type") ? "type" : tags.sort()[0];
      if (tag !== undefined) {
        // a tag may repeat (a member that is itself a union under one tag): the union tries each
        const names = [...new Set(shapes.map((s) => deref(s.properties[tag]).enum[0]))];
        const members = rest.map((a) => emitMember(a, from)).join(", ");
        return wrap(member ? `Schema.Union([${members}])` : `openTagged(${JSON.stringify(tag)}, ${lit(names)}, [${members}])`);
      }
    }
    // externally tagged (serde's default enum form): `"name"` or `{name: …}`
    const single = (s) => isObject(s) && Object.keys(s.properties).length === 1 && (s.required ?? []).includes(Object.keys(s.properties)[0]);
    if (shapes.every((s) => stringEnum(s) || single(s)) && shapes.some(single)) return wrap(emitExternal(rest, shapes, from, false));
    return wrap(`Schema.Union([${rest.map((a) => emit(a, from)).join(", ")}])`);
  };
  // A member of an open union: an inline union is closed; a named one that is a union itself
  // would bring its own fallback, which no pin has needed yet.
  const emitMember = (a, from) => {
    if (typeof a?.$ref === "string") {
      const d = deref(a);
      const alts = d.oneOf ?? d.anyOf;
      if ((alts !== undefined && alts.filter((x) => !isNull(x)).length > 1) || (stringEnum(d) && d.enum.length > 1))
        throw new Error(`${from}: a named union ${a.$ref} is a member of an open union`);
    }
    return emit(a, from, true);
  };
  const emitExternal = (alts, shapes, from, any) => {
    const strings = [...new Set(shapes.filter(stringEnum).flatMap((s) => s.enum))];
    const keys = shapes.filter((s) => !stringEnum(s)).map((s) => Object.keys(s.properties)[0]);
    const members = alts.map((a, i) => (stringEnum(shapes[i]) ? null : emitMember(a, from))).filter((m) => m !== null);
    if (strings.length > 0) members.unshift(`Schema.Literals(${lit(strings)})`);
    return `openExternal(${lit(strings)}, ${lit(keys)}, [${members.join(", ")}]${any ? ", true" : ""})`;
  };

  // ---- the file ----
  const out = [];
  const w = (s = "") => out.push(s);
  w(`// GENERATED by scripts/gen-codex-protocol.mjs from vendor/codex/${version}/schema (and schema-experimental`);
  w(`// for ${[...EXPERIMENTAL].join(", ")}). Do not edit: \`node scripts/gen-codex-protocol.mjs\`; the gate`);
  w("// `scripts/check-codex-protocol.mjs` fails when this file differs from a regeneration.");
  w("// The schema is Codex's own (`codex app-server generate-json-schema`, OpenAI, Apache-2.0).");
  w('import { Schema, SchemaTransformation } from "effect";');
  w();
  w(`/** The Codex version this file was generated from (\`vendor/codex/pin.json\`). */`);
  w(`export const CODEX_PROTOCOL_VERSION = ${JSON.stringify(version)};`);
  w();
  w("/** A member this pin does not know: an enum value, a tagged member's `type`, an external tag. */");
  w("export class UnknownMember {");
  w("  // nominal: a protocol object with a `name` is not one");
  w("  readonly #unknownMember = true;");
  w("  readonly name: string;");
  w("  constructor(name: string) {");
  w("    this.name = name.slice(0, 64);");
  w("  }");
  w("}");
  w();
  w("/** `T` without `UnknownMember`: what Finisher itself writes (params, answers). */");
  w("export type Closed<T> = T extends UnknownMember");
  w("  ? never");
  w("  : T extends readonly (infer E)[]");
  w("    ? readonly Closed<E>[]");
  w("    : T extends object");
  w("      ? { readonly [K in keyof T]: Closed<T[K]> }");
  w("      : T;");
  w();
  w("const unknownWhere = (nameOf: (u: unknown) => string | undefined) =>");
  w("  Schema.Unknown.check(Schema.makeFilter((u: unknown) => nameOf(u) !== undefined)).pipe(");
  w("    Schema.decodeTo(");
  w("      Schema.instanceOf(UnknownMember),");
  w("      SchemaTransformation.transform({ decode: (u: unknown) => new UnknownMember(nameOf(u)!), encode: (m: UnknownMember): unknown => m.name }),");
  w("    ),");
  w("  );");
  w("const plain = (u: unknown): u is Record<string, unknown> => typeof u === \"object\" && u !== null && !Array.isArray(u);");
  w("/** A string enum; a value it does not list is an `UnknownMember`. */");
  w("const openEnum = <const L extends readonly [string, ...string[]]>(values: L) => {");
  w("  const known = new Set<string>(values);");
  w("  return Schema.Union([Schema.Literals(values), unknownWhere((u) => (typeof u === \"string\" && !known.has(u) ? u : undefined))]);");
  w("};");
  w("/** A union tagged by `tag`; an object whose tag it does not list is an `UnknownMember`. */");
  w("const openTagged = <const M extends readonly Schema.Top[]>(tag: string, tags: readonly string[], members: M) => {");
  w("  const known = new Set(tags);");
  w("  const nameOf = (u: unknown) => (plain(u) && typeof u[tag] === \"string\" && !known.has(u[tag]) ? u[tag] : undefined);");
  w("  return Schema.Union([...members, unknownWhere(nameOf)]);");
  w("};");
  w("/** An externally tagged union (`\"name\"` or `{name: …}`); a name it does not list is an `UnknownMember`. */");
  w("// `any`: the schema admits any other string or object (CodexErrorInfo), so an object naming no known key is one too.");
  w("const openExternal = <const M extends readonly Schema.Top[]>(strings: readonly string[], keys: readonly string[], members: M, any = false) => {");
  w("  const known = new Set([...strings, ...keys]);");
  w("  const nameOf = (u: unknown) => {");
  w("    if (typeof u === \"string\") return known.has(u) ? undefined : u;");
  w("    if (!plain(u)) return undefined;");
  w("    const k = Object.keys(u);");
  w("    if (any) return k.some((x) => known.has(x)) ? undefined : (k[0] ?? \"object\");");
  w("    return k.length === 1 && !known.has(k[0]!) ? k[0] : undefined;");
  w("  };");
  w("  return Schema.Union([...members, unknownWhere(nameOf)]);");
  w("};");
  w();
  w("// ---- definitions (dependencies first) ----");
  w();
  for (const name of order) {
    w(`export const ${name} = ${emit(defs.get(name).node, name)};`);
    w(`export type ${name} = typeof ${name}.Type;`);
  }
  w();
  w("// ---- methods ----");
  w();
  w(`/** Every client request method of the pin's \`ClientRequest\` union (stable and experimental). */`);
  w(`export const CLIENT_REQUEST_METHODS = [`);
  for (const m of allClientMethods) w(`  ${JSON.stringify(m)},`);
  w("] as const;");
  w("export type ClientMethod = (typeof CLIENT_REQUEST_METHODS)[number];");
  w();
  w("/** The client requests the adapter sends: params (`undefined`: none) and result. */");
  w("export const CLIENT_REQUESTS = {");
  for (const [m, p, r] of clientRows) w(`  ${JSON.stringify(m)}: { params: ${p ?? "undefined"}, result: ${r} },`);
  w("} as const;");
  w();
  w(`export const CLIENT_NOTIFICATIONS = ${lit(CLIENT_NOTIFICATIONS)} as const;`);
  w();
  w("/** Every server notification method of the pin (decoded or not). */");
  w(`export const SERVER_NOTIFICATION_METHODS = [`);
  for (const m of [...notifications.keys()].sort()) w(`  ${JSON.stringify(m)},`);
  w("] as const;");
  w();
  w("/** The server notifications the adapter decodes: their params. */");
  w("export const SERVER_NOTIFICATIONS = {");
  for (const [m, p] of notificationRows) w(`  ${JSON.stringify(m)}: ${p},`);
  w("} as const;");
  w();
  w("/** The server requests: params and the answer's result. */");
  w("export const SERVER_REQUESTS = {");
  for (const [m, p, r] of serverRows) w(`  ${JSON.stringify(m)}: { params: ${p}, result: ${r} },`);
  w("} as const;");
  w();
  return out.join("\n");
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = fileURLToPath(new URL("..", import.meta.url));
  writeFileSync(join(root, OUT), generate(root));
  console.log(`wrote ${OUT}`);
}
