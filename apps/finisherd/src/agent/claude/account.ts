// Claude Code's account-change detection (plan 7D Decision 19, spec §8.6).
//
// - **What is kept**: the keyed hash of the account label the CLI reports (`auth status --json`;
//   Task 7's instances may also read it from `initialize.account`) and the one-word sign-in
//   method, in `<server-data>/agents/claude-account.json` (0600). Never the label itself: the
//   label lives only in the parsed answer in memory, and is hashed at once. The key is
//   `<server-data>/auth/account.key` (32 random bytes, made once, the approval key's sibling),
//   so the hash cannot be matched against a guessed e-mail by anyone without the key.
// - **A change**: a hash that differs from the last recorded one, with no
//   `finisherd provider signin claudeCode --confirm` since, is held as `pending`. While one is
//   pending, new Claude Code sessions are refused `PROVIDER_ACCOUNT_CHANGED` (exit 5) until a
//   person confirms with `finisherd provider account confirm claudeCode` (y/N, audited, console
//   only). Running sessions keep running: they started on the earlier check. The account
//   switching back to the recorded one clears what is pending.
// - **Where it is observed**: detection (`detect.ts`, every `agent.providers` and every plan) and,
//   once Task 7's session runs `auth status` at each instance's start, `instanceAccount` there.
//
// **Finisher opens nothing under `providers/`**: the label comes from what the CLI prints.
import { createHmac, randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { FinError } from "@finisher/contracts";
import { KEY_BYTES } from "../../auth/crypto.ts";
import { authDir, readKeyFile, writeKeyFile } from "../../auth/keys.ts";
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

/**
 * The fields of `auth status --json` (and `initialize.account`) that name the signed-in account,
 * in order of preference; their values are joined into the hashed label.
 */
// TODO(7D Task 0 Step 4): which fields carry the account label when signed in (Task 0's
// unsigned half saw none); modelled by fake-claude as `email` (its `account` sidecar knob).
export const LABEL_FIELDS = ["email"] as const;

/** The label `answer` names (`auth status --json`'s object or `initialize.account`), else undefined. */
export function labelOf(answer: unknown): string | undefined {
  if (answer === null || typeof answer !== "object" || Array.isArray(answer)) return undefined;
  const o = answer as Record<string, unknown>;
  const parts = LABEL_FIELDS.map((f) => o[f]).filter((v): v is string => typeof v === "string" && v !== "");
  return parts.length === 0 ? undefined : parts.join("\0");
}

/** `labelOf` of `auth status`'s raw output (not JSON: none). */
export function labelOfOutput(stdout: string): string | undefined {
  try {
    return labelOf(JSON.parse(stdout));
  } catch {
    return undefined;
  }
}

const HASH_LABEL = "finisher.claudeCode.account.v1\0";

/**
 * The keyed hash of `status`'s account (8 hex characters, what `agent.providers` reports) and
 * whether it differs from `last`, the last recorded one (`null`: none yet, so no change).
 * Signed in without a label (an API key that names no account) hashes the sign-in method alone.
 * Throws `PROVIDER_AUTH` when signed out: there is no account to hash.
 */
export function accountCheck(status: AuthStatus, key: Buffer, last: string | null): { hash: string; changed: boolean } {
  if (!status.signedIn) throw finError("PROVIDER_AUTH", "Claude Code is not signed in", { harness: "claudeCode" }, "sign it in with the commands `finisherd provider signin claudeCode` prints");
  const subject = status.label === undefined ? `method\0${status.authMethod ?? "other"}` : `label\0${status.label}`;
  const hash = createHmac("sha256", key).update(HASH_LABEL, "utf8").update(subject, "utf8").digest("hex").slice(0, 8);
  return { hash, changed: last !== null && last !== hash };
}

/** The server's account key, made once in `<server-data>/auth/`. */
export function accountKey(serverData: string): Buffer {
  const dir = authDir(serverData);
  ensurePrivateDir(dir);
  const path = join(dir, ACCOUNT_KEY_FILE);
  try {
    return readKeyFile(path);
  } catch (e) {
    if ((e as { code?: unknown }).code !== "ENOENT") throw e;
    const key = randomBytes(KEY_BYTES);
    writeKeyFile(path, key);
    return key;
  }
}

interface Seen {
  readonly hash: string;
  readonly authMethod?: string;
  readonly at: string;
}

/** What `claude-account.json` holds: hashes and one-word methods only. */
export interface AccountState {
  readonly v: 1;
  /** The account sessions run on; absent until the CLI is first seen signed in. */
  readonly current?: Seen;
  /** A different account seen since, waiting for a person's confirmation. */
  readonly pending?: Seen;
  /** `provider signin claudeCode --confirm`: the next different account is taken without one. */
  readonly expectNext?: boolean;
}

export interface Observation {
  readonly hash: string;
  /** A change waits for a confirmation: new sessions are refused. */
  readonly changed: boolean;
}

/** `PROVIDER_ACCOUNT_CHANGED` (exit 5). */
export function accountChanged(hash?: string): FinError {
  return finError(
    "PROVIDER_ACCOUNT_CHANGED",
    "Claude Code is signed in to another account than the one this server's sessions ran on",
    { harness: "claudeCode", ...(hash === undefined ? {} : { accountHash: hash }) },
    "a person at the server confirms the new account with `finisherd provider account confirm claudeCode`",
  );
}

/** The account state in `<server-data>/agents/claude-account.json`. Only the server writes it. */
export class ClaudeAccounts {
  readonly #path: string;
  readonly #now: () => Date;

  constructor(serverData: string, now: () => Date = () => new Date()) {
    this.#path = join(serverData, "agents", ACCOUNT_FILE);
    this.#now = now;
  }

  /** The state; a missing or unreadable file is the empty state (the next sign-in is the account). */
  state(): AccountState {
    try {
      const v = JSON.parse(readFileSync(this.#path, "utf8")) as AccountState;
      return v !== null && typeof v === "object" && v.v === 1 ? v : { v: 1 };
    } catch {
      return { v: 1 };
    }
  }

  #write(s: AccountState): void {
    mkdirSync(join(this.#path, ".."), { recursive: true, mode: 0o700 });
    const tmp = `${this.#path}.${process.pid}.tmp`;
    try {
      writeFileSync(tmp, `${JSON.stringify(s)}\n`, { mode: 0o600 });
      renameSync(tmp, this.#path);
    } catch (e) {
      rmSync(tmp, { force: true });
      throw e;
    }
  }

  #seen(hash: string, authMethod: string | undefined): Seen {
    return { hash, ...(authMethod === undefined ? {} : { authMethod }), at: this.#now().toISOString() };
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
    if (s.current.hash === hash) {
      // Back on the recorded account: nothing waits any more.
      if (s.pending !== undefined || s.current.authMethod !== authMethod) this.#write({ v: 1, current: this.#seen(hash, authMethod), ...(s.expectNext === true ? { expectNext: true } : {}) });
      return { hash, changed: false };
    }
    if (s.expectNext === true) {
      this.#write({ v: 1, current: this.#seen(hash, authMethod) });
      return { hash, changed: false };
    }
    if (s.pending?.hash !== hash) this.#write({ ...s, pending: this.#seen(hash, authMethod) });
    return { hash, changed: true };
  }

  /** Whether a change waits for a confirmation, and its hash. */
  pending(): string | undefined {
    return this.state().pending?.hash;
  }

  /** A person takes the pending account as the one sessions run on. */
  confirm(): { confirmed: boolean; accountHash?: string; previous?: string } {
    const s = this.state();
    if (s.pending === undefined) return { confirmed: false, ...(s.current === undefined ? {} : { accountHash: s.current.hash }) };
    this.#write({ v: 1, current: s.pending });
    return { confirmed: true, accountHash: s.pending.hash, ...(s.current === undefined ? {} : { previous: s.current.hash }) };
  }

  /** `provider signin claudeCode --confirm`: the next account seen is taken without a confirmation. */
  expectNext(): void {
    this.#write({ ...this.state(), expectNext: true });
  }
}

/**
 * The session layer's check at each instance's start (Task 7 calls it with the `auth status` it
 * ran under the session environment, or what `initialize.account` reports): records the account
 * and answers its hash (for `ClaudeModelCache.recordInstanceModels`) and, for a **new** session
 * while a change waits, `PROVIDER_ACCOUNT_CHANGED`. A running session's instance (a respawn, a
 * resume) is never refused: it started on the earlier check. Signed out: no hash, no refusal
 * (the CLI's own error is `PROVIDER_AUTH`).
 */
export function instanceAccount(serverData: string, status: AuthStatus, o: { readonly newSession: boolean; readonly accounts?: ClaudeAccounts }): { hash?: string; refused: FinError | null } {
  if (!status.signedIn) return { refused: null };
  const seen = (o.accounts ?? new ClaudeAccounts(serverData)).observe(status, accountKey(serverData));
  return { hash: seen.hash, refused: seen.changed && o.newSession ? accountChanged(seen.hash) : null };
}
