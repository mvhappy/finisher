// A deliberately raw Effect-RPC client over `node:net` with NDJSON framing: the same
// frames the Rust `fin` client (Task 12) writes. Used by the tests and the fixture capture.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { connect, type Socket } from "node:net";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { b64u, canonicalEndpoint, helloProof, nonce, proveMac, sameMac } from "../../src/auth/crypto.ts";
import { authDir, readKeyFile } from "../../src/auth/keys.ts";

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

export type CredentialName = "local" | "console";

/** The key a test proves with: the server's `auth/<name>.key`. */
export function keyOf(serverData: string, name: CredentialName = "local"): Buffer {
  return readKeyFile(join(authDir(serverData), `${name}.key`));
}

/** `server.hello` with a fresh client nonce; checks nothing (tests look at the answer). */
export async function hello(
  client: NdjsonClient,
  fields: { env?: string; endpoint?: string; credential?: unknown } = {},
): Promise<{ clientNonce: Buffer; exit: Exit }> {
  const clientNonce = nonce();
  const exit = await client.call("server.hello", { clientNonce: b64u.encode(clientNonce), ...fields });
  return { clientNonce, exit };
}

/**
 * The whole handshake as `fin` does it (5A Decisions 6–9): hello, check the server's proof with
 * `key`, prove. Resolves with the `auth.prove` exit.
 */
export async function prove(client: NdjsonClient, info: ServerInfo, key: Buffer, credential: CredentialName = "local"): Promise<Exit> {
  const endpoint = canonicalEndpoint(info.socket);
  const { clientNonce, exit } = await hello(client, { env: info.envId, endpoint, credential: { kind: credential } });
  const answer = ok<{ envId: string; endpoint: string; serverNonce: string; proof?: string }>(exit);
  const serverNonce = b64u.decode(answer.serverNonce)!;
  const proof = answer.proof === undefined ? null : b64u.decode(answer.proof);
  if (proof === null || !sameMac(proof, helloProof(key, answer.envId, answer.endpoint, clientNonce, serverNonce))) {
    throw new Error("the server did not prove itself");
  }
  return client.call("auth.prove", { mac: b64u.encode(proveMac(key, answer.envId, answer.endpoint, serverNonce, clientNonce)) });
}

/** Connects to a running server's socket and proves `credential` (default `local`). */
export async function connectProved(server: { info: ServerInfo }, serverData: string, credential: CredentialName = "local"): Promise<NdjsonClient> {
  const client = await NdjsonClient.connect(server.info.socket);
  ok(await prove(client, server.info, keyOf(serverData, credential), credential));
  return client;
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
  diagnostics: { enabled: boolean };
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
