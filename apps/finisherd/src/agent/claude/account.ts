// Claude Code's account-change detection (plan 7D Decision 19, spec §8.6).
//
// - **What is kept**: the keyed hash of the account label the CLI reports (each instance's
//   `initialize.account.email`, or `auth status --json` when it names one) and the one-word sign-in
//   method, in `<server-data>/agents/claude-account.json` (0600). Never the label itself: the
//   label lives only in the parsed answer in memory, and is hashed at once. The key is
//   `<server-data>/auth/account.key` (32 random bytes, made once, the approval key's sibling),
//   so the hash cannot be matched against a guessed e-mail by anyone without the key.
// - **A change**: a hash that differs from the last recorded one is held as `pending`. While one
//   is pending, new Claude Code sessions are refused `PROVIDER_ACCOUNT_CHANGED` (exit 5) until a
//   person confirms the account they were shown (`finisherd provider account confirm
//   claudeCode`, y/N, audited, console only; another account pending by then is
//   `ACCOUNT_CONFIRM_STALE`, exit 4). Running sessions keep running: they started on the
//   earlier check. The account switching back to the recorded one clears what is pending.
// - **An expected change**: `finisherd provider signin claudeCode --confirm` (refused while a
//   change is pending) lets the next different account through without a confirmation, within
//   `EXPECT_TTL_MS`; taking it is audited (hashes only) under the person who expected it.
// - **Fail closed**: a state file that cannot be read (anything but missing) is
//   `PROVIDER_ACCOUNT_CHANGED` too: a change could not be seen.
// - **Where the account is named** (verified on 2.1.321, 7D Task 0): `auth status` names it only
//   from a config dir holding the sign-in's profile (`.claude.json` `oauthAccount`): a fresh one
//   reports `email`/`orgId`/`orgName` as `null`. So detection (`detect.ts`, every
//   `agent.providers`, every plan and every instance's start) runs it with the **sign-in home** as
//   its config dir (Decision 19 as amended; the person's `auth login` writes the profile there),
//   in the detect host sandbox (off Windows: TODO(7D Task 13)), and hashes the `email`, else the
//   `orgId`; signed in naming neither is `PROVIDER_AUTH` there. An instance's
//   `initialize.account.email` (present once the CLI holds the profile in the session's own config
//   dir: a resumed process, or the profile fetch finishing first) is recorded too, under the same
//   subject as detection's e-mail, so both give one hash.
// - **Nothing names the account** (Windows, where detection keeps a throwaway config dir; or an
//   API-key sign-in): no refusal by itself: nothing is recorded, and a new session is refused
//   only while a change already waits or the record cannot be read (`instanceAccount`).
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

/** What the CLI says of its sign-in, as parsed. In memory only: `label` is never stored or logged. */
export interface AuthStatus {
  readonly signedIn: boolean;
  /** One word (`detect.ts` allow-lists it). */
  readonly authMethod?: string;
  /** The account the CLI names; absent when it names none (an API key may not). */
  readonly label?: string;
}

/** `<server-data>/auth/account.key`. */
export const ACCOUNT_KEY_FILE = "account.key";
/** `<server-data>/agents/claude-account.json`. */
export const ACCOUNT_FILE = "claude-account.json";
/** How long `provider signin --confirm` lets the next account through: time to run the printed sign-in. */
export const EXPECT_TTL_MS = 30 * 60_000;

/**
 * The fields of `auth status --json` (and `initialize.account`) that name the signed-in account;
 * their values are joined into the hashed label. Verified on 2.1.321 (7D Task 0): `email` is the
 * one both carry (`auth status` also has `orgId`/`orgName`, `initialize.account` never). Never
 * `subscriptionType`: its text varies for one account ("Claude Max", "Claude Ultra").
 */
export const LABEL_FIELDS = ["email"] as const;

/**
 * Sign-in methods that may name no account (an API key): hashed by the method alone. Any other
 * sign-in without a label is not identified (`accountIdentified`): two accounts would look alike.
 */
// TODO(7D Task 0 Step 4): the method names the CLI prints for an API key (`apiKey` is assumed).
export const API_KEY_METHODS: ReadonlySet<string> = new Set(["apiKey"]);

/** The label `answer` names (`auth status --json`'s object or `initialize.account`), else undefined. */
export function labelOf(answer: unknown): string | undefined {
  if (answer === null || typeof answer !== "object" || Array.isArray(answer)) return undefined;
  const o = answer as Record<string, unknown>;
  const parts = LABEL_FIELDS.map((f) => o[f]).filter((v): v is string => typeof v === "string" && v !== "");
  return parts.length === 0 ? undefined : parts.join("\0");
}

/** What an organization-only label starts with (an e-mail never holds a NUL). */
const ORG_LABEL = "orgId\0";

/**
 * The account `auth status --json`'s raw output names (not JSON: none): its `email` (the same
 * label, so the same hash, as an instance's `initialize.account.email`), else its `orgId` (a label
 * of its own kind, never equal to an e-mail's). Verified on 2.1.321 (7D Task 0, 04-auth-status D):
 * both come from the config dir's profile (`oauthAccount`), the sign-in home's for detection.
 */
export function labelOfOutput(stdout: string): string | undefined {
  let v: unknown;
  try {
    v = JSON.parse(stdout);
  } catch {
    return undefined;
  }
  const email = labelOf(v);
  if (email !== undefined) return email;
  const org = v !== null && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>).orgId : undefined;
  return typeof org === "string" && org !== "" ? `${ORG_LABEL}${org}` : undefined;
}


const HASH_LABEL = "finisher.claudeCode.account.v1\0";
const HARNESS = "claudeCode";
const CONFIRM_FIX = "a person at the server confirms the new account with `finisherd provider account confirm claudeCode`";

/** `PROVIDER_ACCOUNT_CHANGED` (exit 5): a change waits for a person. */
export function accountChanged(hash?: string): FinError {
  return finError(
    "PROVIDER_ACCOUNT_CHANGED",
    "Claude Code is signed in to another account than the one this server's sessions ran on",
    { harness: HARNESS, ...(hash === undefined ? {} : { accountHash: hash }) },
    CONFIRM_FIX,
  );
}

/** Whether `status` (signed in) tells the account apart: a label, or an API-key method. */
export const accountIdentified = (status: AuthStatus): boolean => status.label !== undefined || API_KEY_METHODS.has(status.authMethod ?? "");

/**
 * The keyed hash of `status`'s account (8 hex characters, what `agent.providers` reports) and
 * whether it differs from `last`, the last recorded one (`null`: none yet, so no change).
 * Signed in without a label: an API-key method hashes the method alone; any other method is not
 * identified and has no hash (`INTERNAL`: callers ask `accountIdentified` first). Signed out is
 * `PROVIDER_AUTH`.
 */
export function accountCheck(status: AuthStatus, key: Buffer, last: string | null): { hash: string; changed: boolean } {
  if (!status.signedIn) throw finError("PROVIDER_AUTH", "Claude Code is not signed in", { harness: HARNESS }, "sign it in with the commands `finisherd provider signin claudeCode` prints");
  if (!accountIdentified(status)) throw finError("INTERNAL", "no account to hash: the sign-in names none", { harness: HARNESS });
  const subject = status.label === undefined ? `method\0${status.authMethod}` : `label\0${status.label}`;
  const hash = createHmac("sha256", key).update(HASH_LABEL, "utf8").update(subject, "utf8").digest("hex").slice(0, 8);
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

/** What `claude-account.json` holds: hashes, one-word methods, times and a principal only. */
export interface AccountState {
  readonly v: 1;
  /** The account sessions run on; absent until the CLI is first seen signed in. */
  readonly current?: Seen;
  /** A different account seen since, waiting for a person's confirmation. */
  readonly pending?: Seen;
  /** `provider signin claudeCode --confirm`: the next different account seen before then is taken. */
  readonly expectUntil?: string;
  readonly expectBy?: By;
}

export interface Observation {
  readonly hash: string;
  /** A change waits for a confirmation: new sessions are refused. */
  readonly changed: boolean;
}

/** What is audited when an expected change is taken: hashes only. */
export type AccountAudit = (by: By, details: { accountHash: string; previousHash?: string }) => void;

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

/** Where an expected account change taken for `serverData` is audited. */
export function registerAccountAudit(serverData: string, audit: AccountAudit): void {
  audits.set(keyOf(serverData), audit);
}

const unreadable = (path: string, cause: string) =>
  finError(
    "PROVIDER_ACCOUNT_CHANGED",
    "Claude Code's recorded account cannot be read, so Finisher cannot tell whether the account changed",
    { harness: HARNESS, reason: "stateUnreadable", cause },
    `repair ${path}, or remove it: the account Claude Code is signed in to next becomes the account`,
  );

/** `ACCOUNT_CONFIRM_STALE` (exit 4): another account waits than the one the person was shown. */
export const confirmStale = (shown: string, waiting: string | undefined) =>
  finError(
    "ACCOUNT_CONFIRM_STALE",
    waiting === undefined ? "no account change waits any more" : "another Claude Code account waits than the one you were shown",
    { harness: HARNESS, shown, ...(waiting === undefined ? {} : { accountHash: waiting }) },
    "run `finisherd provider account confirm claudeCode` again and check the account it shows",
  );

const isSeen = (v: unknown): v is Seen =>
  v !== null && typeof v === "object" && typeof (v as Seen).hash === "string" && /^[0-9a-f]{8}$/.test((v as Seen).hash) && typeof (v as Seen).at === "string";

/** The account state in `<server-data>/agents/claude-account.json`. Only the server writes it. */
export class ClaudeAccounts {
  readonly #serverData: string;
  readonly #path: string;
  readonly #now: () => Date;

  constructor(serverData: string, now: () => Date = () => new Date()) {
    this.#serverData = serverData;
    this.#path = join(serverData, "agents", ACCOUNT_FILE);
    this.#now = now;
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
      throw unreadable(this.#path, typeof code === "string" ? code : "read");
    }
    let v: unknown;
    try {
      v = JSON.parse(text);
    } catch {
      throw unreadable(this.#path, "json");
    }
    const s = v as AccountState;
    if (s === null || typeof s !== "object" || s.v !== 1 || (s.current !== undefined && !isSeen(s.current)) || (s.pending !== undefined && !isSeen(s.pending))) {
      throw unreadable(this.#path, "shape");
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
    return this.record(accountCheck(status, key, null).hash, status.authMethod);
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
      audits.get(keyOf(this.#serverData))?.(s.expectBy ?? { kind: "local", id: "server" }, { accountHash: hash, previousHash: s.current.hash });
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
    if (s.pending.hash !== shown) throw confirmStale(shown, s.pending.hash);
    this.#write({ v: 1, current: s.pending });
    return { confirmed: true, accountHash: s.pending.hash, ...(s.current === undefined ? {} : { previous: s.current.hash }) };
  }

  /**
   * `provider signin claudeCode --confirm`: the next different account seen within
   * `EXPECT_TTL_MS` is taken without a confirmation (audited under `by`). Refused
   * `PROVIDER_ACCOUNT_CHANGED` while a change waits: that one is confirmed or undone first.
   */
  expectNext(by: By): { expectUntil: string } {
    const s = this.state();
    if (s.pending !== undefined) {
      throw finError(
        "PROVIDER_ACCOUNT_CHANGED",
        "a Claude Code account change already waits for a confirmation",
        { harness: HARNESS, reason: "changeWaiting", accountHash: s.pending.hash },
        "confirm it first with `finisherd provider account confirm claudeCode`, or sign the CLI back in to the recorded account",
      );
    }
    const expectUntil = new Date(this.#now().getTime() + EXPECT_TTL_MS).toISOString();
    this.#write({ v: 1, ...(s.current === undefined ? {} : { current: s.current }), expectUntil, expectBy: { kind: by.kind, id: by.id } });
    return { expectUntil };
  }
}

/** The account problem to show while one stands: a pending change, or a state that cannot be read. */
export function accountProblem(serverData: string): FinError | undefined {
  try {
    const waiting = new ClaudeAccounts(serverData).pending();
    return waiting === undefined ? undefined : accountChanged(waiting);
  } catch (e) {
    return e as FinError;
  }
}

/**
 * The session layer's check at each instance's start (the adapter calls it with what the
 * instance's `initialize.account` names, else detection's `auth status`): records the account
 * and answers its hash (for `ClaudeModelCache.recordInstanceModels`) and, for a **new** session
 * while a change waits (or the record cannot be read), `PROVIDER_ACCOUNT_CHANGED`. A running
 * session's instance (a respawn, a resume) is never refused: it started on the earlier check.
 * Signed out: no hash, no refusal (the CLI's own error is `PROVIDER_AUTH`). Signed in naming no
 * account (`accountIdentified`): no hash, nothing recorded; a new session is refused only by a
 * change that already waits (or a record that cannot be read).
 */
export function instanceAccount(serverData: string, status: AuthStatus, o: { readonly newSession: boolean; readonly accounts?: ClaudeAccounts }): { hash?: string; refused: FinError | null } {
  if (!status.signedIn) return { refused: null };
  if (!accountIdentified(status)) {
    if (!o.newSession) return { refused: null };
    try {
      const waiting = (o.accounts ?? new ClaudeAccounts(serverData)).pending();
      return { refused: waiting === undefined ? null : accountChanged(waiting) };
    } catch (e) {
      return { refused: e as FinError };
    }
  }
  try {
    const seen = (o.accounts ?? new ClaudeAccounts(serverData)).observe(status, accountKey(serverData));
    return { hash: seen.hash, refused: seen.changed && o.newSession ? accountChanged(seen.hash) : null };
  } catch (e) {
    return { refused: o.newSession ? (e as FinError) : null };
  }
}

/**
 * `instanceAccount` for an account detection already hashed (its `auth status` named it), when
 * the instance's `initialize.account` names none: the same record and the same refusal rule.
 */
export function instanceAccountHash(serverData: string, hash: string, authMethod: string | undefined, o: { readonly newSession: boolean; readonly accounts?: ClaudeAccounts }): { hash?: string; refused: FinError | null } {
  try {
    const seen = (o.accounts ?? new ClaudeAccounts(serverData)).record(hash, authMethod);
    return { hash: seen.hash, refused: seen.changed && o.newSession ? accountChanged(seen.hash) : null };
  } catch (e) {
    return { refused: o.newSession ? (e as FinError) : null };
  }
}
