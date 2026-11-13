// What a person's sign-in (plan 7D Decision 23's `auth login`) leaves in Claude Code's sign-in
// home `<server-data>/providers/claudeCode/home/`: the store folder and `.claude.json` holding the
// account's profile (`oauthAccount`), which detection's `auth status` names the account from
// (Decision 19 as 7D Task 0 amended it; fake-claude reads it as 2.1.321 does). Written by the
// test, never by finisherd.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/** The e-mail a signed-in fake names by default (`fake-claude`'s `account` knob uses its own). */
export const SIGNED_IN_EMAIL = "tester@example.invalid";

/** Signs Claude Code in under `serverData` to `email` (the sign-in home's profile). */
export function signInClaude(serverData: string, email: string = SIGNED_IN_EMAIL): void {
  const home = join(serverData, "providers", "claudeCode", "home");
  mkdirSync(join(home, "store"), { recursive: true, mode: 0o700 });
  writeFileSync(join(home, ".claude.json"), JSON.stringify({ oauthAccount: { emailAddress: email, organizationUuid: `org-of-${email}` } }), { mode: 0o600 });
}
