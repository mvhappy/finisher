// The `fin` runs a shell command line shows statically (plan 7D Task 12, Decision 20; spec
// §12.3; review of Task 12). Harness-neutral: Claude Code's `Bash` and 7E's Codex shell hand
// their command strings here.
//
// **An allowlist, not a denylist.** A line is read statically (not opaque) only when the whole
// of it is in this small grammar; anything else is **opaque**: it may run `fin` in a way no hash
// can be derived for, so its call is a candidate for every commit in its window.
// - Words: unquoted text, `'…'`, `"…"` without `$` or a backquote, backslash escapes, line
//   continuations. Any `$` (a parameter, `${x:=…}`, `$(…)`, `$((…))`) or backquote anywhere
//   is opaque.
// - Between simple commands: `;`, `&&`, `||`, `|`, `&`, newlines; `#` comments. No `(`, `)`,
//   here-documents or here-strings (`<<`), no process substitution.
// - Redirections `<`, `>`, `>>`, `>|`, `&>`, `&>>`, `<>`, `N>`, `N>&M`, `>&N`, with a plain target.
// - Each simple command starts with its program, as a plain word: no assignment before it, no
//   reserved word, no wrapper. The program is
//   - `fin` (resolved through the session's `PATH`, which the launch sets; nothing in the line
//     can change it, since assignments and `export` are not in the grammar), or exactly the
//     configured `fin` path (`agents.finPath`): a `fin` run, whose words must not be expanded
//     (no glob, brace or `~`), or the line is opaque;
//   - or one of `SAFE`, commands that take data and never run another program or change the
//     shell's state that `fin` sees: `echo` and `pwd` (builtins that print), `true`/`false`
//     (exit statuses), `cd` (the directory only: `fin`'s hash takes its arguments as written),
//     `ls`, `cat` (read files) and `sleep` (waits). Their words may be globbed.
//   Everything else (an interpreter, a script, `git`, `env`, `command`, `exec`, `xargs`, `find`,
//   `alias`, `source`, a function definition, a group) is opaque.

/** A word as the shell would pass it: its text, whether any of it was quoted or escaped, and whether it would be globbed. */
interface Word {
  readonly text: string;
  readonly quoted: boolean;
  readonly globbed: boolean;
}

/** What a command line runs of `fin`: each visible run's argv (after `fin`), and whether it may run more. */
export interface FinRuns {
  readonly runs: ReadonlyArray<ReadonlyArray<string>>;
  readonly opaque: boolean;
}

const OPAQUE: FinRuns = { runs: [], opaque: true };

/** Programs that read or print data and never run another program (module docs). */
const SAFE: ReadonlySet<string> = new Set(["echo", "pwd", "true", "false", "cd", "ls", "cat", "sleep"]);

/** Reads `s` into simple commands; undefined when anything is outside the grammar. */
function commandsOf(s: string): Word[][] | undefined {
  const commands: Word[][] = [[]];
  const end = () => {
    if (commands.at(-1)!.length > 0) commands.push([]);
  };
  let i = 0;
  /** One word from `i`; undefined outside the grammar. */
  const word = (): Word | undefined => {
    let text = "";
    let quoted = false;
    let globbed = false;
    while (i < s.length) {
      const c = s[i]!;
      if (" \t\n;&|()<>".includes(c)) break;
      if (c === "$" || c === "`") return undefined;
      if (c === "'") {
        const close = s.indexOf("'", i + 1);
        if (close < 0) return undefined;
        text += s.slice(i + 1, close);
        quoted = true;
        i = close + 1;
      } else if (c === '"') {
        quoted = true;
        i++;
        for (;;) {
          if (i >= s.length) return undefined;
          const d = s[i]!;
          if (d === '"') break;
          if (d === "$" || d === "`") return undefined;
          if (d === "\\" && '$`"\\\n'.includes(s[i + 1] ?? "")) {
            if (s[i + 1] !== "\n") text += s[i + 1];
            i += 2;
            continue;
          }
          text += d;
          i++;
        }
        i++;
      } else if (c === "\\") {
        const n = s[i + 1];
        if (n === undefined) return undefined;
        if (n !== "\n") {
          text += n;
          quoted = true;
        }
        i += 2;
      } else {
        if ("*?[{".includes(c) || (c === "~" && text === "" && !quoted)) globbed = true;
        text += c;
        i++;
      }
    }
    return { text, quoted, globbed };
  };
  /** A redirection's operator at `i` (after any fd digits) and its plain target. */
  const redirect = (): boolean => {
    if (s.startsWith("<<", i) || s.startsWith("<(", i) || s.startsWith(">(", i)) return false;
    if (s[i] === "&") i++; // `&>`, `&>>`
    i++;
    if (s[i] === ">" || s[i] === "|") i++; // `>>`, `>|`, `<>`
    else if (s[i] === "&") {
      // `>&N`, `>&-`: a descriptor, not a file.
      i++;
      const m = /^(?:\d+|-)/.exec(s.slice(i));
      if (m === null) return false;
      i += m[0].length;
      return true;
    }
    while (s[i] === " " || s[i] === "\t") i++;
    if (s[i] === "(") return false;
    const target = word();
    return target !== undefined && target.text !== "" && !target.globbed;
  };
  while (i < s.length) {
    const c = s[i]!;
    if (c === " " || c === "\t") i++;
    else if (c === "\\" && s[i + 1] === "\n") i += 2;
    else if (c === "\n" || c === ";") {
      i++;
      end();
    } else if (c === "(" || c === ")") return undefined;
    else if (c === "&" && s[i + 1] === ">") {
      if (!redirect()) return undefined;
    } else if (c === "&" || c === "|") {
      i += s[i + 1] === c || (c === "|" && s[i + 1] === "&") ? 2 : 1;
      end();
    } else if (c === "#" && (i === 0 || " \t\n;&|".includes(s[i - 1]!))) {
      while (i < s.length && s[i] !== "\n") i++;
    } else if (c === "<" || c === ">") {
      if (!redirect()) return undefined;
    } else {
      const w = word();
      if (w === undefined) return undefined;
      // An fd number right before a redirection belongs to it (`2>&1`).
      if ((s[i] === "<" || s[i] === ">") && /^\d+$/.test(w.text) && !w.quoted) {
        if (!redirect()) return undefined;
        continue;
      }
      commands.at(-1)!.push(w);
    }
  }
  return commands.filter((c) => c.length > 0);
}

/** The `fin` runs of `command` (module docs); `finPath` is the configured `fin`, also trusted by its exact path. */
export function finRuns(command: string, finPath?: string): FinRuns {
  const commands = commandsOf(command);
  if (commands === undefined) return OPAQUE;
  const runs: string[][] = [];
  for (const [program, ...args] of commands) {
    if (program!.globbed) return OPAQUE;
    if (program!.text === "fin" || (finPath !== undefined && program!.text === finPath)) {
      if (args.some((w) => w.globbed)) return OPAQUE;
      runs.push(args.map((w) => w.text));
      continue;
    }
    // A quoted program word is never a reserved word or assignment to the shell, but it is not
    // a safe command either: only the plain spelling counts.
    if (program!.quoted || !SAFE.has(program!.text)) return OPAQUE;
  }
  return { runs, opaque: false };
}
