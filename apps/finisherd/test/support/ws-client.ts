// A WebSocket RPC client for the tests (5A Task 6): mints a one-time ticket over a proved socket
// connection and offers it in `Sec-WebSocket-Protocol` beside `finisher.v2`.
import { WebSocket } from "ws";
import { connectProved, ok, type ServerInfo } from "./ndjson-client.ts";

export interface WsClient {
  call(tag: string, payload: unknown): Promise<Record<string, unknown>>;
  /** `tag` with a fresh `console.challenge` of this connection typed back (a `console` ticket's socket). */
  confirmed(tag: string, payload: Record<string, unknown>): Promise<Record<string, unknown>>;
  readonly protocol: string;
  close(): void;
}

/** A ticket for `credential`, minted over the socket. */
export async function mintTicket(server: { info: ServerInfo; serverData: string }, credential: "local" | "console" = "local", ttlS?: number): Promise<string> {
  const c = await connectProved(server, credential);
  try {
    return ok<{ ticket: string }>(await c.call("auth.ticket", ttlS === undefined ? {} : { ttlS })).ticket;
  } finally {
    await c.close();
  }
}

/** Opens `/rpc` with `protocols` (and `headers`); rejects with the HTTP status of a refused upgrade. */
export function openWs(port: number, protocols: string[], headers: Record<string, string> = {}, path = "/rpc"): Promise<WsClient> {
  return new Promise((resolve, reject) => {
    const sock = new WebSocket(`ws://127.0.0.1:${port}${path}`, protocols, { headers });
    const waiting = new Map<string, (f: Record<string, unknown>) => void>();
    let next = 1;
    sock.on("message", (data) => {
      for (const line of String(data).split("\n").filter(Boolean)) {
        const f = JSON.parse(line) as Record<string, unknown>;
        waiting.get(f.requestId as string)?.(f);
      }
    });
    sock.once("unexpected-response", (_req, res) => reject(Object.assign(new Error(`HTTP ${res.statusCode}`), { status: res.statusCode })));
    sock.once("error", reject);
    const call = (tag: string, payload: unknown) =>
      new Promise<Record<string, unknown>>((done) => {
        const id = String(next++);
        waiting.set(id, done);
        sock.send(`${JSON.stringify({ _tag: "Request", id, tag, payload, headers: [] })}\n`);
      });
    sock.once("open", () =>
      resolve({
        protocol: sock.protocol,
        call,
        confirmed: async (tag, payload) => {
          const exit = (await call("console.challenge", {})).exit as { _tag: string; value?: { code: string } };
          if (exit._tag !== "Success") throw new Error(`console.challenge failed: ${JSON.stringify(exit)}`);
          return call(tag, { ...payload, challenge: exit.value!.code });
        },
        close: () => sock.close(),
      }),
    );
  });
}

/** A WebSocket carrying a fresh `credential` ticket's principal. */
export async function connectWs(server: { info: ServerInfo; serverData: string }, credential: "local" | "console" = "local"): Promise<WsClient> {
  const ticket = await mintTicket(server, credential);
  return openWs(server.info.wsPort, ["finisher.v2", `finisher.ticket.${ticket}`]);
}
