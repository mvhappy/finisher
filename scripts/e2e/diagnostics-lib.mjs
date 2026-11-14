// Helpers of the "diagnostics" e2e scenario: reading the journal, content fields from the
// generated schema, the leak scan and the install key's ids.
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, statSync } from "node:fs";
import { hostname } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { TEST_NATIVE_DIR } from "./fixture.mjs";
import { ROOT, finRaw } from "./lib.mjs";

/** `fin diagnostics export [args]` as entries; throws on a non-zero exit. */
export function exportEntries(serverData, cwd, args = [], env = {}) {
  const r = finRaw(serverData, cwd, ["diagnostics", "export", ...args], env);
  if (r.status !== 0) throw new Error(`fin diagnostics export ${args.join(" ")} exited ${r.status}: ${r.text} ${r.stderr}`);
  return parseJsonl(r.text);
}

/** JSONL text as objects (blank lines ignored). */
export function parseJsonl(text) {
  return text
    .split("\n")
    .filter((l) => l.trim().length > 0)
    .map((l) => JSON.parse(l));
}

/** The schema definition of a kind's payload: `log.commit` → `LogCommit`. */
export function kindDef(kind) {
  return kind
    .split(/[._]/)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join("");
}

/**
 * Payload fields classed `content` (`x-fin-class: content`) per schema definition, from the
 * generated fin-core schema: `{ LogCommit: Set{"intent"}, … }`.
 */
export function contentFields(schema) {
  const isContent = (s) => s?.["x-fin-class"] === "content" || (Array.isArray(s?.allOf) && s.allOf.some(isContent));
  const out = {};
  for (const [name, def] of Object.entries(schema.$defs ?? {})) {
    const props = Object.entries(def.properties ?? {}).filter(([, s]) => isContent(s));
    if (props.length > 0) out[name] = new Set(props.map(([p]) => p));
  }
  return out;
}

export function loadContentFields() {
  return contentFields(JSON.parse(readFileSync(join(ROOT, "packages", "core-types", "schema", "fin-core.schema.json"), "utf8")));
}

/**
 * Problems with an entry recorded without training: a `training` purpose in its stamp, or a
 * payload field the schema classes as content (`fields` from [`contentFields`]).
 */
export function contentProblems(entry, fields) {
  const problems = [];
  if (entry.consent?.purposes?.includes("training")) problems.push(`${entry.kind} ${entry.id} is stamped training`);
  for (const f of fields[kindDef(entry.kind)] ?? []) {
    if (entry.payload && Object.hasOwn(entry.payload, f)) problems.push(`${entry.kind} ${entry.id} has content field ${f}`);
  }
  return problems;
}

/** Every file under `dir` (recursively) as `[path, bytes]`; none when `dir` is missing. */
export function filesUnder(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir).flatMap((name) => {
    const p = join(dir, name);
    return statSync(p).isDirectory() ? filesUnder(p) : [[p, readFileSync(p)]];
  });
}

/**
 * The needles found in `files` (`[path, bytes]`), as `{ file, needle }`. An unkeyed
 * `blake3:` hash is a leak too: every hash on disk is keyed (`k1:`). Empty needles are skipped.
 */
export function leaks(files, needles) {
  const found = [];
  for (const [file, bytes] of files) {
    for (const needle of [...needles, "blake3:"]) {
      if (needle && bytes.includes(Buffer.from(needle, "utf8"))) found.push({ file, needle });
    }
  }
  return found;
}

/**
 * Ids as the journal of `serverData` stores them (a ULID raw, anything else keyed with its
 * install key): a test-hooks `Journal` opened on a copy of `events/key` in `scratch`, so the
 * live server's journal is never opened twice. Returns `{ idRef(raw), close() }`.
 */
export async function keyer(serverData, scratch) {
  process.env.FIN_NODE_BINARY_DIR = TEST_NATIVE_DIR;
  const { Journal } = await import(pathToFileURL(join(ROOT, "packages", "fin-node", "index.js")).href);
  mkdirSync(join(scratch, "events"), { recursive: true, mode: 0o700 });
  copyFileSync(join(serverData, "events", "key"), join(scratch, "events", "key"));
  const j = Journal.open(scratch, JSON.stringify({ diagnostics: "on" }), hostname());
  return {
    idRef(raw) {
      const id = j.idRefForTest(raw);
      if (id === undefined) throw new Error("the key journal is off");
      return id;
    },
    close: () => j.close(),
  };
}
