// What the console shows and asks (plan 5A Decision 30): through this interface, so unit tests
// drive it without a terminal. The real one needs stdin and stdout to be terminals.
import { createInterface } from "node:readline";

export interface Prompt {
  /** Whether a person can be asked: stdin and stdout are both terminals. */
  isInteractive(): boolean;
  /** Writes `text` and a newline to the person. */
  show(text: string): void;
  /** Asks `question` and resolves with the line typed (`""` at end of input). */
  ask(question: string): Promise<string>;
}

/**
 * The process's terminal. A pipe on either side (`yes |`, `> file`, a script without a
 * pseudo-terminal) is not interactive. A program that drives a pseudo-terminal (`script`,
 * `expect`) is: the typed, server-issued code keeps pasted scripts and pipes out, not same-user
 * code built to read the screen (spec §4.6, owner decision 6; sub-project 7's sandbox closes it).
 */
export function ttyPrompt(stdin: NodeJS.ReadStream = process.stdin, stdout: NodeJS.WriteStream = process.stdout): Prompt {
  return {
    isInteractive: () => stdin.isTTY === true && stdout.isTTY === true,
    show: (text) => void stdout.write(`${text}\n`),
    ask: (question) =>
      new Promise((resolve) => {
        const rl = createInterface({ input: stdin, output: stdout, terminal: true });
        let answered = false;
        rl.question(question, (line) => {
          answered = true;
          rl.close();
          resolve(line);
        });
        rl.once("close", () => {
          if (!answered) resolve("");
        });
      }),
  };
}
