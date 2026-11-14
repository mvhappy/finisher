// A mutating human-only command (spec §4.7; plan 5A Decision 30, owner decision of 2026-11-08):
// it needs an interactive terminal (`CONSOLE_NEEDS_TTY`, checked before connecting), says what
// it will do, then asks the person in one of two ways:
//
// - `proceed`: a plain `Proceed? [y/N]`, for most human-only actions;
// - `typeName(noun, name)`: the target's name typed back, GitHub-style, for the ones that cannot
//   be undone or destroy something (a purge, a removal, a key rotation, a takeover).
//
// Only after that does it ask the server for a `console.challenge` on its own connection and
// send the call with that code in `challenge`: the server still refuses a confirmed call without
// a fresh code of the same connection, but the person never sees or types it. A code the server
// turns down as wrong or expired is replaced once, without asking again. The code is never
// shown, logged or put in an error.
import { type ConsoleConnection, ConsoleError, consoleError, valueOf } from "./client.ts";
import type { Prompt } from "./prompt.ts";
import { safe } from "./safe.ts";

/** Tries a person gets at typing a name before the command gives up. */
export const CONFIRM_TRIES = 3;

/** How the person confirms: `y`, or the target's name typed back. */
export type Confirmation = { readonly kind: "proceed" } | { readonly kind: "name"; readonly noun: string; readonly name: string };

/** `Proceed? [y/N]`. */
export const proceed: Confirmation = { kind: "proceed" };

/** Type `name` back to confirm; `noun` says what it is ("library's name", "word"). */
export const typeName = (noun: string, name: string): Confirmation => ({ kind: "name", noun, name: safe(name) });

/** `CONSOLE_NEEDS_TTY` (exit 7): the fix speaks to the person, in words. */
export function needsTty(command: string): ConsoleError {
  return consoleError(
    "CONSOLE_NEEDS_TTY",
    `${command} needs an interactive terminal`,
    { command },
    "Run this command yourself in a terminal on the server: it says what it will do and goes ahead once you confirm it there.",
  );
}

/** Refuses unless a person can answer (before anything connects or changes). */
export function requireTty(prompt: Prompt, command: string): void {
  if (!prompt.isInteractive()) throw needsTty(command);
}

const cancelled = (tag: string | undefined, why: string): ConsoleError =>
  consoleError("CONFIRMATION_REQUIRED", `${why}, so nothing was changed`, { ...(tag === undefined ? {} : { tag }), reason: "cancelled" });

/**
 * Shows `what` and asks the person as `how` says; throws `CONFIRMATION_REQUIRED` (`cancelled`)
 * unless they confirm. Nothing is sent.
 */
export async function ask(prompt: Prompt, what: string, how: Confirmation, tag?: string): Promise<void> {
  prompt.show(what);
  if (how.kind === "proceed") {
    const answer = (await prompt.ask("Proceed? [y/N] ")).trim().toLowerCase();
    if (answer !== "y" && answer !== "yes") throw cancelled(tag, "you did not answer y");
    return;
  }
  prompt.show(`To confirm, type the ${how.noun}: ${how.name.trim()}`);
  for (let attempt = 1; ; attempt++) {
    const typed = (await prompt.ask(`Type the ${how.noun} to confirm: `)).trim();
    // Compared trimmed: a name the server allows with surrounding spaces can still be typed.
    if (typed === how.name.trim()) return;
    if (typed.length === 0) throw cancelled(tag, "nothing was typed");
    if (attempt >= CONFIRM_TRIES) throw cancelled(tag, `the ${how.noun} was not typed back`);
    prompt.show(`That is not ${how.name.trim()}. Type it exactly as shown, or press Enter to cancel.`);
  }
}

/**
 * Sends `tag` with `payload` and a fresh `console.challenge` code of `conn` (fetched here, never
 * shown); a wrong or expired code is replaced once.
 */
export async function withChallenge<A>(conn: ConsoleConnection, tag: string, payload: Record<string, unknown>): Promise<A> {
  for (let attempt = 1; ; attempt++) {
    const { code } = valueOf<{ code: string; expiresAt: number }>(await conn.call("console.challenge", {}));
    const exit = await conn.call(tag, { ...payload, challenge: code });
    if (exit.ok) return exit.value as A;
    const reason = (exit.error.details as { reason?: unknown } | undefined)?.reason;
    const retry = exit.error.code === "CONFIRMATION_REQUIRED" && (reason === "wrong" || reason === "expired") && attempt < 2;
    if (!retry) throw new ConsoleError(exit.error);
  }
}

/** Runs `tag` with `payload` once the person has confirmed `what` as `how` says (module docs). */
export async function confirmed<A>(
  conn: ConsoleConnection,
  prompt: Prompt,
  what: string,
  how: Confirmation,
  tag: string,
  payload: Record<string, unknown>,
): Promise<A> {
  await ask(prompt, what, how, tag);
  return withChallenge<A>(conn, tag, payload);
}
