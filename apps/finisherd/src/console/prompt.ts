// What the console shows and asks (plan 5A Decision 30): through this interface, so unit tests
// drive it without a terminal. The real one needs stdin and stdout to be terminals.
import { createInterface } from "node:readline";
import { Writable } from "node:stream";
import { safe } from "./safe.ts";

/** `text` line by line with the controls and bidirectional overrides taken out (review M1). */
const plain = (text: string): string => text.split("\n").map(safe).join("\n");

export interface Prompt {
  /** Whether a person can be asked: stdin and stdout are both terminals. */
  isInteractive(): boolean;
  /** Writes `text` and a newline to the person. */
  show(text: string): void;
  /** Asks `question` and resolves with the line typed (`""` at end of input). */
  ask(question: string): Promise<string>;
  /** Whether the last `ask` ended with Ctrl-C (`pair` stops on it; others treat it as no answer). */
  interrupted?(): boolean;
  /** Like `ask`, without echoing what is typed (a key pasted for `provider key set`). */
  secret?(question: string): Promise<string>;
}

/**
 * The process's terminal. A pipe on either side (`yes |`, `> file`, a script without a
 * pseudo-terminal) is not interactive. A program that drives a pseudo-terminal (`script`,
 * `expect`) is: the typed, server-issued code keeps pasted scripts and pipes out, not same-user
 * code built to read the screen (spec §4.6, owner decision 6; sub-project 7's sandbox closes it).
 */
export function ttyPrompt(stdin: NodeJS.ReadStream = process.stdin, stdout: NodeJS.WriteStream = process.stdout): Prompt {
  let interrupted = false;
  return {
    isInteractive: () => stdin.isTTY === true && stdout.isTTY === true,
    interrupted: () => interrupted,
    show: (text) => void stdout.write(`${plain(text)}\n`),
    ask: (question) =>
      new Promise((resolve) => {
        const rl = createInterface({ input: stdin, output: stdout, terminal: true });
        let answered = false;
        interrupted = false;
        rl.question(plain(question), (line) => {
          answered = true;
          rl.close();
          resolve(line);
        });
        // Ctrl-C at the question cancels it, as end of input does (review of Tasks 8–9, M2:
        // the command then cleans up, removing a reserved credential file).
        rl.once("SIGINT", () => {
          interrupted = true;
          rl.close();
        });
        rl.once("close", () => {
          if (!answered) resolve("");
        });
      }),
    secret: (question) =>
      new Promise((resolve) => {
        // Nothing typed is written back: readline's own echo goes through a muted output.
        let muted = false;
        const output = new Writable({
          write: (chunk, _enc, done) => {
            if (!muted) stdout.write(chunk);
            done();
          },
        });
        const rl = createInterface({ input: stdin, output, terminal: true });
        let answered = false;
        rl.question(plain(question), (line) => {
          answered = true;
          stdout.write("\n");
          rl.close();
          resolve(line);
        });
        muted = true;
        rl.once("SIGINT", () => rl.close());
        rl.once("close", () => {
          if (!answered) resolve("");
        });
      }),
  };
}
