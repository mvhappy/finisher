// The `fin` runs a shell command line shows statically (plan 7D Task 12, Decision 20; spec
// §12.3). Harness-neutral: Claude Code's `Bash` and 7E's Codex shell hand their command strings
// here.
//
// A POSIX `sh` subset is read: words with single and double quotes and backslashes, the list and
// pipe operators (`;`, `&`, `&&`, `||`, `|`, `|&`, newlines), subshell parentheses, redirections
// (their targets are not arguments), comments, assignments before a program, the reserved words
// (`if`, `then`, `do`, `{`, ...), and a few wrappers that run their argument as the program
// (`nohup`, `time`, `command`, `exec`, `setsid`, `env`, `nice`, `timeout`).
//
// What it cannot follow makes the line **opaque** (it may run `fin` in a way no hash can be
// derived for): a here-document, `case`, `function`, a command substitution (`$(...)`, a
// backquote), an expanded word (`$x`, a glob, a brace, `~`) in a `fin` run or as a program, a
// program given by path (a script) other than `fin`, a shell or other indirection (`sh`, `bash`,
// `eval`, `source`, `xargs`, `sudo`, ...), or any other word that names `fin` (`find -exec fin`).

/** A word as the shell would pass it, and whether it would be expanded first. */
interface Word {
  readonly text: string;
  readonly expanded: boolean;
}

/** What a command line runs of `fin`: each visible run's argv (after `fin`), and whether it may run more. */
export interface FinRuns {
  readonly runs: ReadonlyArray<ReadonlyArray<string>>;
  readonly opaque: boolean;
}

const OPAQUE: FinRuns = { runs: [], opaque: true };

/** Reserved words skipped in a program's place. */
const RESERVED = new Set(["if", "then", "else", "elif", "fi", "do", "done", "while", "until", "!", "{", "}", "[[", "]]"]);
/** Reserved words whose bodies this reader does not follow. */
const UNFOLLOWED = new Set(["case", "esac", "function", "coproc", "select"]);
/** Programs that run other programs from their arguments or from a file. */
const INDIRECT = new Set(["sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "csh", "tcsh", "busybox", "eval", "source", ".", "xargs", "parallel", "sudo", "su", "doas", "watch", "script", "flock", "chroot", "unshare", "nsenter", "systemd-run"]);
/** Wrappers whose program is the first word after their options (and `timeout`'s duration). */
const WRAPPERS = new Set(["nohup", "time", "command", "exec", "setsid", "env", "nice", "timeout"]);
/** A word that names `fin` as a program, in any quoting. */
const NAMES_FIN = /(?:^|[\s/;&|'"`(=])fin(?:\.exe)?(?:$|[\s;&|'"`)])/;

const isFin = (program: string) => program === "fin" || program === "fin.exe" || /[/\\]fin(?:\.exe)?$/.test(program);
const isAssignment = (w: Word) => /^[A-Za-z_][A-Za-z0-9_]*=/.test(w.text) && !w.expanded;

class Lexer {
  readonly #s: string;
  #i = 0;
  /** The simple commands so far, each a list of words. */
  readonly commands: Word[][] = [[]];
  opaque = false;

  constructor(s: string) {
    this.#s = s;
  }

  #end(): void {
    if (this.commands.at(-1)!.length > 0) this.commands.push([]);
  }

  /** Reads the whole line; false when it cannot (unbalanced quotes, unsupported syntax). */
  read(): boolean {
    const s = this.#s;
    while (this.#i < s.length) {
      const c = s[this.#i]!;
      if (c === " " || c === "\t") {
        this.#i++;
      } else if (c === "\\" && s[this.#i + 1] === "\n") {
        this.#i += 2; // a line continuation between words
      } else if (c === "\n" || c === ";" || c === "(" || c === ")") {
        this.#i++;
        this.#end();
      } else if (c === "&" || c === "|") {
        if (c === "&" && s[this.#i + 1] === ">") {
          // `&>file`, `&>>file`: a redirection of both streams.
          this.#i += s[this.#i + 2] === ">" ? 3 : 2;
          if (this.#word() === undefined) return false;
          continue;
        }
        this.#i += s[this.#i + 1] === c || (c === "|" && s[this.#i + 1] === "&") ? 2 : 1;
        this.#end();
      } else if (c === "#") {
        while (this.#i < s.length && s[this.#i] !== "\n") this.#i++;
      } else if (c === "<" || c === ">") {
        if (!this.#redirect()) return false;
      } else {
        const w = this.#word();
        if (w === undefined) return false;
        // An fd number right before a redirection belongs to it (`2>&1`).
        const next = s[this.#i];
        if ((next === "<" || next === ">") && /^\d+$/.test(w.text) && !w.expanded) continue;
        this.commands.at(-1)!.push(w);
      }
    }
    return true;
  }

  /** A redirection at `<` or `>`: its operator and target word. */
  #redirect(): boolean {
    const s = this.#s;
    if (s.startsWith("<<", this.#i)) return false; // a here-document (or here-string): not followed
    this.#i += s[this.#i + 1] === ">" || s[this.#i + 1] === "&" || s[this.#i + 1] === "|" ? 2 : 1;
    while (s[this.#i] === " " || s[this.#i] === "\t") this.#i++;
    return this.#word() !== undefined;
  }

  /** One word from the cursor; undefined when a quote is not closed or a substitution starts. */
  #word(): Word | undefined {
    const s = this.#s;
    let text = "";
    let expanded = false;
    const start = this.#i;
    while (this.#i < s.length) {
      const c = s[this.#i]!;
      if (" \t\n;&|()<>".includes(c)) break;
      if (c === "'") {
        const close = s.indexOf("'", this.#i + 1);
        if (close < 0) return undefined;
        text += s.slice(this.#i + 1, close);
        this.#i = close + 1;
      } else if (c === '"') {
        this.#i++;
        for (;;) {
          if (this.#i >= s.length) return undefined;
          const d = s[this.#i]!;
          if (d === '"') break;
          if (d === "`" || (d === "$" && s[this.#i + 1] === "(")) return undefined;
          if (d === "$") expanded = true;
          if (d === "\\" && '$`"\\\n'.includes(s[this.#i + 1] ?? "")) {
            if (s[this.#i + 1] !== "\n") text += s[this.#i + 1];
            this.#i += 2;
            continue;
          }
          text += d;
          this.#i++;
        }
        this.#i++;
      } else if (c === "\\") {
        const n = s[this.#i + 1];
        if (n === undefined) return undefined;
        if (n !== "\n") text += n;
        this.#i += 2;
      } else if (c === "`" || (c === "$" && s[this.#i + 1] === "(")) {
        return undefined;
      } else {
        if (c === "$" || c === "*" || c === "?" || c === "[" || c === "{" || (c === "~" && this.#i === start)) expanded = true;
        text += c;
        this.#i++;
      }
    }
    // A group's brace and the test commands are words of their own, never expanded.
    if (this.#i - start === text.length && (text === "{" || text === "[" || text === "[[")) expanded = false;
    return { text, expanded };
  }
}

/** The words after a wrapper's options (and `timeout`'s duration, `nice -n`'s value). */
function unwrap(words: Word[]): Word[] {
  let rest = words;
  for (;;) {
    const program = rest[0];
    if (program === undefined || program.expanded || !WRAPPERS.has(program.text)) return rest;
    const name = program.text;
    rest = rest.slice(1);
    while (rest[0] !== undefined && (rest[0].text.startsWith("-") || (name === "env" && isAssignment(rest[0])))) {
      const flag = rest[0].text;
      rest = rest.slice(name === "nice" && flag === "-n" ? 2 : 1);
    }
    if (name === "timeout") rest = rest.slice(1);
  }
}

/** The `fin` runs of `command` (module docs). */
export function finRuns(command: string): FinRuns {
  const lexer = new Lexer(command);
  if (!lexer.read()) return OPAQUE;
  const runs: string[][] = [];
  let opaque = false;
  for (const words of lexer.commands) {
    let i = 0;
    while (i < words.length && (isAssignment(words[i]!) || (!words[i]!.expanded && RESERVED.has(words[i]!.text)))) i++;
    const rest = unwrap(words.slice(i));
    const program = rest[0];
    if (program === undefined) continue;
    if (program.expanded) {
      opaque = true;
      continue;
    }
    if (UNFOLLOWED.has(program.text)) return OPAQUE;
    if (isFin(program.text)) {
      const argv = rest.slice(1);
      if (argv.some((w) => w.expanded)) opaque = true;
      else runs.push(argv.map((w) => w.text));
      continue;
    }
    if (INDIRECT.has(program.text) || /[/\\]/.test(program.text) || rest.slice(1).some((w) => NAMES_FIN.test(` ${w.text} `))) opaque = true;
  }
  return { runs, opaque };
}
