// The JS licence gate's logic (`check-js-licenses.mjs` is its command line): which packages ship,
// and whether an SPDX licence expression allows shipping them.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const ALLOWED = new Set([
  "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "0BSD", "MPL-2.0",
  "CC0-1.0", "CC-BY-4.0", "Unlicense", "BlueOak-1.0.0", "Python-2.0", "Zlib",
]);

const ROOT = fileURLToPath(new URL("..", import.meta.url));

/**
 * `pnpm licenses list --json --prod` for **every** workspace (`-r`): at the root alone it lists
 * the root's production dependencies, of which there are none, so nothing was checked.
 */
export function discover(root = ROOT) {
  return execFileSync("pnpm", ["-r", "licenses", "list", "--json", "--prod"], {
    cwd: root,
    encoding: "utf8",
    shell: process.platform === "win32",
    maxBuffer: 64 * 1024 * 1024,
  });
}

const ID = /^[A-Za-z0-9][A-Za-z0-9.-]*\+?$|^(?:DocumentRef-[A-Za-z0-9.-]+:)?LicenseRef-[A-Za-z0-9.-]+$/;

function tokens(expr) {
  return expr.replace(/[()]/g, " $& ").trim().split(/\s+/).filter(Boolean);
}

/**
 * Parses an SPDX licence expression (SPDX 2.3 annex D) by recursive descent: `OR` binds looser
 * than `AND`, `AND` looser than `WITH`, parentheses group. Operators and ids are case-sensitive.
 * Returns a tree of `{or: [..]}`, `{and: [..]}`, `{id, exception?}`; throws on a malformed one.
 */
export function parse(expr) {
  const ts = tokens(expr);
  let i = 0;
  const peek = () => ts[i];
  const take = (want) => {
    const t = ts[i];
    if (t === undefined) throw new Error(`expected ${want ?? "a licence"} at the end`);
    if (want && t !== want) throw new Error(`expected ${want}, found ${JSON.stringify(t)}`);
    i += 1;
    return t;
  };
  const list = (op, next) => {
    const items = [next()];
    while (peek() === op) {
      take(op);
      items.push(next());
    }
    return items.length === 1 ? items[0] : { [op.toLowerCase()]: items };
  };
  const or = () => list("OR", and);
  const and = () => list("AND", withExpr);
  function withExpr() {
    const base = atom();
    if (peek() !== "WITH") return base;
    take("WITH");
    if (!("id" in base)) throw new Error("WITH needs a single licence before it");
    const exception = take();
    if (!ID.test(exception) || ["AND", "OR", "WITH", "(", ")"].includes(exception)) {
      throw new Error(`${JSON.stringify(exception)} is not a licence exception`);
    }
    return { ...base, exception };
  }
  function atom() {
    if (peek() === "(") {
      take("(");
      const inner = or();
      take(")");
      return inner;
    }
    const t = take();
    if (["AND", "OR", "WITH", ")"].includes(t) || !ID.test(t)) {
      throw new Error(`${JSON.stringify(t)} is not a licence id`);
    }
    return { id: t };
  }
  const tree = or();
  if (i !== ts.length) throw new Error(`unexpected ${JSON.stringify(ts[i])}`);
  return tree;
}

/** `{ok, blockers}`: whether `tree` allows shipping, and the ids that keep it from doing so. */
function judge(tree, allowed) {
  if ("id" in tree) {
    // An exception only grants permissions: it is as allowed as its licence. `+` is "or later".
    const ok = allowed.has(tree.id.replace(/\+$/, ""));
    return { ok, blockers: ok ? [] : [tree.id] };
  }
  const parts = (tree.or ?? tree.and).map((t) => judge(t, allowed));
  const ok = tree.or ? parts.some((p) => p.ok) : parts.every((p) => p.ok);
  const blockers = ok ? [] : [...new Set(parts.flatMap((p) => p.blockers))];
  return { ok, blockers };
}

/** `null` when `expr` may ship, else why not (the blocking ids, or the parse error). */
export function refusal(expr, allowed = ALLOWED) {
  let tree;
  try {
    tree = parse(expr);
  } catch (e) {
    return `not a valid SPDX expression: ${e.message}`;
  }
  const { ok, blockers } = judge(tree, allowed);
  return ok ? null : `blocked by ${blockers.join(", ")}`;
}

/** The violations of a `pnpm licenses list --json` report; throws when it names no package. */
export function violations(report, allowlist = {}, allowed = ALLOWED) {
  const entries = Object.entries(report);
  if (!entries.some(([, pkgs]) => pkgs.length > 0)) {
    throw new Error("the licence report names no package: the gate would check nothing");
  }
  const out = [];
  for (const [license, pkgs] of entries) {
    const why = refusal(license, allowed);
    if (why === null) continue;
    for (const pkg of pkgs) {
      if (allowlist[pkg.name]) continue;
      for (const version of pkg.versions ?? [pkg.version]) {
        out.push(`${pkg.name}@${version}: ${license} is not allowed (${why})`);
      }
    }
  }
  return out;
}
