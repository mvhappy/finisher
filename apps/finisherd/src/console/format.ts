// The console's arguments and output (spec §4.7): English text for a person, `--json` for the
// read-only commands; errors print `finisherd: <CODE>: <message>` and the fix to stderr and exit
// as `fin` does.
import type { AuditEntry } from "@finisher/contracts";
import { consoleError, type Failure } from "./client.ts";
import { exitCodeOf } from "./exit-codes.ts";
import { safe } from "./safe.ts";

/** Where the console writes: stdout for answers, stderr for errors. */
export interface Output {
  out(text: string): void;
  err(text: string): void;
}

export const processOutput: Output = {
  out: (text) => void process.stdout.write(`${text}\n`),
  err: (text) => void process.stderr.write(`${text}\n`),
};

/** Prints `e` and answers its exit code. */
export function printFailure(o: Output, e: Failure): number {
  const details = e.details !== undefined && e.details !== null && Object.keys(e.details).length > 0 ? ` ${safe(JSON.stringify(e.details))}` : "";
  // Messages can carry what a registry wrote (an advisory, a plugin name): plain text only.
  o.err(`finisherd: ${safe(e.code)}: ${safe(e.message)}${details}`);
  if (e.fix !== undefined) o.err(`finisherd: fix: ${safe(e.fix)}`);
  return exitCodeOf(e.code);
}

/** Parsed arguments: positional words, flags (`--name` or `--name value`) and repeated flags. */
export interface Args {
  readonly words: ReadonlyArray<string>;
  readonly flags: ReadonlyMap<string, ReadonlyArray<string>>;
}

/** Flags that take a value; every other flag is a switch. */
const VALUED = new Set([
  "server-data", "since", "kind", "limit", "before", "project", "label", "scope", "branch", "idle", "absolute", "out",
  "dir", "root", "root-sha256", "registry", "store",
  // Agent sessions (7A Task 12).
  "model", "role", "mode", "base", "title", "prompt", "prompt-file", "budget-usd", "budget-tokens", "provider", "harness", "status", "add",
  // Command rules (7B Task 2).
  "session", "preset", "bytes-over", "minutes-over",
  // Remembered answers (7B Task 5).
  "remember",
  "name", "storage", "include", "exclude", "kinds", "poll", "full", "bandwidth", "alias", "accept-unmounted",
  // 5B: pairing and remote access.
  "ttl", "listen", "tls", "cert", "key", "public-url",
]);
/** Flags whose value is optional: taken only when the next word is one of these (`--hydrate` or `--hydrate off`). */
const OPTIONAL = new Map<string, ReadonlyArray<string>>([["hydrate", ["on", "off"]]]);

export function usage(message: string, fix = "Run `finisherd help` to list the commands."): never {
  throw consoleError("USAGE", message, {}, fix);
}

export function parseArgs(argv: ReadonlyArray<string>): Args {
  const words: string[] = [];
  const flags = new Map<string, string[]>();
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]!;
    if (!a.startsWith("--") || a === "--") {
      words.push(a);
      continue;
    }
    const eq = a.indexOf("=");
    const name = eq < 0 ? a.slice(2) : a.slice(2, eq);
    let value = "";
    if (eq >= 0) value = a.slice(eq + 1);
    else if (OPTIONAL.get(name)?.includes(argv[i + 1] ?? "") === true) value = argv[++i]!;
    else if (VALUED.has(name)) {
      const next = argv[++i];
      if (next === undefined) usage(`--${name} needs a value`);
      value = next;
    }
    flags.set(name, [...(flags.get(name) ?? []), value]);
  }
  return { words, flags };
}

export const has = (a: Args, name: string): boolean => a.flags.has(name);
export const one = (a: Args, name: string): string | undefined => a.flags.get(name)?.at(-1);
export const all = (a: Args, name: string): ReadonlyArray<string> => a.flags.get(name) ?? [];

/** Refuses flags `command` does not take. */
export function onlyFlags(a: Args, command: string, allowed: ReadonlyArray<string>): void {
  for (const name of a.flags.keys()) {
    if (name !== "server-data" && !allowed.includes(name)) usage(`${command} does not take --${name}`);
  }
}

/** A date or date-time (`2026-11-08`, `2026-11-08T10:00:00Z`) or ms since the epoch, as ms. */
export function parseDate(flag: string, value: string): number {
  const ms = /^\d+$/.test(value) ? Number(value) : /^\d{4}-\d{2}-\d{2}/.test(value) ? Date.parse(value) : Number.NaN;
  if (!Number.isFinite(ms)) usage(`--${flag} needs a date such as 2026-11-08 or 2026-11-08T10:00:00Z`);
  return ms;
}

/** A positive whole number. */
export function parseCount(flag: string, value: string): number {
  const n = Number(value);
  if (!Number.isSafeInteger(n) || n < 1) usage(`--${flag} needs a whole number above 0`);
  return n;
}

/** One audit entry as a line for a person. */
export function auditLine(e: AuditEntry): string {
  const who = e.principalId.length > 0 ? `${e.principalKind}:${e.principalId}` : e.principalKind;
  const parts = [new Date(e.at).toISOString(), e.kind, who, e.outcome];
  if (e.code !== undefined) parts.push(e.code);
  if (e.count > 1) parts.push(`×${e.count}`);
  if (Object.keys(e.details).length > 0) parts.push(JSON.stringify(e.details));
  return parts.join("  ");
}

/** A pin (`base64url(SHA-256(SPKI))`, 43 characters) in groups of four, for reading aloud. */
export const pinGroups = (spki: string): string => (spki.match(/.{1,4}/g) ?? []).join(" ");

/**
 * The QR code text (`qrText`, Unicode half blocks) when it fits `columns`; `undefined` when it
 * is wider (or the width is unknown and it is wider than 80): the caller shows the link only.
 */
export function qrThatFits(qr: string, columns: number | undefined): string | undefined {
  const width = Math.max(...qr.split("\n").map((l) => [...l].length));
  return width <= (columns ?? 80) ? qr : undefined;
}
