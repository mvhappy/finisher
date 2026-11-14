// The Codex protocol drift gate (sub-project 7E plan Task 3, Decision 2; spec §4.6, §16.2):
// - regenerating `apps/finisherd/src/agent/codex/generated/protocol.ts` from the pinned schema
//   gives the committed file (CODEX_PROTOCOL_DRIFT);
// - `ALLOWED_METHODS` in `codex/rpc.ts` is exactly the generator's `CLIENT_METHODS`, and each is a
//   method of the pinned schema's `ClientRequest` union (CODEX_PROTOCOL_METHODS).
// Needs no network and runs no Codex.
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { CLIENT_METHODS, OUT, generate, readPinVersion } from "./gen-codex-protocol.mjs";
import { report, rootArg } from "./walk.mjs";

export const RPC = "apps/finisherd/src/agent/codex/rpc.ts";

/** The string literals of `export const ALLOWED_METHODS … = new Set…([ … ])` in `text`, or null. */
export function allowedMethods(text) {
  const m = /export const ALLOWED_METHODS\b[^=]*=\s*new Set(?:<[^>]*>)?\(\[([^\]]*)\]\)/.exec(text);
  if (!m) return null;
  return [...m[1].matchAll(/"([^"]*)"/g)].map((x) => x[1]);
}

/** Every `ClientRequest` method of the pinned schema (stable and experimental bundles). */
export function schemaMethods(root) {
  const version = readPinVersion(root);
  const out = new Set();
  for (const dir of ["schema", "schema-experimental"]) {
    const b = JSON.parse(readFileSync(join(root, "vendor/codex", version, dir, "codex_app_server_protocol.schemas.json"), "utf8"));
    for (const member of b.definitions.ClientRequest.oneOf) out.add(member.properties.method.enum[0]);
  }
  return out;
}

export function findings(root) {
  const out = [];
  let fresh;
  try {
    fresh = generate(root);
  } catch (e) {
    return [`CODEX_PROTOCOL_DRIFT the generator failed: ${e.message}`];
  }
  const committed = existsSync(join(root, OUT)) ? readFileSync(join(root, OUT), "utf8") : null;
  if (committed !== fresh) out.push(`CODEX_PROTOCOL_DRIFT ${OUT} differs from a regeneration; run \`node scripts/gen-codex-protocol.mjs\``);
  const text = existsSync(join(root, RPC)) ? readFileSync(join(root, RPC), "utf8") : "";
  const allowed = allowedMethods(text);
  if (allowed === null) return [...out, `CODEX_PROTOCOL_METHODS ${RPC} has no ALLOWED_METHODS list`];
  const schema = schemaMethods(root);
  for (const m of allowed) if (!schema.has(m)) out.push(`CODEX_PROTOCOL_METHODS ALLOWED_METHODS names ${m}, which the pinned schema does not have`);
  const want = [...CLIENT_METHODS].sort();
  const got = [...new Set(allowed)].sort();
  if (got.length !== allowed.length) out.push("CODEX_PROTOCOL_METHODS ALLOWED_METHODS repeats a method");
  if (JSON.stringify(got) !== JSON.stringify(want))
    out.push(`CODEX_PROTOCOL_METHODS ALLOWED_METHODS (${got.join(", ")}) is not the generator's CLIENT_METHODS (${want.join(", ")})`);
  return out;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) report(findings(rootArg()));
