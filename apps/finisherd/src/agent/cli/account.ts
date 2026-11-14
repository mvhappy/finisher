// A CLI harness's account-change detection (plan 7D Decision 19, spec §8.6; lifted from
// `claude/account.ts` and keyed by harness in plan 7E Task 1). How each CLI names its account is
// its own module's (`claude/account.ts`: `auth status`/`initialize`; 7E's `codex/account.ts`:
// `account/read`); what is kept, and the rules, are the same for every CLI:
//
// - **What is kept**: the keyed hash of the account label the CLI reports and the one-word
//   sign-in method, in `<server-data>/agents/<harness>-account.json` (0600; Claude Code's is
//   `claude-account.json`, `accountFile`), one record per harness. Never the label itself: the
//   label lives only in the parsed answer in memory, and is hashed at once. The key is
//   `<server-data>/auth/account.key` (32 random bytes, made once, the approval key's sibling, one
//   for every harness), so the hash cannot be matched against a guessed e-mail by anyone without
//   the key; each harness hashes under its own domain (`finisher.<harness>.account.v1\0`).
// - **A change**: a hash that differs from the last recorded one is held as `pending`. While one
//   is pending, new sessions of that harness are refused `PROVIDER_ACCOUNT_CHANGED` (exit 5) until
//   a person confirms the account they were shown (`finisherd provider account confirm
//   <harness>`, y/N, audited, console only; another account pending by then is
//   `ACCOUNT_CONFIRM_STALE`, exit 4). Running sessions keep running: they started on the
//   earlier check. The account switching back to the recorded one clears what is pending.
// - **An expected change**: `finisherd provider signin <harness> --confirm` (refused while a
//   change is pending) lets the next different account through without a confirmation, within
//   `EXPECT_TTL_MS`; taking it is audited (hashes only) under the person who expected it.
// - **Fail closed**: a state file that cannot be read (anything but missing) is
//   `PROVIDER_ACCOUNT_CHANGED` too: a change could not be seen.
// - **Nothing names the account**: a method alone never identifies an account (`accountCheck`
//   refuses it), except where the owner chose to record one alone (`ACCOUNT_OPTIONS`: plan 7E
//   Owner question 7(a), Codex's API key only, which names no account at all; a key swapped for
//   another cannot be seen, a switch between a key and a ChatGPT plan can). Claude Code never.
//
// **Finisher opens nothing under `providers/`**: the label comes from what the CLI prints.
import { createHmac, randomBytes } from "node:crypto";
import { closeSync, fsyncSync, linkSync, openSync, readFileSync, realpathSync, rmSync, writeSync } from "node:fs";
import { dirname, join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { b64u, KEY_BYTES } from "../../auth/crypto.ts";
import { authDir, readKeyFile, writePrivateFile } from "../../auth/keys.ts";
import { ensurePrivateDir } from "../../config.ts";
import { finError } from "../../errors.ts";

/** The CLI harnesses with an account record. */
export type CliHarness = "claudeCode" | "codex";

/** How messages name each harness. */
export const HARNESS_NAMES: Readonly<Record<CliHarness, string>> = { claudeCode: "Claude Code", codex: "Codex" };

/** What the CLI says of its sign-in, as parsed. In memory only: `label` is never stored or logged. */
export interface AuthStatus {
  readonly signedIn: boolean;
  /** One word (each harness's detection allow-lists it). */
  readonly authMethod?: string;
  /** The account the CLI names; absent when it names none (an API key may not). */
  readonly label?: string;
}

/** `<server-data>/auth/account.key`. */
export const ACCOUNT_KEY_FILE = "account.key";
/** `<server-data>/agents/<file>`: Claude Code's name is kept from 7D. */
export const accountFile = (h: CliHarness): string => (h === "claudeCode" ? "claude-account.json" : "codex-account.json");
/** How long `provider signin --confirm` lets the next account through: time to run the printed sign-in. */
export const EXPECT_TTL_MS = 30 * 60_000;

/** The hash's domain per harness (Claude Code's is 7D's, unchanged: its recorded hashes stay valid). */
const hashDomain = (h: CliHarness): string => `finisher.${h}.account.v1\0`;
const confirmFix = (h: CliHarness) => `a person at the server confirms the new account with \`finisherd provider account confirm ${h}\``;

/** `PROVIDER_ACCOUNT_CHANGED` (exit 5): a change waits for a person. */
export function accountChanged(hash?: string, harness: CliHarness = "claudeCode"): FinError {
  return finError(
    "PROVIDER_ACCOUNT_CHANGED",
    `${HARNESS_NAMES[harness]} is signed in to another account than the one this server's sessions ran on`,
    { harness, ...(hash === undefined ? {} : { accountHash: hash }) },
    confirmFix(harness),
  );
}

/**
 * Whether `status` (signed in) tells the account apart: only by a label. A method alone never
 * does (two accounts would look alike); `CliAccounts.identified` adds the methods a record takes
 * alone (`ACCOUNT_OPTIONS`).
 */
export const accountIdentified = (status: AuthStatus): boolean => status.label !== undefined;

const keyed = (key: Buffer, harness: CliHarness, subject: string) => createHmac("sha256", key).update(hashDomain(harness), "utf8").update(subject, "utf8").digest("hex").slice(0, 8);

/**
 * The keyed hash of `status`'s account (8 hex characters, what `agent.providers` reports) and
 * whether it differs from `last`, the last recorded one (`null`: none yet, so no change), under
 * `harness`'s domain. Signed in without a label is not identified and has no hash (`INTERNAL`:
 * callers ask `accountIdentified` first). Signed out is `PROVIDER_AUTH`.
 */
export function accountCheck(status: AuthStatus, key: Buffer, last: string | null, harness: CliHarness = "claudeCode"): { hash: string; changed: boolean } {
  if (!status.signedIn) throw finError("PROVIDER_AUTH", `${HARNESS_NAMES[harness]} is not signed in`, { harness }, `sign it in with the commands \`finisherd provider signin ${harness}\` prints`);
  if (!accountIdentified(status)) throw finError("INTERNAL", "no account to hash: the sign-in names none", { harness });
  const hash = keyed(key, harness, `label\0${status.label}`);
  return { hash, changed: last !== null && last !== hash };
}

/**
 * The server's account key, made once in `<server-data>/auth/`: written in full to a temp file,
 * then linked into place (`EEXIST`: another caller made it first), so a reader never sees half
 * a key and two creators agree on one.
 */
export function accountKey(serverData: string): Buffer {
  const dir = authDir(serverData);
  ensurePrivateDir(dir);
  const path = join(dir, ACCOUNT_KEY_FILE);
  try {
    return readKeyFile(path);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw e;
  }
  const tmp = `${path}.${process.pid}.${randomBytes(6).toString("hex")}.tmp`;
  const fd = openSync(tmp, "wx", 0o600);
  try {
    writeSync(fd, `${b64u.encode(randomBytes(KEY_BYTES))}\n`);
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
  try {
    linkSync(tmp, path);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "EEXIST") throw e;
  } finally {
    rmSync(tmp, { force: true });
  }
  if (process.platform !== "win32") {
    const d = openSync(dir, "r");
    try {
      fsyncSync(d);
    } finally {
      closeSync(d);
    }
  }
  return readKeyFile(path);
}

interface Seen {
  readonly hash: string;
  readonly authMethod?: string;
  readonly at: string;
}

/** Who set an expectation (an audit principal). */
export interface By {
  readonly kind: string;
  readonly id: string;
}

/** What `<harness>-account.json` holds: hashes, one-word methods, times and a principal only. */
export interface AccountState {
  readonly v: 1;
  /** The account sessions run on; absent until the CLI is first seen signed in. */
  readonly current?: Seen;
  /** A different account seen since, waiting for a person's confirmation. */
  readonly pending?: Seen;
  /** `provider signin <harness> --confirm`: the next different account seen before then is taken. */
  readonly expectUntil?: string;
  readonly expectBy?: By;
}

export interface Observation {
  readonly hash: string;
  /** A change waits for a confirmation: new sessions are refused. */
  readonly changed: boolean;
}

/** What is audited when an expected change is taken: hashes only, and whose record it was. */
export type AccountAudit = (by: By, details: { accountHash: string; previousHash?: string }, harness: CliHarness) => void;

// The audit of expected changes, per server data (links resolved). `AgentAdmin` registers it;
// TODO(7D Task 7): pass it to detection and the session explicitly from `wiring.ts`.
const audits = new Map<string, AccountAudit>();
const keyOf = (serverData: string) => {
  try {
    return realpathSync(serverData);
  } catch {
    return serverData;
  }
};

/** Where an expected account change taken for `serverData` (any harness) is audited. */
export function registerAccountAudit(serverData: string, audit: AccountAudit): void {
  audits.set(keyOf(serverData), audit);
}

const unreadable = (harness: CliHarness, path: string, cause: string) =>
  finError(
    "PROVIDER_ACCOUNT_CHANGED",
    `${HARNESS_NAMES[harness]}'s recorded account cannot be read, so Finisher cannot tell whether the account changed`,
    { harness, reason: "stateUnreadable", cause },
    `repair ${path}, or remove it: the account ${HARNESS_NAMES[harness]} is signed in to next becomes the account`,
  );

/** `ACCOUNT_CONFIRM_STALE` (exit 4): another account waits than the one the person was shown. */
export const confirmStale = (shown: string, waiting: string | undefined, harness: CliHarness = "claudeCode") =>
  finError(
    "ACCOUNT_CONFIRM_STALE",
    waiting === undefined ? "no account change waits any more" : `another ${HARNESS_NAMES[harness]} account waits than the one you were shown`,
    { harness, shown, ...(waiting === undefined ? {} : { accountHash: waiting }) },
    `run \`finisherd provider account confirm ${harness}\` again and check the account it shows`,
  );

const isSeen = (v: unknown): v is Seen =>
  v !== null && typeof v === "object" && typeof (v as Seen).hash === "string" && /^[0-9a-f]{8}$/.test((v as Seen).hash) && typeof (v as Seen).at === "string";

export interface CliAccountsOptions {
  readonly harness: CliHarness;
  /**
   * Sign-in methods recorded by the method alone when the CLI names no account (Codex only,
   * Owner question 7(a): `["apiKey"]`); refused for Claude Code, whose unnamed sign-in throws.
   */
  readonly methodOnly?: readonly string[];
  readonly now?: () => Date;
}

/**
 * What each harness's record is built with. **Owner question 7** lives here alone: (a) records
 * Codex's API key by its method; (b) would be `{harness: "codex"}` (an API key then throws, and
 * Codex's detection refuses it `PROVIDER_AUTH {reason: noAccount}`).
 */
export const ACCOUNT_OPTIONS: Readonly<Record<CliHarness, CliAccountsOptions>> = {
  claudeCode: { harness: "claudeCode" },
  codex: { harness: "codex", methodOnly: ["apiKey"] },
};

/** The account state of `harness` in `<server-data>/agents/<accountFile>`. Only the server writes it. */
export class CliAccounts {
  readonly #serverData: string;
  readonly #path: string;
  readonly #now: () => Date;
  readonly #harness: CliHarness;
  readonly #methodOnly: readonly string[];

  constructor(serverData: string, o: CliAccountsOptions) {
    if (o.harness !== "codex" && (o.methodOnly?.length ?? 0) > 0) throw finError("INTERNAL", "only Codex records a sign-in method alone", { harness: o.harness });
    this.#serverData = serverData;
    this.#harness = o.harness;
    this.#path = join(serverData, "agents", accountFile(o.harness));
    this.#now = o.now ?? (() => new Date());
    this.#methodOnly = o.methodOnly ?? [];
  }

  get harness(): CliHarness {
    return this.#harness;
  }

  /** Whether `status` (signed in) can be recorded: a label, or a method this record takes alone. */
  identified(status: AuthStatus): boolean {
    return accountIdentified(status) || (status.authMethod !== undefined && this.#methodOnly.includes(status.authMethod));
  }

  /** The keyed hash `observe` records for `status` (a label's, else a method's this record takes alone). */
  hashOf(status: AuthStatus, key: Buffer): string {
    if (status.signedIn && !accountIdentified(status) && status.authMethod !== undefined && this.#methodOnly.includes(status.authMethod)) {
      return keyed(key, this.#harness, `method\0${status.authMethod}`);
    }
    return accountCheck(status, key, null, this.#harness).hash;
  }

  /**
   * The state; a missing file is the empty state (the next sign-in is the account). Anything else
   * that cannot be read or does not parse throws `PROVIDER_ACCOUNT_CHANGED {reason:
   * stateUnreadable}`: forgetting a pending change would let it through.
   */
  state(): AccountState {
    let text: string;
    try {
      text = readFileSync(this.#path, "utf8");
    } catch (e) {
      const code = (e as { code?: unknown }).code;
      if (code === "ENOENT") return { v: 1 };
      throw unreadable(this.#harness, this.#path, typeof code === "string" ? code : "read");
    }
    let v: unknown;
    try {
      v = JSON.parse(text);
    } catch {
      throw unreadable(this.#harness, this.#path, "json");
    }
    const s = v as AccountState;
    if (s === null || typeof s !== "object" || s.v !== 1 || (s.current !== undefined && !isSeen(s.current)) || (s.pending !== undefined && !isSeen(s.pending))) {
      throw unreadable(this.#harness, this.#path, "shape");
    }
    return s;
  }

  #write(s: AccountState): void {
    ensurePrivateDir(dirname(this.#path));
    writePrivateFile(this.#path, `${JSON.stringify(s)}\n`);
  }

  #seen(hash: string, authMethod: string | undefined): Seen {
    return { hash, ...(authMethod === undefined ? {} : { authMethod }), at: this.#now().toISOString() };
  }

  #expecting(s: AccountState): boolean {
    return s.expectUntil !== undefined && Date.parse(s.expectUntil) > this.#now().getTime();
  }

  /** Records what the CLI reports (signed in); a change is pending unless one was expected. */
  observe(status: AuthStatus, key: Buffer): Observation {
    return this.record(this.hashOf(status, key), status.authMethod);
  }

  /** `observe` of an account already hashed (detection keeps the hash, never the label). */
  record(hash: string, authMethod: string | undefined): Observation {
    const s = this.state();
    if (s.current === undefined) {
      this.#write({ v: 1, current: this.#seen(hash, authMethod) });
      return { hash, changed: false };
    }
    const expect = this.#expecting(s) ? { expectUntil: s.expectUntil!, ...(s.expectBy === undefined ? {} : { expectBy: s.expectBy }) } : {};
    if (s.current.hash === hash) {
      // Back on the recorded account: nothing waits any more (an expectation still runs its time).
      if (s.pending !== undefined || s.current.authMethod !== authMethod || (s.expectUntil !== undefined && !this.#expecting(s))) {
        this.#write({ v: 1, current: this.#seen(hash, authMethod), ...expect });
      }
      return { hash, changed: false };
    }
    if (this.#expecting(s)) {
      this.#write({ v: 1, current: this.#seen(hash, authMethod) });
      audits.get(keyOf(this.#serverData))?.(s.expectBy ?? { kind: "local", id: "server" }, { accountHash: hash, previousHash: s.current.hash }, this.#harness);
      return { hash, changed: false };
    }
    if (s.pending?.hash !== hash || s.expectUntil !== undefined) this.#write({ v: 1, current: s.current, pending: this.#seen(hash, authMethod) });
    return { hash, changed: true };
  }

  /** Whether a change waits for a confirmation, and its hash. */
  pending(): string | undefined {
    return this.state().pending?.hash;
  }

  /**
   * A person takes the pending account they were shown (`shown`) as the one sessions run on.
   * Nothing pending: `{confirmed: false}`; another account pending: `ACCOUNT_CONFIRM_STALE`.
   */
  confirm(shown: string): { confirmed: boolean; accountHash?: string; previous?: string } {
    const s = this.state();
    if (s.pending === undefined) return { confirmed: false, ...(s.current === undefined ? {} : { accountHash: s.current.hash }) };
    if (s.pending.hash !== shown) throw confirmStale(shown, s.pending.hash, this.#harness);
    this.#write({ v: 1, current: s.pending });
    return { confirmed: true, accountHash: s.pending.hash, ...(s.current === undefined ? {} : { previous: s.current.hash }) };
  }

  /**
   * `provider signin <harness> --confirm`: the next different account seen within
   * `EXPECT_TTL_MS` is taken without a confirmation (audited under `by`). Refused
   * `PROVIDER_ACCOUNT_CHANGED` while a change waits: that one is confirmed or undone first.
   */
  expectNext(by: By): { expectUntil: string } {
    const s = this.state();
    if (s.pending !== undefined) {
      throw finError(
        "PROVIDER_ACCOUNT_CHANGED",
        `a ${HARNESS_NAMES[this.#harness]} account change already waits for a confirmation`,
        { harness: this.#harness, reason: "changeWaiting", accountHash: s.pending.hash },
        `confirm it first with \`finisherd provider account confirm ${this.#harness}\`, or sign the CLI back in to the recorded account`,
      );
    }
    const expectUntil = new Date(this.#now().getTime() + EXPECT_TTL_MS).toISOString();
    this.#write({ v: 1, ...(s.current === undefined ? {} : { current: s.current }), expectUntil, expectBy: { kind: by.kind, id: by.id } });
    return { expectUntil };
  }
}

/** `harness`'s record as the server keeps it (`ACCOUNT_OPTIONS`). */
export const accountsOf = (serverData: string, harness: CliHarness, now?: () => Date): CliAccounts => new CliAccounts(serverData, { ...ACCOUNT_OPTIONS[harness], ...(now === undefined ? {} : { now }) });

/** The account problem to show while one stands: a pending change, or a state that cannot be read. */
export function accountProblem(serverData: string, harness: CliHarness = "claudeCode"): FinError | undefined {
  try {
    const waiting = accountsOf(serverData, harness).pending();
    return waiting === undefined ? undefined : accountChanged(waiting, harness);
  } catch (e) {
    return e as FinError;
  }
}

/**
 * The session layer's check at each instance's start (the adapter calls it with what the
 * instance names, else detection's): records the account and answers its hash (for the model
 * cache) and, for a **new** session while a change waits (or the record cannot be read),
 * `PROVIDER_ACCOUNT_CHANGED`. A running session's instance (a respawn, a resume) is never
 * refused: it started on the earlier check. Signed out: no hash, no refusal (the CLI's own error
 * is `PROVIDER_AUTH`). Signed in naming no account the record can take (`identified`): no hash,
 * nothing recorded; a new session is refused only by a change that already waits (or a record
 * that cannot be read). `accounts` defaults to Claude Code's record.
 */
export function instanceAccount(serverData: string, status: AuthStatus, o: { readonly newSession: boolean; readonly accounts?: CliAccounts }): { hash?: string; refused: FinError | null } {
  if (!status.signedIn) return { refused: null };
  const accounts = o.accounts ?? accountsOf(serverData, "claudeCode");
  if (!accounts.identified(status)) {
    if (!o.newSession) return { refused: null };
    try {
      const waiting = accounts.pending();
      return { refused: waiting === undefined ? null : accountChanged(waiting, accounts.harness) };
    } catch (e) {
      return { refused: e as FinError };
    }
  }
  try {
    const seen = accounts.observe(status, accountKey(serverData));
    return { hash: seen.hash, refused: seen.changed && o.newSession ? accountChanged(seen.hash, accounts.harness) : null };
  } catch (e) {
    return { refused: o.newSession ? (e as FinError) : null };
  }
}

/**
 * `instanceAccount` for an account detection already hashed, when the instance names none: the
 * same record and the same refusal rule.
 */
export function instanceAccountHash(serverData: string, hash: string, authMethod: string | undefined, o: { readonly newSession: boolean; readonly accounts?: CliAccounts }): { hash?: string; refused: FinError | null } {
  const accounts = o.accounts ?? accountsOf(serverData, "claudeCode");
  try {
    const seen = accounts.record(hash, authMethod);
    return { hash: seen.hash, refused: seen.changed && o.newSession ? accountChanged(seen.hash, accounts.harness) : null };
  } catch (e) {
    return { refused: o.newSession ? (e as FinError) : null };
  }
}
