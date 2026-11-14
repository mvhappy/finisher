// Claude Code's account label (plan 7D Decision 19, spec §8.6): what names the account in the
// CLI's `auth status --json` and `initialize.account`. The record, the key, the hash and the
// confirm/expect rules are harness-neutral and live in `../cli/account.ts` (plan 7E Task 1),
// which keeps Claude Code's state in `agents/claude-account.json` as 7D wrote it.
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
//   in the detect host sandbox (the home read only, its store read-write; off Windows: TODO(7D
//   Task 13)), and hashes the `email`, else the
//   `orgId`; signed in naming neither is `PROVIDER_AUTH` there. An instance's
//   `initialize.account.email` (present once the CLI holds the profile in the session's own config
//   dir: a resumed process, or the profile fetch finishing first) is recorded too, under the same
//   subject as detection's e-mail, so both give one hash.
// - **Nothing names the account**: off Windows that is `PROVIDER_AUTH {reason: noAccount}` (above;
//   a method alone never identifies an account). On Windows, where detection keeps a throwaway
//   config dir, nothing is recorded and there is no refusal by itself: a new session is refused
//   only while a change already waits or the record cannot be read (`instanceAccount`).
// - **Residual (review of Decision 19, S2)**: the hashed label is the sign-in home's cached
//   profile (`.claude.json` `oauthAccount`), not bound to the credential. A session's `agent`
//   profile reads and writes `home/store/`, so it can swap `.credentials.json` for another
//   account's token while detection keeps hashing the old profile; and on a resume
//   `initialize.account.email` comes from the session's own `provider/.claude.json`, which the
//   session can write. So "a switch is seen on the first session" holds only for a switch made
//   with `auth login` (which rewrites the profile). TODO(7D Task 18): that the person's `auth
//   login` fills the sign-in home's `.claude.json` (only the owner's long-lived dir was seen).
//
// **Finisher opens nothing under `providers/`**: the label comes from what the CLI prints.
/**
 * The fields of `auth status --json` (and `initialize.account`) that name the signed-in account;
 * their values are joined into the hashed label. Verified on 2.1.321 (7D Task 0): `email` is the
 * one both carry (`auth status` also has `orgId`/`orgName`, `initialize.account` never). Never
 * `subscriptionType`: its text varies for one account ("Claude Max", "Claude Ultra").
 */
export const LABEL_FIELDS = ["email"] as const;

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
