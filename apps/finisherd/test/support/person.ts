// A person at the console's terminal (owner decision of 2026-11-08): asked `Proceed? [y/N]`, or
// to type a name shown back. The console never shows them a challenge code any more.
import type { Prompt } from "../../src/console/prompt.ts";

export type Person = Prompt & { shown: string[]; asked: string[] };

/**
 * Answers each question with the next of `answers`, then `""` (which cancels). Without
 * `answers` the person confirms everything: `y` to `Proceed? [y/N]`, the name the console
 * showed to `Type the … to confirm:`, `""` to anything else.
 */
export function person(...answers: string[]): Person {
  const shown: string[] = [];
  const asked: string[] = [];
  const scripted = answers.length > 0;
  let name = "";
  return {
    shown,
    asked,
    isInteractive: () => true,
    show: (text) => {
      shown.push(text);
      const m = /^To confirm, type the .+?: (.+)$/.exec(text);
      if (m) name = m[1]!;
    },
    ask: async (question) => {
      asked.push(question);
      if (scripted) return answers.shift() ?? "";
      if (question === "Proceed? [y/N] ") return "y";
      return /^Type the .+ to confirm: $/.test(question) ? name : "";
    },
  };
}
