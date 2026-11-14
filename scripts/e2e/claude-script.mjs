// What `fake-claude` plays in the "claude" scenario (7D Task 16): one script file per phase, read by
// each CLI process when it starts (the sidecar names it), so the scenario rewrites it between
// processes. The CLI's `Bash` tool is the editing route: each call runs `fin` as the session's
// credential, on the session's branch.
import { FIN } from "./lib.mjs";

export const PROMPTS = {
  work: "mark the intro and check curl",
  steer: "also mark the outro",
  hold: "wait for me",
  crash: "mark the credits",
  carryOn: "carry on",
  budget: "mark the end card",
  budgetMore: "go on within the new budget",
};

export const LABELS = { work: "intro", steered: "outro", killed: "credits" };

const q = (s) => `'${s.replaceAll("'", "'\\''")}'`;
/** A `Bash` call as the CLI announces it, run when the runtime allows it. */
export const bash = (command, description, toolUseId) => ({ ask: { tool: "Bash", bash: true, input: { command, description }, ...(toolUseId === undefined ? {} : { toolUseId }) } });
/** `fin marker add` at `at` seconds. */
export const mark = (label, at = 1) => bash(`${q(FIN)} marker add --at ${at}s --label ${label}`, `mark ${label}`);

export const CURL = "curl --version";
const result = (inputTokens = 10, outputTokens = 5) => ({ cumulativeUsage: { inputTokens, outputTokens } });
/** A plan sign-in (what `claude.ai` reports): tokens are charged, money is shown. */
const SIGNED = { init: { apiKeySource: "/login managed key" } };

/**
 * Process 1 of session A. Turn 1 marks the intro and asks for `curl` (the steer is sent while the
 * person has not answered it yet, so the runtime holds it for the next CLI turn of the same run);
 * turn 2 is the steer; turn 3 waits for an interrupt, which ends the process (the runtime closes
 * the CLI after an interrupt and resumes its transcript with the next prompt).
 */
export function sessionA() {
  return {
    ...SIGNED,
    steps: [
      { expect: "user", ...mark(LABELS.work) },
      bash(CURL, "check curl"),
      result(),
      { expect: "user", ...mark(LABELS.steered, 2) },
      result(),
      { expect: "user", sleep: 120_000 },
      result(),
    ],
  };
}

/**
 * The process that resumes A after the interrupt: one shell call commits a marker, then holds
 * until the server is killed. Run again, the same call would commit a second marker: the side
 * effect the runtime must refuse when the resumed CLI asks for it again.
 */
export function sessionACrash() {
  return { ...SIGNED, steps: [{ expect: "user", ...bash(`${q(FIN)} marker add --at 3s --label ${LABELS.killed}; sleep 300`, "mark the credits, then wait", "toolu_crash_1") }, result()] };
}

/** The process after the restart: the CLI re-asks the call it left open (allowed, it runs again); one turn. */
export function sessionAResumed() {
  return { ...SIGNED, onResumeDangling: "reask", steps: [{ expect: "user", ...result(7, 3) }] };
}

/** Session B, first process: the stream's usage passes its token limit mid-turn. */
export function sessionB() {
  return { ...SIGNED, steps: [{ expect: "user", liveUsage: { inputTokens: 1500, outputTokens: 10 } }, { sleep: 60_000, ...result(1, 1) }] };
}

/** Session B's next process (the budget stop closed the CLI): one marker and a short turn. */
export function sessionBMore() {
  return { ...SIGNED, steps: [{ expect: "user", ...bash(`${q(FIN)} marker add --at 4s --label endcard`, "mark the end card", "toolu_more_1") }, result(20, 8)] };
}
