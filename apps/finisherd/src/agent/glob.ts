// Command globs for command rules (sub-project 7B Decision 3, spec §7.2): a pattern matches a
// command name segment-wise. Segments are separated by `.` (`asset.add`, `marker.add`): `*`
// matches within one segment, `**` across segments, every other character itself.
//
// Three kinds of name are free text, with no segments: plugin commands (`plugin:<id>/<cmd>`,
// sp8 §3.8, whose ids may hold `/` and `.`: `plugin:@acme/*` is every command of every `@acme`
// plugin, `plugin:*/publish` every plugin's `publish`), RPC names (`rpc:<method>`: `rpc:*` is
// every RPC, `rpc:history.*` every `history.` one) and provider tool requests
// (`tool:Bash(curl https://x.y)`, Decision 13: a tool's input is text), where `*` and `**` both
// match any run of characters. Matching is exact otherwise (case included) and whole-name;
// command names are lowercase, so `RuleCommand` refuses a pattern with an uppercase letter
// outside a `tool:` rule (it could match nothing).
//
// Matching is one pass of dynamic programming over the pattern's tokens (time pattern × name,
// no backtracking), so no pattern can make a long tool input slow to match.

/** Names whose `*` crosses every character (see the module docs). */
const FREE_TEXT = ["plugin:", "rpc:", "tool:"] as const;

/** A literal character, `*` (within a segment) or `**` (anything). */
type Token = string | 1 | 2;

function tokens(pattern: string): Token[] {
  const free = FREE_TEXT.some((p) => pattern.startsWith(p));
  const out: Token[] = [];
  for (let i = 0; i < pattern.length; i++) {
    const c = pattern[i]!;
    if (c !== "*") {
      out.push(c);
      continue;
    }
    const double = pattern[i + 1] === "*";
    if (double) i++;
    const t: Token = double || free ? 2 : 1;
    // Runs of stars are one star (the widest of them).
    const last = out[out.length - 1];
    if (last === 1 || last === 2) out[out.length - 1] = Math.max(last, t) as 1 | 2;
    else out.push(t);
  }
  return out;
}

/** Whether `name` (a command name, `rpc:<method>`, `plugin:…`, `tool:…`) matches `pattern`. */
export function globMatch(pattern: string, name: string): boolean {
  const n = name.length;
  // reach[j]: the tokens so far match name[0, j).
  let reach = new Uint8Array(n + 1);
  reach[0] = 1;
  for (const t of tokens(pattern)) {
    const next = new Uint8Array(n + 1);
    if (t === 1 || t === 2) {
      for (let j = 0; j <= n; j++) next[j] = reach[j]! || (j > 0 && next[j - 1]! && (t === 2 || name[j - 1] !== ".")) ? 1 : 0;
    } else {
      for (let j = 1; j <= n; j++) next[j] = reach[j - 1]! && name[j - 1] === t ? 1 : 0;
    }
    reach = next;
  }
  return reach[n] === 1;
}
