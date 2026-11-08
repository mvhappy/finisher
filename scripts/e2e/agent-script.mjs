// What the scripted model of the "agent" scenario says (7A Task 13): one reply per request, chosen
// by the conversation's first user message (its prompt) and the number of tool results already in it,
// so a resumed session (whose rebuilt history holds the synthetic result of an interrupted call)
// continues the same script.
export const MODEL = "llama3.3:70b";

export const PROMPTS = {
  intro: "tighten the intro",
  outro: "caption the outro",
  budget: "stay within the token budget",
  local: "check the local preset",
};

/** Timed words as `caption import --words` reads them. */
export const wordsJson = (n) => JSON.stringify(Array.from({ length: n }, (_, i) => ({ text: `word${i + 1}`, start: String(i * 0.4), end: String(i * 0.4 + 0.3) })));

/** `respond` for `startModelServer`: `{ clip, words, srt, track }` name what the calls act on. */
export function scripted({ clip, words, srt, track }) {
  const fin = (...argv) => ({ tool: "fin", args: { argv } });
  const caption = (flag, file) => fin("caption", "import", flag, file, "--track", track, "--style", "preset:bold");
  const scripts = {
    [PROMPTS.intro]: [fin("view", "--budget", "2000"), fin("item", "trim", clip, "--end", "-1s"), caption("--words", words)],
    // The outro's third call imports a long SRT: the widest window for a crash while a `fin` runs.
    [PROMPTS.outro]: [fin("view", "--budget", "2000"), fin("marker", "add", "--at", "1s", "--label", "outro"), caption("--srt", srt)],
    [PROMPTS.budget]: [{ ...fin("view", "--budget", "2000"), in: 120, out: 20 }, { ...fin("view", "--budget", "2000"), in: 120, out: 20 }],
    [PROMPTS.local]: [fin("view", "--budget", "2000")],
  };
  return ({ prompt, toolMessages }) => {
    const key = Object.keys(scripts).find((k) => prompt.includes(k));
    const steps = key === undefined ? [] : scripts[key];
    return toolMessages < steps.length ? steps[toolMessages] : { text: "Done." };
  };
}
