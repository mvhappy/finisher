// The console's connection (spec §4.7, plan 5A Decisions 29–30): it finds the running server
// through `<server-data>/finisherd.json`, connects to its socket (or pipe) and proves itself as
// `console` with `<server-data>/auth/console.key`, checking the server's proof first (a squatted
// or foreign socket is `SERVER_UNTRUSTED`). Nothing secret is printed: not the key, the nonces,
// the MACs or a ticket, nor the `console.challenge` code a confirmed call carries (`confirm.ts`).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { b64u, canonicalEndpoint, helloProof, nonce, proveMac, sameMac } from "../auth/crypto.ts";
import { authDir, KEY_FILES, readKeyFile } from "../auth/keys.ts";
import { infoPath } from "../config.ts";
import { type Exit, NdjsonClient } from "./ndjson.ts";

/** A server or console error, as printed. */
export interface Failure {
  readonly code: string;
  readonly message: string;
  readonly details?: unknown;
  readonly fix?: string | undefined;
}

/** An error the console prints and exits with (`fin`'s exit codes, `exit-codes.ts`). */
export class ConsoleError extends Error {
  readonly error: Failure;
  constructor(error: Failure) {
    super(`${error.code}: ${error.message}`);
    this.error = error;
  }
}

export const consoleError = (code: string, message: string, details: Record<string, unknown> = {}, fix?: string): ConsoleError =>
  new ConsoleError({ code, message, details, ...(fix === undefined ? {} : { fix }) });

const unavailable = (serverData: string, reason: string) =>
  consoleError(
    "SERVER_UNAVAILABLE",
    "the Finisher server is not running for this server data",
    { reason },
    `Start finisherd for ${serverData}, then try again.`,
  );

const untrusted = (reason: string) =>
  consoleError(
    "SERVER_UNTRUSTED",
    "the server at this socket did not prove it is this Finisher server",
    { reason },
    "Another program may be using the server's socket. Stop it, start finisherd again, then try again.",
  );

interface Info {
  readonly socket: string;
  readonly envId: string;
}

function readInfo(serverData: string): Info {
  let text: string;
  try {
    text = readFileSync(infoPath(serverData), "utf8");
  } catch {
    throw unavailable(serverData, "noServerInfo");
  }
  try {
    const info = JSON.parse(text) as Partial<Info>;
    if (typeof info.socket === "string" && typeof info.envId === "string") return { socket: info.socket, envId: info.envId };
  } catch {
    // below
  }
  throw unavailable(serverData, "badServerInfo");
}

/** The success value of `exit`, or a `ConsoleError` with the server's error. */
export function valueOf<A = unknown>(exit: Exit): A {
  if (exit.ok) return exit.value as A;
  throw new ConsoleError(exit.error);
}

/** A proved `console` connection. */
export interface ConsoleConnection {
  call(tag: string, payload: unknown): Promise<Exit>;
  /** A streaming call (`agent.watch`): each value to `onValue`; `stop` interrupts it. */
  stream(tag: string, payload: unknown, onValue: (v: unknown) => void): { done: Promise<Exit>; stop(): void };
  close(): Promise<void>;
}

/** Connects to the server of `serverData` and proves as `console`. */
export async function connectConsole(serverData: string): Promise<ConsoleConnection> {
  const info = readInfo(serverData);
  let key: Buffer;
  try {
    key = readKeyFile(join(authDir(serverData), KEY_FILES.console));
  } catch {
    throw consoleError(
      "UNAUTHENTICATED",
      "the server's console key cannot be read",
      { reason: "noCredential" },
      "Run this as the user who runs the Finisher server, on the same machine.",
    );
  }
  let client: NdjsonClient;
  try {
    client = await NdjsonClient.connect(info.socket);
  } catch {
    throw unavailable(serverData, "notListening");
  }
  try {
    const endpoint = canonicalEndpoint(info.socket);
    const clientNonce = nonce();
    const hello = await client.call("server.hello", {
      clientNonce: b64u.encode(clientNonce),
      env: info.envId,
      endpoint,
      credential: { kind: "console" },
    });
    if (!hello.ok) throw untrusted("helloFailed");
    const answer = hello.value as { envId?: string; endpoint?: string; serverNonce?: string; proof?: string };
    if (answer.envId !== info.envId) throw untrusted("envMismatch");
    if (answer.endpoint !== endpoint) throw untrusted("endpointMismatch");
    const serverNonce = answer.serverNonce === undefined ? null : b64u.decode(answer.serverNonce);
    const proof = answer.proof === undefined ? null : b64u.decode(answer.proof);
    if (serverNonce === null || proof === null) throw untrusted("noProof");
    if (!sameMac(proof, helloProof(key, info.envId, endpoint, clientNonce, serverNonce))) throw untrusted("badProof");
    const proved = await client.call("auth.prove", { mac: b64u.encode(proveMac(key, info.envId, endpoint, serverNonce, clientNonce)) });
    if (!proved.ok) throw untrusted("badProof");
  } catch (e) {
    await client.close();
    throw e;
  }
  return {
    call: (tag, payload) => client.call(tag, payload),
    stream: (tag, payload, onValue) => {
      const { id, done } = client.stream(tag, payload, onValue);
      return { done, stop: () => client.interrupt(id) };
    },
    close: () => client.close(),
  };
}
