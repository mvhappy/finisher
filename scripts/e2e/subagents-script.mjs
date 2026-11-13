// What the scripted model of the "subagents" scenario says (plan 7B Task 13): one reply per
// request, chosen by the current run's prompt (the last user message) and the tool results that
// follow it in the conversation. A parent reads its children's ids from its own `fin agent spawn`
// results, as a model would. Loopback only: `startModelServer` (agent-model.mjs) serves it.
export const MODEL = "llama3.3:70b";

/** A request's window (`agents.models`): run 2 of the compaction session reports enough input to pass 75 % of it. */
export const WINDOW = 32_000;

export const PROMPTS = {
  mark: "MARK the intro twice",
  spawn: "SPAWN two clip finders",
  findOne: "FIND clip one",
  findTwo: "FIND clip two",
  merge: "MERGE",
  chain: "CHAIN",
  compactOne: "COMPACT first run",
  compactTwo: "COMPACT second run",
  review: "REVIEW the second clip",
};

export const SUMMARY = "The person asked for a first run and then a second one; nothing is pending.";

/** The opening words of the compaction prompt (`agent/builtin/prompts/compaction-v1.md`). */
const COMPACTION = "The conversation above is about to be replaced by a summary";

const textOf = (content) => (typeof content === "string" ? content : Array.isArray(content) ? content.map((p) => p?.text ?? "").join("\n") : "");

/** A `fin` tool result's parsed stdout (the built-in tool's `{exit, stdout, error?}`), else the whole output. */
export function finResult(content) {
  let out;
  try {
    out = JSON.parse(content);
  } catch {
    return { exit: null, raw: content };
  }
  let stdout = out?.stdout;
  if (typeof stdout === "string") {
    try {
      stdout = JSON.parse(stdout);
    } catch {
      /* text output */
    }
  }
  return { exit: out?.exit ?? null, stdout, error: out?.error ?? stdout?.error };
}

const fin = (...argv) => ({ tool: "fin", args: { argv } });
const spawn = (brief) => fin("agent", "spawn", "--role", "clip-finder", "--brief", brief, "--model", MODEL);

/** The steps of a run: each gets the parsed results of the steps before it. */
function steps(prompt) {
  if (prompt.includes(PROMPTS.mark)) return [() => fin("marker", "add", "--at", "1s", "--label", "first"), () => fin("marker", "add", "--at", "1s", "--label", "first")];
  if (prompt.includes(PROMPTS.spawn)) {
    return [
      () => spawn(`${PROMPTS.findOne}: trim c1`),
      () => spawn(`${PROMPTS.findTwo}: trim c2`),
      ([a, b]) => fin("agent", "wait", a.stdout.session, b.stdout.session, "--any", "--timeout", "60"),
      ([a, b, first]) => {
        const done = first.stdout.children.find((c) => c.status === "ended" || c.status === "idle").child;
        return fin("agent", "wait", done === a.stdout.session ? b.stdout.session : a.stdout.session, "--any", "--timeout", "60");
      },
    ];
  }
  if (prompt.includes(PROMPTS.findOne)) return [() => fin("item", "trim", "c1", "--end", "-0.5s")];
  if (prompt.includes(PROMPTS.findTwo)) return [() => fin("item", "trim", "c2", "--end", "-0.5s")];
  const merge = new RegExp(`${PROMPTS.merge} (\\S+)`).exec(prompt);
  if (merge !== null) return [() => fin("merge", merge[1])];
  const chain = new RegExp(`${PROMPTS.chain} (\\d+)`).exec(prompt);
  if (chain !== null) {
    // Each level spawns the next and waits for it; the level whose spawn is refused stops there.
    return [() => spawn(`${PROMPTS.chain} ${Number(chain[1]) + 1}`), ([s]) => (s.exit === 0 ? fin("agent", "wait", s.stdout.session, "--timeout", "60") : undefined)];
  }
  // Run 2 of the compaction session: the first call reports most of the window as its input.
  if (prompt.includes(PROMPTS.compactTwo)) return [() => ({ ...fin("view", "--budget", "2000"), in: 25_000 })];
  if (prompt.includes(PROMPTS.review)) return [() => fin("item", "trim", "c2", "--end", "-1s"), () => fin("review", "request", "--title", "Trim the second clip")];
  return [];
}

/** `respond` for `startModelServer`: a tool call per step, then the run's closing text. */
export function scripted({ body }) {
  const messages = body.messages ?? [];
  const last = messages.at(-1);
  if (last?.role === "user" && textOf(last.content).startsWith(COMPACTION)) return { text: SUMMARY, in: 3_000, out: 40 };
  const at = messages.findLastIndex((m) => m.role === "user");
  const prompt = textOf(messages[at]?.content);
  const results = messages.slice(at + 1).filter((m) => m.role === "tool").map((m) => finResult(textOf(m.content)));
  const plan = steps(prompt);
  const next = results.length < plan.length ? plan[results.length](results) : undefined;
  if (next !== undefined) return next;
  if (prompt.includes(PROMPTS.findOne)) return { text: "Clip one: c1 trimmed by half a second." };
  if (prompt.includes(PROMPTS.findTwo)) return { text: "Clip two: c2 trimmed by half a second." };
  if (prompt.includes(PROMPTS.compactTwo)) return { text: "Second run done.", in: 1_000 };
  return { text: "Done." };
}
