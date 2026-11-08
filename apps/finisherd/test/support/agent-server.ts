// A server in this process for the agent session tests (7A Task 7): its clock is the test's,
// its agent core is handed over, and sessions are opened through `openSession` with the root
// branch created over RPC as the `local` starter (what the runtime does through the registry).
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { CredentialFile, Mode } from "@finisher/contracts";
import type { BranchView } from "@finisher/core-types";
import { Effect, Fiber } from "effect";
import { expect } from "vitest";
import { b64u, canonicalEndpoint, helloProof, proveMac, sameMac } from "../../src/auth/crypto.ts";
import type { AgentCore } from "../../src/agent/core.ts";
import { openSession, type SessionBranchOps } from "../../src/agent/credentials.ts";
import { toFinError } from "../../src/errors.ts";
import { serve, StderrLogger } from "../../src/main.ts";
import { LOCAL } from "../../src/principal.ts";
import { connectProved, type Exit, hello, NdjsonClient, ok, type ServerInfo } from "./ndjson-client.ts";

export function testClock(start = Date.now()) {
  let now = start;
  return { now: () => now, advance: (ms: number) => (now += ms) };
}

export class AgentServer {
  readonly clients: NdjsonClient[] = [];
  readonly time = testClock();
  #running: { fiber: Fiber.Fiber<unknown, unknown>; info: ServerInfo; core: AgentCore } | undefined;

  readonly serverData: string;

  constructor(serverData: string) {
    this.serverData = serverData;
  }

  get info(): ServerInfo {
    return this.#running!.info;
  }

  get core(): AgentCore {
    return this.#running!.core;
  }

  async start(): Promise<void> {
    let ready!: (i: ServerInfo) => void;
    const info = new Promise<ServerInfo>((r) => (ready = r));
    let core: AgentCore | undefined;
    const fiber = Effect.runFork(
      serve({
        serverData: this.serverData,
        credentials: { clock: this.time.now, sweepMs: 20 },
        onAgentCore: (c) => (core = c),
        onReady: (i) => ready(i as ServerInfo),
      }).pipe(Effect.provide(StderrLogger)),
    );
    const got = await info;
    this.#running = { fiber, info: got, core: core! };
  }

  async stop(): Promise<void> {
    for (const c of this.clients.splice(0)) await c.close();
    if (this.#running === undefined) return;
    await Effect.runPromise(Fiber.interrupt(this.#running.fiber));
    this.#running = undefined;
  }

  async local(): Promise<NdjsonClient> {
    const c = await connectProved({ info: this.info, serverData: this.serverData });
    this.clients.push(c);
    return c;
  }

  /** Connects and proves `file` as `fin` does: the `auth.prove` exit and whether the server proved itself. */
  async proveAs(file: CredentialFile): Promise<{ client: NdjsonClient; exit: Exit; proved: boolean }> {
    const client = await NdjsonClient.connect(this.info.socket);
    this.clients.push(client);
    const key = b64u.decode(file.key)!;
    const endpoint = canonicalEndpoint(this.info.socket);
    const { clientNonce, exit } = await hello(client, { env: file.env, endpoint, credential: { kind: file.kind, id: file.id } });
    const answer = ok<{ envId: string; endpoint: string; serverNonce: string; proof?: string }>(exit);
    const serverNonce = b64u.decode(answer.serverNonce)!;
    const proof = answer.proof === undefined ? null : b64u.decode(answer.proof);
    const proved = proof !== null && sameMac(proof, helloProof(key, answer.envId, answer.endpoint, clientNonce, serverNonce));
    const mac = proveMac(key, answer.envId, answer.endpoint, serverNonce, clientNonce);
    return { client, exit: await client.call("auth.prove", { mac: b64u.encode(mac) }), proved };
  }

  /** A client proved with the credential file at `path`. */
  async agent(path: string): Promise<NdjsonClient> {
    const { client, exit, proved } = await this.proveAs(readCredential(path));
    expect(proved).toBe(true);
    ok(exit);
    return client;
  }
}

export function readCredential(path: string): CredentialFile {
  return JSON.parse(readFileSync(path, "utf8")) as CredentialFile;
}

/** `SessionBranchOps` over RPC, as `c` (the starter). */
export function rpcBranchOps(c: NdjsonClient): SessionBranchOps {
  const call = <A>(tag: string, payload: unknown) =>
    Effect.flatMap(Effect.promise(() => c.call(tag, payload)), (e) => (e.ok ? Effect.succeed(e.value as A) : Effect.fail(toFinError(Object.assign(new Error(e.error.message), { ...e.error, finNode: true })))));
  const branches = (projectId: string) => Effect.map(call<{ branches: BranchView[] }>("branch.list", { projectId, all: true }), (r) => r.branches);
  return {
    branches,
    resolveBranch: (projectId, ref) =>
      Effect.flatMap(branches(projectId), (bs) => {
        const b = bs.find((x) => x.id === ref || x.name === ref || x.display === ref);
        return b === undefined ? Effect.fail(toFinError(Object.assign(new Error("no branch"), { code: "BRANCH_NOT_FOUND", finNode: true }))) : Effect.succeed(b.id);
      }),
    createBranch: (projectId, req) => Effect.map(call<{ branch: BranchView }>("branch.create", { projectId, ...req }), (r) => r.branch),
  };
}

export interface OpenOptions {
  readonly mode?: Mode;
  readonly role?: string;
  readonly base?: string;
  readonly root?: string;
}

/** Opens a session on `projectId` as `local` (Decision 15 steps 2–5). */
export async function open(core: AgentCore, starter: NdjsonClient, projectId: string, o: OpenOptions = {}) {
  return Effect.runPromise(
    openSession(
      { creds: core.credentials, store: core.store, ops: rpcBranchOps(starter), ids: core.ids },
      {
        projectId,
        harness: "builtin",
        provider: "openrouter",
        model: "anthropic/claude-sonnet-5.5",
        weights: "closed",
        role: o.role ?? "editor",
        mode: o.mode ?? "draft",
        budget: {},
        createdBy: { principal: { kind: "local", id: "local" } },
        ...(o.base === undefined ? {} : { base: o.base }),
        ...(o.root === undefined ? {} : { root: o.root }),
      },
      LOCAL,
    ),
  );
}

/** The `code` of a failed exit, `undefined` for a success. */
export const code = (e: Exit) => (e.ok ? undefined : e.error.code);

export const projectDir = (tmp: string, name: string) => join(tmp, name);
