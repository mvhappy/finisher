// A loopback model server for the "agent" scenario: Chat Completions as a local server (Ollama,
// LM Studio) serves it, scripted by what the conversation holds. `respond(turn)` is called per
// request with `{ prompt, toolMessages, last, n }` and answers `{ tool, args }` (one tool call)
// or `{ text }`, each with the usage it reports; the stream is a template under
// `fixtures/agent-streams/`. Every request's exact body is recorded (the scenario greps them for
// secrets and counts them). Loopback only: nothing here reaches a network.
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

const template = (name) => readFileSync(new URL(`./fixtures/agent-streams/${name}.sse`, import.meta.url), "utf8");
const TOOL = template("tool");
const TEXT = template("text");

const fill = (text, values) => text.replace(/@([A-Z]+)@/g, (_, k) => String(values[k]));

/** The text of a message's content (a string, or an array of parts). */
const textOf = (content) => (typeof content === "string" ? content : Array.isArray(content) ? content.map((p) => p?.text ?? "").join("") : "");

export async function startModelServer({ model, respond }) {
  const requests = [];
  let n = 0;
  const server = createServer((req, res) => {
    const chunks = [];
    req.on("data", (c) => chunks.push(c));
    req.on("end", () => {
      const raw = Buffer.concat(chunks).toString("utf8");
      if (req.method === "GET" && req.url === "/v1/models") {
        res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ object: "list", data: [{ id: model, object: "model" }] }));
        return;
      }
      if (req.method !== "POST" || req.url !== "/v1/chat/completions") {
        res.writeHead(404, { "content-type": "application/json" }).end(JSON.stringify({ error: { message: "no such route" } }));
        return;
      }
      const body = JSON.parse(raw);
      requests.push({ headers: req.headers, raw, body });
      const messages = body.messages ?? [];
      const prompt = textOf(messages.find((m) => m.role === "user")?.content);
      const tools = messages.filter((m) => m.role === "tool");
      const answer = respond({ prompt, toolMessages: tools.length, last: textOf(tools.at(-1)?.content), n: n++, body });
      const id = `e2e-${n}`;
      const usage = { IN: answer.in ?? 100, OUT: answer.out ?? 20 };
      usage.TOTAL = usage.IN + usage.OUT;
      const sse =
        answer.tool === undefined
          ? fill(TEXT, { ID: id, MODEL: model, TEXT: JSON.stringify(answer.text ?? "Done."), ...usage })
          : fill(TOOL, { ID: id, MODEL: model, CALL: `call_${n}`, TOOL: answer.tool, ARGS: JSON.stringify(JSON.stringify(answer.args ?? {})), ...usage });
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" }).end(sse);
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  return {
    baseUrl: `http://127.0.0.1:${port}/v1`,
    requests: () => [...requests],
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}
