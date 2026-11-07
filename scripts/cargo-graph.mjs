// Shared `cargo metadata` helpers for the native build gate and the crate-dependency gate
// (sub-project 6 spec §16.3, §18). A gate reads the live metadata, or a fake document passed
// with `--metadata <file>` (tests).
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

/** `cargo metadata` for the workspace at `root`, all features (as `deny.toml` resolves them). */
export function metadata(root, target) {
  const args = ["metadata", "-q", "--format-version", "1", "--all-features", "--manifest-path", join(root, "Cargo.toml")];
  if (target) args.push("--filter-platform", target);
  const r = spawnSync("cargo", args, { encoding: "utf8", maxBuffer: 1 << 30 });
  if (r.error || r.status !== 0) throw new Error(`cargo metadata${target ? ` --filter-platform ${target}` : ""} failed: ${r.stderr ?? r.error?.message}`);
  return JSON.parse(r.stdout);
}

/** The metadata documents a gate checks: `--metadata <file>` (one document) or `load()`. */
export function documents(argv, load) {
  const i = argv.indexOf("--metadata");
  return i === -1 ? load() : [JSON.parse(readFileSync(argv[i + 1], "utf8"))];
}

/** The `[graph] targets` of `deny.toml`: the shipped OS triples. */
export function denyTargets(root) {
  const text = readFileSync(join(root, "deny.toml"), "utf8");
  const m = text.match(/^\s*targets\s*=\s*\[([^\]]*)\]/m);
  return m ? [...m[1].matchAll(/"([^"]+)"/g)].map((x) => x[1]) : [];
}

/** A dependency graph over one metadata document. Edges keep their kinds (null = normal). */
export class Graph {
  constructor(doc) {
    this.packages = new Map(doc.packages.map((p) => [p.id, p]));
    this.members = new Set(doc.workspace_members ?? []);
    this.edges = new Map();
    for (const node of doc.resolve?.nodes ?? []) {
      this.edges.set(
        node.id,
        (node.deps ?? []).map((d) => ({ pkg: d.pkg, kinds: (d.dep_kinds ?? [{ kind: null }]).map((k) => k.kind ?? null) })),
      );
    }
  }

  name(id) {
    return this.packages.get(id)?.name ?? id;
  }

  /** The id of the workspace member called `name`. */
  member(name) {
    for (const id of this.members) if (this.name(id) === name) return id;
    return undefined;
  }

  /** Package ids reachable from `roots` over edges of the given kinds (normal and build by default). */
  closure(roots, kinds = [null, "build"]) {
    const seen = new Set();
    const stack = [...roots];
    while (stack.length) {
      const id = stack.pop();
      if (seen.has(id)) continue;
      seen.add(id);
      for (const e of this.edges.get(id) ?? []) if (e.kinds.some((k) => kinds.includes(k))) stack.push(e.pkg);
    }
    return seen;
  }

  /** Names of `id`'s direct dependencies over edges of `kind`. */
  direct(id, kind) {
    return (this.edges.get(id) ?? []).filter((e) => e.kinds.includes(kind)).map((e) => this.name(e.pkg));
  }
}

/** Whether an SPDX expression is satisfied by the `allow` list: AND needs both sides, OR
 * either, `X WITH exception` counts as `X` or as the whole `X WITH exception` string. */
export function spdxAllowed(expr, allow) {
  const tokens = expr.replace(/\(/g, " ( ").replace(/\)/g, " ) ").replace(/\//g, " OR ").split(/\s+/).filter(Boolean);
  let i = 0;
  const atom = () => {
    if (tokens[i] === "(") {
      i++;
      const v = or();
      i++;
      return v;
    }
    let id = tokens[i++] ?? "";
    if (tokens[i] === "WITH") {
      const whole = `${id} WITH ${tokens[i + 1]}`;
      i += 2;
      return allow.includes(whole) || allow.includes(id);
    }
    return allow.includes(id);
  };
  const and = () => {
    let v = atom();
    while (tokens[i] === "AND") (i++, (v = atom() && v));
    return v;
  };
  const or = () => {
    let v = and();
    while (tokens[i] === "OR") (i++, (v = and() || v));
    return v;
  };
  return tokens.length > 0 && or();
}
