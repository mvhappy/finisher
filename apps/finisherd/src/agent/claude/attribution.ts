// Inferred attribution of a CLI agent's own shell calls (plan 7D Task 12, Decision 20; spec
// §12.3). Harness-neutral: Claude Code's `Bash` today, 7E's Codex shell through the same class.
//
// A CLI's shell cannot be given `FIN_CALL` per tool call, so a commit its `fin` makes arrives
// with no call. The runtime ties it to a call by what it saw:
// - **The window** of a shell call opens at the runtime's allow decision for it (a policy
//   allow, or a person's answer to its card: while the sandbox is not enforced every shell call
//   asks, owner Q3a) and closes at its tool result (the live one, or the synthetic one an
//   interrupt or recovery writes): `allowed` before the answer is sent, `journal`'s `toolResult`.
// - **The key** is the hash `fin` sends in its `fin-args-hash` header (`crates/fin/src/diag.rs`
//   `describe`: `{args, cmd, flags}` of the parse, BLAKE3 of its JSON). At the allow, each `fin`
//   run the command string shows statically (`shell-words.ts`) is read with `fin schema
//   commands` (`parsedArgs`) and hashed the same way.
// - **The match** (the chain's after stage, `reach.ts`, for a session's write with no `FIN_CALL`):
//   among the session's open windows, the candidates are those whose derived hashes hold the
//   request's and those that may run `fin` unseen (opaque: shell expansion, a script, an
//   interpreter). Exactly one candidate, matched by its hash: `inferred`, the commit carries
//   that call. Several, or only opaque ones: `ambiguous`, no call on the commit, and the
//   txIds listed on every candidate's tool result. None (no window open, or none that could
//   run `fin`, e.g. a process an earlier call left in the background): `none`.
// - The header is the session's own word (only `agent` principals are matched): a session can
//   at worst misattribute its own commits among its own calls.
// - Windows live in memory: a restart closes every call (recovery writes their results), so
//   none is left open across one.
import type { Attribution } from "@finisher/core-types";
import { blake3Hex } from "@finisher/fin-node";
import type { FinCommand, FinFlag, FinIndex } from "../builtin/fin-tool.ts";
import type { RuntimeJournal } from "../runtime-deps.ts";
import { finRuns } from "./shell-words.ts";

/** The header `fin` names its parse in (`ARGS_HASH_HEADER`). */
export const ARGS_HASH_HEADER = "fin-args-hash";
const HASH = /^blake3:[0-9a-f]{64}$/;

/** Shell tools whose commands are matched, by harness tool name (Claude Code's `Bash`). */
export const SHELL_TOOLS: ReadonlySet<string> = new Set(["Bash"]);

/** What `fin`'s `describe` hashes: the command, the long flags given with their raw values, the positionals. */
export interface ParsedArgs {
  readonly args: readonly string[];
  readonly cmd: string;
  readonly flags: Readonly<Record<string, readonly string[]>>;
}

/** What is read of `fin schema commands`. */
export interface CommandIndex {
  readonly commands: ReadonlyArray<FinCommand>;
  readonly globals: ReadonlyArray<FinFlag>;
}

const findFlag = (flags: ReadonlyArray<FinFlag>, word: string): { flag: FinFlag; inline?: string } | undefined => {
  if (word.startsWith("--")) {
    const eq = word.indexOf("=");
    const flag = flags.find((f) => f.long === (eq < 0 ? word : word.slice(0, eq)));
    return flag && (eq < 0 ? { flag } : { flag, inline: word.slice(eq + 1) });
  }
  const flag = flags.find((f) => f.short === word);
  return flag && { flag };
};

/**
 * `argv` (the words after `fin`) as clap parses it, by the index: the command words with global
 * flags before or between them, then the command's flags by their value counts, `--` and the
 * positionals. Undefined where clap's reading cannot be known from the index (an unknown or
 * clustered flag, a word starting with `-` where a value goes, an open value count): no hash
 * is derived and the call is opaque.
 */
export function parsedArgs(argv: ReadonlyArray<string>, index: CommandIndex): ParsedArgs | undefined {
  const byName = new Map(index.commands.map((c) => [c.name, c]));
  const prefixes = new Set(index.commands.flatMap((c) => c.name.split(".").slice(0, -1).map((_, n, w) => w.slice(0, n + 1).join("."))));
  // The command words (their positions), skipping the global flags and their values.
  const wordAt = new Set<number>();
  const words: string[] = [];
  let command: FinCommand | undefined;
  let end = 0;
  for (let i = 0; i < argv.length; ) {
    const a = argv[i]!;
    if (a.startsWith("-") && a !== "-") {
      const g = a === "--" ? undefined : findFlag(index.globals, a);
      if (g === undefined) break;
      i += 1 + (g.inline !== undefined ? 0 : (g.flag.values ?? 0));
      continue;
    }
    const name = [...words, a].join(".");
    const known = byName.get(name);
    if (known === undefined && !prefixes.has(name)) break;
    words.push(a);
    wordAt.add(i);
    i++;
    if (known !== undefined) {
      command = known;
      end = i;
    }
    if (!prefixes.has(name)) break;
  }
  if (command === undefined) return undefined;
  for (const i of [...wordAt]) if (i >= end) wordAt.delete(i);
  const flags = new Map<string, string[]>();
  const positionals: string[] = [];
  for (let i = 0; i < argv.length; ) {
    if (wordAt.has(i)) {
      i++;
      continue;
    }
    const a = argv[i]!;
    if (a === "--") {
      if (i < end) return undefined;
      positionals.push(...argv.slice(i + 1));
      break;
    }
    if (!a.startsWith("-") || a === "-") {
      if (i < end) return undefined;
      positionals.push(a);
      i++;
      continue;
    }
    const hit = findFlag(i < end ? index.globals : (command.flags ?? index.globals), a);
    if (hit === undefined || hit.flag.long === undefined || hit.flag.values === null) return undefined;
    let values: string[];
    if (hit.flag.values === 0) {
      // A switch's raw value is clap's `default_missing_value` (`ArgAction::SetTrue`).
      if (hit.inline !== undefined) return undefined;
      values = ["true"];
      i++;
    } else if (hit.inline !== undefined) {
      if (hit.flag.values !== 1) return undefined;
      values = [hit.inline];
      i++;
    } else {
      values = argv.slice(i + 1, i + 1 + hit.flag.values);
      if (values.length < hit.flag.values || values.some((v) => v.startsWith("-") && v !== "-")) return undefined;
      i += 1 + hit.flag.values;
    }
    const d = hit.flag.delimiter;
    if (d !== undefined) values = values.flatMap((v) => v.split(d));
    const key = hit.flag.long.slice(2);
    flags.set(key, [...(flags.get(key) ?? []), ...values]);
  }
  // A global flag the command lacks shares its id with one of its positionals (`timeline
  // remove <timeline>`): clap propagates the positional's value up as that global, and the
  // parse reads it as both. Given as a flag as well, the reading is unknown.
  const own = new Set((command.flags ?? []).map((f) => f.long));
  for (const g of index.globals) {
    if (g.long === undefined || own.has(g.long)) continue;
    const name = g.long.slice(2);
    const k = (command.positionals ?? []).indexOf(name);
    if (k < 0) continue;
    if (flags.has(name)) return undefined;
    const last = k === (command.positionals ?? []).length - 1;
    const values = last ? positionals.slice(k) : positionals.slice(k, k + 1);
    if (values.length > 0) flags.set(name, values);
  }
  // `fin schema param` is clap's `schema` with the positional `param` (`describe`'s special case).
  const args = command.name === "schema.param" ? ["param", ...positionals] : positionals;
  const sorted: Record<string, string[]> = {};
  for (const k of [...flags.keys()].sort()) sorted[k] = flags.get(k)!;
  return { args, cmd: command.name, flags: sorted };
}

/** The hash `fin` sends for `argv` (`fin-args-hash`), or undefined when it cannot be derived. */
export function argsHashOf(argv: ReadonlyArray<string>, index: CommandIndex): string | undefined {
  const p = parsedArgs(argv, index);
  if (p === undefined) return undefined;
  // `serde_json` writes the keys sorted and compact; the keys here are made in that order.
  const doc = JSON.stringify({ args: p.args, cmd: p.cmd, flags: p.flags });
  return `blake3:${blake3Hex(Buffer.from(doc, "utf8"))}`;
}

/** The `fin` hashes a shell command shows, and whether it may run `fin` unseen. */
export function derive(command: string, index: CommandIndex | undefined): { hashes: ReadonlySet<string>; opaque: boolean } {
  const seen = finRuns(command);
  const hashes = new Set<string>();
  let opaque = seen.opaque;
  for (const argv of seen.runs) {
    const h = index === undefined ? undefined : argsHashOf(argv, index);
    if (h === undefined) opaque = true;
    else hashes.add(h);
  }
  return { hashes, opaque };
}

interface Window {
  readonly callId: string;
  readonly hashes: ReadonlySet<string>;
  readonly opaque: boolean;
  /** The txIds matched ambiguously while it was open. */
  readonly ambiguous: string[];
}

export interface ShellAttributionDeps {
  /** `fin schema commands` of the `fin` the sessions run (`finIndex`); undefined when it cannot be read. */
  readonly index: () => Promise<CommandIndex | undefined>;
  readonly shellTools?: ReadonlySet<string>;
}

/** The match of one commit: the call it carries and its attribution. */
export interface Matched {
  readonly callId: string | null;
  readonly attribution: Attribution;
}

export class ShellAttribution {
  readonly #d: ShellAttributionDeps;
  /** Open windows by session, then call. */
  readonly #open = new Map<string, Map<string, Window>>();

  constructor(deps: ShellAttributionDeps) {
    this.#d = deps;
  }

  /** The command of a shell tool's input, else undefined (not a shell call). */
  commandOf(tool: string, input: unknown): string | undefined {
    if (!(this.#d.shellTools ?? SHELL_TOOLS).has(tool)) return undefined;
    // The Bash tool's input is `{command, description?, timeout?, run_in_background?}` (verified
    // 2.1.321, Task 0); `run_in_background` is refused before any prompt under the launch's
    // `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, so only a shell's own `&`/`nohup` outlives a call.
    const c = (input as { command?: unknown } | null)?.command;
    return typeof c === "string" ? c : undefined;
  }

  /**
   * The runtime allowed call `callId` of `sessionId` (a policy allow or a person's): a shell
   * call's window opens. Resolves once it is open, so the answer goes out after it.
   */
  async allowed(sessionId: string, callId: string | undefined, tool: string, input: unknown): Promise<void> {
    // The CLI writes a call's `assistant` `tool_use` before its `can_use_tool` (whose
    // `tool_use_id` names it), so the call is announced and `callId` known here (verified
    // 2.1.321, Task 0). A prompt for a call not yet announced opens no window (its commits are
    // `none`). The window is short: allow → tool result took 7–46 ms for local tools there.
    const command = this.commandOf(tool, input);
    if (callId === undefined || command === undefined) return;
    const index = await this.#d.index().catch(() => undefined);
    const { hashes, opaque } = derive(command, index);
    let calls = this.#open.get(sessionId);
    if (calls === undefined) this.#open.set(sessionId, (calls = new Map()));
    calls.set(callId, { callId, hashes, opaque, ambiguous: [] });
  }

  /** Call `callId`'s result came: its window closes; answers the txIds matched ambiguously to it. */
  close(sessionId: string, callId: string): string[] {
    const calls = this.#open.get(sessionId);
    const w = calls?.get(callId);
    if (w === undefined) return [];
    calls!.delete(callId);
    if (calls!.size === 0) this.#open.delete(sessionId);
    return w.ambiguous;
  }

  /** Whether a call of `sessionId` is open (tests). */
  isOpen(sessionId: string, callId: string): boolean {
    return this.#open.get(sessionId)?.has(callId) === true;
  }

  /**
   * A commit of `sessionId` without a call (module docs): `argsHash` is its `fin-args-hash`
   * header (anything else counts as none). Ambiguous `txIds` are kept on every candidate.
   */
  match(sessionId: string, argsHash: string | undefined, txIds: ReadonlyArray<string>): Matched {
    const open = [...(this.#open.get(sessionId)?.values() ?? [])];
    const hash = argsHash !== undefined && HASH.test(argsHash) ? argsHash : undefined;
    // Without a hash, any call that may run `fin` is a candidate.
    const exact = open.filter((w) => (hash === undefined ? w.hashes.size > 0 : w.hashes.has(hash)));
    const candidates = open.filter((w) => w.opaque || exact.includes(w));
    if (candidates.length === 0) return { callId: null, attribution: "none" };
    if (candidates.length === 1 && hash !== undefined && exact.length === 1) return { callId: candidates[0]!.callId, attribution: "inferred" };
    for (const w of candidates) w.ambiguous.push(...txIds);
    return { callId: null, attribution: "ambiguous" };
  }

  /**
   * `inner` with each tool result closing its call's window first (a live result or a synthetic
   * one), and naming the txIds matched ambiguously to it (`agent.toolResult.txIds`).
   */
  journal(inner: RuntimeJournal): RuntimeJournal {
    return {
      session: (...a) => inner.session(...a),
      turn: (r) => inner.turn(r),
      capture: (r) => inner.capture(r),
      toolCall: (r) => inner.toolCall(r),
      toolResult: (r) => {
        const ambiguous = [...this.close(r.sessionId, r.callId), ...(r.ambiguous ?? [])];
        inner.toolResult(ambiguous.length === 0 ? r : { ...r, ambiguous });
      },
      approval: (r) => inner.approval(r),
    };
  }
}

/** The index loader over `finIndex` (the built-in tool's cache: once per `fin` binary). */
export function indexOf(load: () => Promise<FinIndex>): () => Promise<CommandIndex | undefined> {
  return () => load().then((i) => ({ commands: i.commands, globals: i.globals }), () => undefined);
}
