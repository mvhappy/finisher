// A mutating human-only command (spec §4.7, plan 5A Decision 30): it needs an interactive
// terminal (`CONSOLE_NEEDS_TTY`, checked before connecting), says what it will do, asks the
// server for a `console.challenge` on its own connection, shows the code, and sends the call
// with `challenge: <typed>` only after the person types it. A wrong code voids the
// connection's codes, so the next try asks for a new one; an expired one too. The code is shown
// on this terminal and nowhere else: never logged, never in an error.
import { type ConsoleConnection, ConsoleError, consoleError, valueOf } from "./client.ts";
import type { Prompt } from "./prompt.ts";

/** Tries a person gets before the command gives up. */
export const CONFIRM_TRIES = 3;

/** `CONSOLE_NEEDS_TTY` (exit 7): the fix speaks to the person, in words. */
export function needsTty(command: string): ConsoleError {
  return consoleError(
    "CONSOLE_NEEDS_TTY",
    `${command} needs an interactive terminal`,
    { command },
    "Run this command yourself in a terminal on the server: it shows a short code and goes ahead once you type it back.",
  );
}

/** Refuses unless a person can answer (before anything connects or changes). */
export function requireTty(prompt: Prompt, command: string): void {
  if (!prompt.isInteractive()) throw needsTty(command);
}

/**
 * Runs `tag` with `payload` once the person has typed the code the server issued to `conn`.
 * `what` says what will happen, in words.
 */
export async function confirmed<A>(
  conn: ConsoleConnection,
  prompt: Prompt,
  what: string,
  tag: string,
  payload: Record<string, unknown>,
): Promise<A> {
  prompt.show(what);
  for (let attempt = 1; ; attempt++) {
    const { code } = valueOf<{ code: string; expiresAt: number }>(await conn.call("console.challenge", {}));
    prompt.show(`To go ahead, type this code within 60 seconds: ${code}`);
    const typed = (await prompt.ask("Code: ")).trim();
    if (typed.length === 0) throw consoleError("CONFIRMATION_REQUIRED", "nothing was typed, so nothing was changed", { tag, reason: "cancelled" });
    const exit = await conn.call(tag, { ...payload, challenge: typed });
    if (exit.ok) return exit.value as A;
    const reason = (exit.error.details as { reason?: unknown } | undefined)?.reason;
    if (exit.error.code !== "CONFIRMATION_REQUIRED" || reason === "missing" || attempt >= CONFIRM_TRIES) throw new ConsoleError(exit.error);
    prompt.show(reason === "expired" ? "That code expired. Here is a new one." : "That code did not match. Here is a new one.");
  }
}
