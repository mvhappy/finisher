// A deliberately raw Effect-RPC client over `node:net` with NDJSON framing: the same
// frames the Rust `fin` client (Task 12) writes. Used by the tests and the fixture capture.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { connect, type Socket } from "node:net";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

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
  }

  static connect(path: string): Promise<NdjsonClient> {
    return new Promise((resolve, reject) => {
      const socket = connect({ path });
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

  call(tag: string, payload: unknown): Promise<Exit> {
    const id = String(this.#nextId++);
    const done = new Promise<Exit>((resolve) => this.#pending.set(id, resolve));
    this.send({ _tag: "Request", id, tag, payload, headers: [] });
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

  interrupt(id: string): void {
    this.send({ _tag: "Interrupt", requestId: id });
  }

  close(): Promise<void> {
    return new Promise((resolve) => {
      this.#socket.once("close", () => resolve());
      this.#socket.end();
    });
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

/** Unwraps a successful exit or throws with the error frame. */
export function ok<A = unknown>(exit: Exit): A {
  if (!exit.ok) throw new Error(`expected success, got ${JSON.stringify(exit.error)}`);
  return exit.value as A;
}

export interface ServerInfo {
  pid: number;
  wsPort: number;
  socket: string;
  token: string;
  protocolVersion: number;
  envId: string;
}

export interface RunningServer {
  readonly info: ServerInfo;
  readonly child: ChildProcess;
  readonly stderr: () => string;
  /** Sends SIGTERM and resolves with the exit code. */
  stop(): Promise<number | null>;
}

const MAIN = fileURLToPath(new URL("../../src/main.ts", import.meta.url));

/** Spawns `node src/main.ts --server-data <dir>` (with `env`) and waits for its `finisherd.json`. */
export async function startServer(
  serverData: string,
  timeoutMs = 15_000,
  env: NodeJS.ProcessEnv = process.env,
): Promise<RunningServer> {
  const child = spawn(process.execPath, [MAIN, "--server-data", serverData], { stdio: ["ignore", "pipe", "pipe"], env });
  let stderr = "";
  let stdout = "";
  child.stderr.setEncoding("utf8").on("data", (d: string) => (stderr += d));
  child.stdout.setEncoding("utf8").on("data", (d: string) => (stdout += d));
  const exited = new Promise<number | null>((resolve) => child.once("exit", (code) => resolve(code)));
  const file = join(serverData, "finisherd.json");
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (child.exitCode !== null) throw new Error(`finisherd exited ${child.exitCode}: ${stderr}`);
    if (existsSync(file)) {
      try {
        const info = JSON.parse(readFileSync(file, "utf8")) as ServerInfo;
        if (info.pid === child.pid) {
          return {
            info,
            child,
            stderr: () => stderr,
            stop: async () => {
              child.kill("SIGTERM");
              const code = await exited;
              if (stdout.length > 0) throw new Error(`finisherd wrote to stdout: ${stdout}`);
              return code;
            },
          };
        }
      } catch {
        // partially written; retry
      }
    }
    if (Date.now() > deadline) {
      child.kill("SIGKILL");
      throw new Error(`finisherd did not become ready: ${stderr}`);
    }
    await new Promise((r) => setTimeout(r, 25));
  }
}

export function tx(txId: string, baseRev: number, ops: unknown[]): Record<string, unknown> {
  return { txId, author: "test", branch: "main", baseRev, schemaVersion: 2, ops };
}

export function addAsset(placeholder = "$a"): Record<string, unknown> {
  return {
    op: "asset.add",
    id: placeholder,
    path: "/media/a.mov",
    duration: "600",
    video: { width: 1920, height: 1080, rate: "30" },
    audio: null,
    fingerprint: "fp",
  };
}

export function insert(at: number, duration = 1): Record<string, unknown> {
  return { op: "clip.insert", id: "$c", track: "t1", at: String(at), asset: "a1", sourceIn: "0", duration: String(duration) };
}
