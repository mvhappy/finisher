// A deliberately raw Effect-RPC client over `node:net` with NDJSON framing: the frames `fin`
// writes. The console (`finisherd <command>`, plan 5A Task 8) talks to the server through it, and
// the tests and the fixture capture use it too (`test/support/ndjson-client.ts`).
import { connect, type Socket } from "node:net";

export type Frame = Record<string, unknown> & { _tag: string };
export type Exit =
  | { ok: true; value: unknown }
  | { ok: false; error: { _tag: string; code: string; message: string; details: unknown; fix?: string } };

export interface Transcript {
  readonly dir: ">>" | "<<";
  readonly line: string;
}

export class NdjsonClient {
  readonly transcript: Transcript[] = [];
  #socket: Socket;
  #buf = "";
  #nextId = 1;
  #pending = new Map<string, (exit: Exit) => void>();
  #chunks = new Map<string, (values: unknown[]) => void>();
  #frames: Array<(f: Frame) => void> = [];

  private constructor(socket: Socket) {
    this.#socket = socket;
    socket.setEncoding("utf8");
    socket.on("data", (d: string) => this.#onData(d));
    // A request still open when the server goes away fails instead of hanging.
    socket.on("close", () => {
      const gone = { ok: false as const, error: { _tag: "FinError", code: "SERVER_UNAVAILABLE", message: "the server closed the connection", details: {} } };
      for (const resolve of this.#pending.values()) resolve(gone);
      this.#pending.clear();
    });
  }

  /** `path`: the local socket; `{host, port}`: TCP (the agent listener, plan 7C Task 6). */
  static connect(path: string | { readonly host: string; readonly port: number }): Promise<NdjsonClient> {
    return new Promise((resolve, reject) => {
      const socket = connect(typeof path === "string" ? { path } : { host: path.host, port: path.port });
      socket.once("connect", () => resolve(new NdjsonClient(socket)));
      socket.once("error", reject);
    });
  }

  /** Writes one frame as a single NDJSON line. */
  send(frame: Frame): void {
    const line = JSON.stringify(frame);
    this.transcript.push({ dir: ">>", line });
    this.#socket.write(`${line}\n`);
  }

  /** Resolves with the next frame of any kind (for Ping/Pong). */
  nextFrame(): Promise<Frame> {
    return new Promise((resolve) => this.#frames.push(resolve));
  }

  /** `headers`: the request frame's `[name, value]` pairs (`fin` sends its actor there). */
  call(tag: string, payload: unknown, headers: ReadonlyArray<[string, string]> = []): Promise<Exit> {
    const id = String(this.#nextId++);
    const done = new Promise<Exit>((resolve) => this.#pending.set(id, resolve));
    this.send({ _tag: "Request", id, tag, payload, headers });
    return done;
  }

  /** Starts a streaming request; each `Chunk` is acked so the server sends the next one. */
  stream(tag: string, payload: unknown, onValue: (v: unknown) => void): { id: string; done: Promise<Exit> } {
    const id = String(this.#nextId++);
    this.#chunks.set(id, (values) => values.forEach(onValue));
    const done = new Promise<Exit>((resolve) => this.#pending.set(id, resolve));
    this.send({ _tag: "Request", id, tag, payload, headers: [] });
    return { id, done };
  }

  /** Numbers the next request 1 again (after the handshake, whose answers have all arrived). */
  restartIds(): void {
    this.#nextId = 1;
  }

  interrupt(id: string): void {
    this.send({ _tag: "Interrupt", requestId: id });
  }

  close(): Promise<void> {
    return new Promise((resolve) => {
      if (this.#socket.closed) return resolve();
      this.#socket.once("close", () => resolve());
      this.#socket.end();
    });
  }

  /** Resolves when the server closes the connection. */
  closed(): Promise<void> {
    return new Promise((resolve) => {
      if (this.#socket.closed) return resolve();
      this.#socket.once("close", () => resolve());
    });
  }

  /** Writes raw bytes (malformed or oversized frames). */
  write(data: string | Buffer): void {
    this.#socket.write(data);
  }

  #onData(data: string): void {
    this.#buf += data;
    let nl: number;
    while ((nl = this.#buf.indexOf("\n")) >= 0) {
      const line = this.#buf.slice(0, nl);
      this.#buf = this.#buf.slice(nl + 1);
      if (line.length === 0) continue;
      this.transcript.push({ dir: "<<", line });
      this.#onFrame(JSON.parse(line) as Frame);
    }
  }

  #onFrame(frame: Frame): void {
    this.#frames.shift()?.(frame);
    const requestId = frame.requestId as string | undefined;
    if (frame._tag === "Chunk" && requestId !== undefined) {
      this.#chunks.get(requestId)?.(frame.values as unknown[]);
      this.send({ _tag: "Ack", requestId });
    } else if (frame._tag === "Exit" && requestId !== undefined) {
      const exit = frame.exit as { _tag: string; value?: unknown; cause?: Array<{ _tag: string; error?: unknown; defect?: unknown }> };
      this.#chunks.delete(requestId);
      const resolve = this.#pending.get(requestId);
      this.#pending.delete(requestId);
      if (exit._tag === "Success") resolve?.({ ok: true, value: exit.value });
      else {
        const fail = exit.cause?.find((c) => c._tag === "Fail");
        const error = (fail?.error ?? { _tag: "Defect", code: "DEFECT", message: JSON.stringify(exit.cause), details: exit.cause }) as Extract<Exit, { ok: false }>["error"];
        resolve?.({ ok: false, error });
      }
    }
  }
}
