// A loopback model server speaking Chat Completions as OpenRouter serves it (plan 7A Task 9):
// each request takes the next scripted response (an SSE stream, an error status, or a stream cut
// short), and every request's exact bytes are recorded. Tests never leave loopback.
import { readFileSync } from "node:fs";
import { createServer, type IncomingHttpHeaders } from "node:http";
import type { AddressInfo } from "node:net";

export type FakeResponse =
  /** An SSE body (a fixture's text); `cutAfter` bytes are written and then the socket is destroyed. */
  | { readonly sse: string; readonly cutAfter?: number; readonly headers?: Record<string, string> }
  /** A plain JSON error. */
  | { readonly status: number; readonly body: unknown; readonly headers?: Record<string, string> };

export interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: IncomingHttpHeaders;
  readonly body: Buffer;
  readonly json: Record<string, unknown>;
}

export interface FakeModelServer {
  readonly baseUrl: string;
  requests(): RecordedRequest[];
  close(): Promise<void>;
}

/** A fixture under `test/fixtures/model-streams/openrouter/`. */
export function sse(name: string): string {
  return readFileSync(new URL(`../fixtures/model-streams/openrouter/${name}.sse`, import.meta.url), "utf8");
}

export async function startFakeModelServer(scripts: readonly FakeResponse[]): Promise<FakeModelServer> {
  const recorded: RecordedRequest[] = [];
  let next = 0;
  const server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      let json: Record<string, unknown> = {};
      try {
        json = JSON.parse(body.toString("utf8")) as Record<string, unknown>;
      } catch {
        // Recorded as is.
      }
      recorded.push({ method: req.method ?? "", path: req.url ?? "", headers: req.headers, body, json });
      const script = scripts[Math.min(next++, scripts.length - 1)];
      if (script === undefined || req.url !== "/api/v1/chat/completions") {
        res.writeHead(404, { "content-type": "application/json" }).end(JSON.stringify({ error: { code: 404, message: "no such route" } }));
        return;
      }
      if ("status" in script) {
        res.writeHead(script.status, { "content-type": "application/json", ...script.headers }).end(JSON.stringify(script.body));
        return;
      }
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", ...script.headers });
      if (script.cutAfter !== undefined) {
        res.write(script.sse.slice(0, script.cutAfter), () => res.socket?.destroy());
        return;
      }
      res.end(script.sse);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address() as AddressInfo;
  return {
    baseUrl: `http://127.0.0.1:${port}/api/v1`,
    requests: () => [...recorded],
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}
