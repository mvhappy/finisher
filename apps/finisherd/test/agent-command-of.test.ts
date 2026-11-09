// `commandOf` (sub-project 7B Task 1, Decision 2): an incoming RPC named back to its `fin`
// command through the exported registry, so a rule holds whatever the transport; and the
// journal's argv-level naming, moved next to it.
import { FinRpcs } from "@finisher/contracts";
import { REGISTRY, RegistrySchema, type RegistryCommand, type RegistryMatch, type RegistryRpc } from "@finisher/contracts/registry";
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import { commandOf, commandOfArgv, commandsOf, RPC_ONLY } from "../src/agent/command-of.ts";

const tags = [...FinRpcs.requests.keys()];
const rpcOnly: ReadonlySet<string> = RPC_ONLY;
const named = new Set(REGISTRY.commands.flatMap((c) => c.rpcs.map((r) => r.method)));

/** Sets `value` at a dotted path; a `*` segment is the first element of an array. */
function put(node: Record<string, unknown>, path: readonly string[], value: unknown): void {
  const [head, ...rest] = path as [string, ...string[]];
  if (rest.length === 0) {
    node[head] = value;
  } else if (rest[0] === "*") {
    const list = (node[head] ??= [{}]) as Record<string, unknown>[];
    put(list[0] as Record<string, unknown>, rest.slice(1), value);
  } else {
    put((node[head] ??= {}) as Record<string, unknown>, rest, value);
  }
}

/** A payload that satisfies every condition of `rpc.match`. */
function sample(rpc: RegistryRpc): unknown {
  const root: Record<string, unknown> = {};
  for (const [path, m] of Object.entries(rpc.match ?? {})) {
    const value = typeof m === "object" ? (m.present ? "x" : undefined) : m;
    if (value !== undefined) put(root, path.split("."), value);
  }
  return root;
}

/** Commands whose RPC and conditions another, earlier command repeats: the first one is named. */
const SHARED: Record<string, string> = {
  status: "project.status",
  try: "branch.new",
  "try.keep": "merge",
  "try.drop": "branch.archive",
  "item.trim": "clip.trim",
  "item.delete": "clip.delete",
  search: "library.search",
};

const dyn = (c: RegistryCommand) => c.dynamic === true;
const tx = (...ops: object[]) => ({ projectId: "p", tx: { branch: "main", ops } });

describe("commandOf", () => {
  test("command_of_round_trip", () => {
    let checked = 0;
    for (const c of REGISTRY.commands.filter((c) => !dyn(c))) {
      for (const rpc of c.rpcs) {
        // A condition-free sample is merged with the conditions of the `rpc` alone.
        const payload = sample(rpc);
        const all = commandsOf(rpc.method, payload);
        expect(all, `${c.name} sample ${JSON.stringify(payload)}`).toContain(c.name);
        expect(commandOf(rpc.method, payload), c.name).toBe(SHARED[c.name] ?? c.name);
        checked++;
      }
    }
    expect(checked).toBeGreaterThan(100);
  });

  test("shared_rpcs_disambiguate_by_match", () => {
    // `branch.set`: a `name` is a rename whatever `archived` says; archive and restore.
    expect(commandOf("branch.set", { projectId: "p", branch: "b", name: "n" })).toBe("branch.rename");
    expect(commandOf("branch.set", { projectId: "p", branch: "b", archived: true })).toBe("branch.archive");
    expect(commandOf("branch.set", { projectId: "p", branch: "b", archived: false })).toBe("branch.archive");
    // `restore --as`: both the rename and the restore.
    expect(commandsOf("branch.set", { projectId: "p", branch: "b", name: "n", archived: false })).toEqual([
      "branch.rename",
      "branch.archive",
      "rpc:branch.set",
    ]);
    // A `null` is no value: `name: null` renames nothing.
    expect(commandsOf("branch.set", { projectId: "p", branch: "b", name: null, archived: true })).not.toContain("branch.rename");
    // `history.undo` with `to` is still `undo` (one command; `to` is an identity field).
    expect(commandOf("history.undo", { branch: "b", to: "c" })).toBe("undo");
    expect(REGISTRY.commands.find((c) => c.name === "undo")?.identity).toContain("to");
    // `tx.commit` by the ops it holds, an `item.insert` by the item's kind.
    expect(commandOf("tx.commit", tx({ op: "item.split", item: "c1" }))).toBe("item.split");
    expect(commandOf("tx.commit", tx({ op: "item.insert", kind: "clip" }))).toBe("clip.insert");
    expect(commandOf("tx.commit", tx({ op: "item.insert", kind: "image" }))).toBe("image.insert");
    expect(commandOf("tx.commit", tx({ op: "item.insert", kind: "caption" }))).toBe("caption.import");
    expect(commandOf("tx.commit", tx({ op: "caption.setText" }))).toBe("caption.set-text");
    // A command that composes ops names the most specific one it holds, and `commandsOf` all,
    // with `batch` for a transaction of several ops.
    const composed = tx({ op: "item.split", item: "c1" }, { op: "item.link", items: [] });
    expect(commandsOf("tx.commit", composed)).toEqual(["item.split", "item.link", "batch", "rpc:tx.commit"]);
    expect(commandOf("tx.commit", tx({ op: "style.add" }, { op: "weird.op" }))).toBe("style.add");
    expect(commandsOf("tx.commit", tx({ op: "style.add" }, { op: "weird.op" }))).toContain("batch");
    // No row singles the transaction out: it is a `batch`.
    expect(commandOf("tx.commit", tx({ op: "weird.op" }))).toBe("batch");
    expect(commandsOf("tx.commit", tx({ op: "weird.op" }))).toEqual(["batch", "rpc:tx.commit"]);
    // One known op is not a batch.
    expect(commandsOf("tx.commit", tx({ op: "item.split" }))).toEqual(["item.split", "rpc:tx.commit"]);
    // An update's plan is `plugin.update`; any other plan (and the approval step) `plugin.install`.
    expect(commandOf("plugin.plan", { id: "x", update: true })).toBe("plugin.update");
    expect(commandOf("plugin.plan", { id: "x" })).toBe("plugin.install");
    expect(commandOf("plugin.plan", { id: "x", update: false })).toBe("plugin.install");
    expect(commandOf("plugin.install", { planId: "p", digest: "d" })).toBe("plugin.install");
  });

  test("alias_ops_are_named_as_their_item_ops", () => {
    // fin-core takes the `clip.*` aliases on the wire and rewrites them only in the engine.
    const same = (alias: object, item: object) =>
      expect(new Set(commandsOf("tx.commit", tx(alias)))).toEqual(new Set(commandsOf("tx.commit", tx(item))));
    same({ op: "clip.trim", clip: "c1" }, { op: "item.trim", item: "c1" });
    same({ op: "clip.delete", clip: "c1" }, { op: "item.delete", item: "c1" });
    same({ op: "clip.insert", asset: "a1" }, { op: "item.insert", kind: "clip" });
    expect(commandsOf("tx.commit", tx({ op: "clip.trim" }))).toEqual(["clip.trim", "item.trim", "rpc:tx.commit"]);
    expect(commandsOf("tx.commit", tx({ op: "clip.insert" }))).toEqual(["clip.insert", "rpc:tx.commit"]);
  });

  test("padding_a_tx_keeps_its_commands", () => {
    // Policy evaluates every name: another op never hides `asset.add`.
    const padded = commandsOf("tx.commit", tx({ op: "asset.add" }, { op: "marker.add" }, { op: "weird.op" }));
    expect(padded).toContain("asset.add");
    expect(padded).toContain("marker.add");
    expect(padded).toContain("batch");
    expect(padded.at(-1)).toBe("rpc:tx.commit");
  });

  test("plugin_commands_are_named_only_from_the_host", () => {
    const body = { tx: { ops: [{ op: "item.split" }] } };
    const host = (plugin: string, command: string) => ({ via: { plugin, command } });
    expect(commandOf("tx.commit", body, host("@acme/tools@1.2.0", "trim-silence"))).toBe("plugin:@acme/tools/trim-silence");
    expect(commandOf("tx.commit", body, host("plain@3.0.0", "go"))).toBe("plugin:plain/go");
    expect(commandsOf("tx.commit", body, host("plain@3.0.0", "go"))).toEqual(["plugin:plain/go", "item.split", "rpc:tx.commit"]);
    expect(REGISTRY.commands.some((c) => c.name === "plugin:<id>/<command>" && c.dynamic === true)).toBe(true);
    // A payload's own `via` is the caller's claim: it names nothing.
    const claimed = { tx: { ops: [{ op: "item.split" }], via: { plugin: "trusted@1.0.0", command: "go" } } };
    expect(commandsOf("tx.commit", claimed).some((n) => n.startsWith("plugin:"))).toBe(false);
    expect(commandsOf("tx.commit", { ...claimed, via: claimed.tx.via }).some((n) => n.startsWith("plugin:"))).toBe(false);
  });

  test("every_rpc_also_names_rpc_method", () => {
    for (const tag of tags) expect(commandsOf(tag, {}).at(-1), tag).toBe(`rpc:${tag}`);
    expect(commandsOf("project.status", { projectId: "p" })).toEqual(["project.status", "status", "rpc:project.status"]);
  });

  test("an_rpc_no_command_names_is_rpc_method", () => {
    expect(commandOf("agent.self", {})).toBe("rpc:agent.self");
    expect(commandsOf("tx.get", {})).toEqual(["rpc:tx.get"]);
    // `view`'s base snapshot is every edit command's too: it names no command.
    expect(commandsOf("snapshot.get", { projectId: "p", branch: "b" })).toEqual(["rpc:snapshot.get"]);
    expect(REGISTRY.commands.find((c) => c.name === "view")?.rpcs).toEqual([]);
    // Payloads of any shape are named, never thrown on.
    for (const p of [null, undefined, 5, "x", [], {}]) expect(typeof commandOf("tx.commit", p)).toBe("string");
  });

  test("a_render_job_is_render", () => {
    // Server-side naming until a `fin` render command exists.
    expect(commandsOf("jobs.submit", { kind: "render", params: {} })).toEqual(["render", "rpc:jobs.submit"]);
    expect(commandOf("jobs.submit", { kind: "render", params: {} })).toBe("render");
    expect(commandsOf("jobs.submit", { kind: "proxy", params: {} })).toEqual(["rpc:jobs.submit"]);
  });

  test("every_rpc_is_named_or_rpc_only", () => {
    for (const tag of tags) {
      expect(named.has(tag) || rpcOnly.has(tag), `${tag} is neither a command's RPC nor in RPC_ONLY`).toBe(true);
      expect(named.has(tag) && rpcOnly.has(tag), `${tag} is both named and RPC_ONLY`).toBe(false);
    }
    for (const tag of rpcOnly) expect(tags, `RPC_ONLY lists ${tag}, which is no RPC`).toContain(tag);
    for (const m of named) expect(tags, `the registry names ${m}, which is no RPC`).toContain(m);
  });

  test("registry_is_wellformed", () => {
    // The committed JSON is decoded, not cast: a malformed one is refused.
    const decode = Schema.decodeUnknownSync(RegistrySchema);
    expect(() => decode({ version: 1, commands: [{ name: "x" }] })).toThrow();
    expect(() => decode({ version: 1, commands: [{ ...REGISTRY.commands[0], argKinds: { a: "file" } }] })).toThrow();
    expect(decode(REGISTRY)).toEqual(REGISTRY);
    expect(REGISTRY.version).toBe(1);
    expect(new Set(REGISTRY.commands.map((c) => c.name)).size).toBe(REGISTRY.commands.length);
    for (const c of REGISTRY.commands) {
      for (const rpc of c.rpcs)
        for (const [field, m] of Object.entries(rpc.match ?? {}) as [string, RegistryMatch][]) {
          expect(field).toMatch(/^[A-Za-z*][A-Za-z0-9.*]*$/);
          expect(["string", "boolean", "object"]).toContain(typeof m);
        }
    }
  });
});

describe("the journal's argv naming", () => {
  test("journal_command_names_unchanged", () => {
    expect(commandOfArgv({ argv: ["item", "trim", "c1"] })).toBe("item.trim");
    expect(commandOfArgv({ argv: ["view", "--budget", "2000"] })).toBe("view");
    expect(commandOfArgv({ argv: ["--json"] })).toBeUndefined();
    expect(commandOfArgv("x")).toBeUndefined();
    expect(commandOfArgv({ argv: ["item", "trim", "c1", "x"] })).toBe("item.trim");
    expect(commandOfArgv({ argv: ["branch", "ls", "--all"] })).toBe("branch.ls");
    expect(commandOfArgv({ argv: [] })).toBeUndefined();
  });
});
