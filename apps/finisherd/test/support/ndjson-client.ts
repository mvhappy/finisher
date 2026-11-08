// Test helpers over the shared NDJSON client (`src/console/ndjson.ts`): the handshake, a server
// started as a child process, and payload builders. Used by the tests and the fixture capture.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { b64u, canonicalEndpoint, helloProof, nonce, proveMac, sameMac } from "../../src/auth/crypto.ts";
import { authDir, readKeyFile } from "../../src/auth/keys.ts";

export { type Exit, type Frame, NdjsonClient, type Transcript } from "../../src/console/ndjson.ts";
import { type Exit, NdjsonClient } from "../../src/console/ndjson.ts";

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

/**
 * Connects to a running server's socket and proves `credential` (default `local`): since 5A
 * Task 4 an unproved connection may only say hello. The handshake's frames are left out of the
 * transcript (the RPC fixtures record what comes after).
 */
export async function connectProved(server: { info: ServerInfo; serverData: string }, credential: CredentialName = "local"): Promise<NdjsonClient> {
  const client = await NdjsonClient.connect(server.info.socket);
  ok(await prove(client, server.info, keyOf(server.serverData, credential), credential));
  client.transcript.length = 0;
  client.restartIds();
  return client;
}

/**
 * A mutating human-only call as the console makes it (5A Decision 30): asks this connection for
 * a `console.challenge` and sends `tag` with its code typed back (`challenge`).
 */
export async function confirmed(c: { call(tag: string, payload: unknown): Promise<Exit> }, tag: string, payload: Record<string, unknown>): Promise<Exit> {
  const { code } = ok<{ code: string }>(await c.call("console.challenge", {}));
  return c.call(tag, { ...payload, challenge: code });
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
  protocolVersion: number;
  envId: string;
  diagnostics: { enabled: boolean };
}

export interface RunningServer {
  readonly info: ServerInfo;
  readonly serverData: string;
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
            serverData,
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

/** The generation `branch` is read at now (`snapshot.get`): protocol 2 requires it on writes. */
export async function generationOf(c: NdjsonClient, projectId: string, branch = "main"): Promise<{ incarnation: string; n: number }> {
  return ok<{ generation: { incarnation: string; n: number } }>(await c.call("snapshot.get", { projectId, branch })).generation;
}

/** `tx.commit` of `t` at its branch's current generation. */
export async function commit(c: NdjsonClient, projectId: string, t: Record<string, unknown>): Promise<Exit> {
  const generation = await generationOf(c, projectId, typeof t.branch === "string" ? t.branch : "main");
  return c.call("tx.commit", { projectId, tx: t, generation });
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
