// Diagnostics from principals (plan 5A Task 7; spec §3.4, §4.6): the journal's source and a
// call's actor come from the connection's principal; consent changes, purges, `project default`
// and content exports are a person's; training on is a device's alone; the rate limit is per
// principal; a non-human export is redacted.
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { type FinError, FinRpcs, type Principal, SCOPES } from "@finisher/contracts";
import type { ConsentState, Envelope, ExportPage } from "@finisher/core-types";
import { rpcMethods } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { withChain } from "../src/chain/chain.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { settingsPath } from "../src/config.ts";
import { Connections } from "../src/connections.ts";
import { actorFor, LOCAL_RECORD_RATE, makeDiagnostics, RECORD_RATE, sourceOf } from "../src/diagnostics.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { confirmed, connectProved, type Exit as RpcExit, NdjsonClient, ok, type RunningServer, startServer } from "./support/ndjson-client.ts";
import { connectWs } from "./support/ws-client.ts";

let tmp: string;
let serverData: string;
let server: RunningServer | undefined;
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-diag-p-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

function env(extra: Record<string, string> = {}): NodeJS.ProcessEnv {
  const { DO_NOT_TRACK: _d, FIN_DIAGNOSTICS: _f, FIN_TRAINING: _t, ...rest } = process.env;
  return { ...rest, ...extra };
}

async function start(extra: Record<string, string> = {}): Promise<RunningServer> {
  server = await startServer(serverData, 15_000, env(extra));
  return server;
}

async function client(credential: "local" | "console" = "local"): Promise<NdjsonClient> {
  const c = await connectProved(server!, credential);
  clients.push(c);
  return c;
}

const errorCode = (e: RpcExit): string | undefined => (e.ok ? undefined : e.error.code);
const wsCode = (e: Record<string, unknown>): string | undefined => {
  const exit = e.exit as { _tag: string; cause?: Array<{ error?: { code?: string } }> };
  return exit._tag === "Success" ? undefined : exit.cause?.find((c) => c.error !== undefined)?.error?.code;
};

const cli = (marker = "m") => ({
  name: "item.trim",
  flags: ["project"],
  argsHash: `blake3:${"cd".repeat(32)}`,
  micros: 900,
  exit: 0,
  online: true,
  dryRun: false,
  txIds: [`idem-${marker}`],
});
const panic = { where: "fin", thirdParty: false, thread: "main", message: "Error: a secret caption" };

const agent = (id = "01JC000000000000000000AGNT"): Principal => ({
  kind: "agent",
  id,
  scopes: ["server.read", "diagnostics.read", "diagnostics.record"],
  projects: "*",
  branches: "*",
  human: false,
});
const mcp = (id = "01JC0000000000000000000MCP"): Principal => ({ ...agent(id), kind: "mcp" });
const device: Principal = { kind: "device", id: "01device00000000000000000a", scopes: [...SCOPES], projects: "*", branches: "*", human: true };

async function exported(c: NdjsonClient, filter: object = {}, more: object = {}): Promise<ExportPage> {
  return ok<ExportPage>(await c.call("diagnostics.export", { filter, ...more }));
}

/** Runs `f` against an in-process `Diagnostics` with a frozen rate-limit clock. */
function inProcess<A>(f: (d: Effect.Success<ReturnType<typeof makeDiagnostics>>) => Promise<A>): Promise<A> {
  return Effect.runPromise(Effect.scoped(Effect.flatMap(makeDiagnostics(serverData, { settingsPollMs: 20, now: () => 0 }), (d) => Effect.promise(() => f(d)))));
}

describe("sources and actors", () => {
  test("source_follows_principal", async () => {
    expect([LOCAL, agent(), mcp(), CONSOLE, device].map(sourceOf)).toEqual(["fin", "fin", "mcp", "console", "ui"]);
    expect(sourceOf({ ...LOCAL, kind: "anonymous" })).toBeUndefined();
    await start();
    const c = await client();
    ok(await c.call("diagnostics.record", { kind: "cli.command", kv: 1, payload: cli("socket") }));
    // A WebSocket opened with a `local` ticket is `fin`, never `ui` (spec §3.4).
    const ws = await connectWs(server!);
    try {
      expect(wsCode(await ws.call("diagnostics.record", { kind: "cli.command", kv: 1, payload: cli("ws") }))).toBeUndefined();
    } finally {
      ws.close();
    }
    // The console sends nothing of its own.
    const person = await client("console");
    expect(errorCode(await person.call("diagnostics.record", { kind: "cli.command", kv: 1, payload: cli("console") }))).toBe("EVENT_INVALID");
    const lines = (await exported(c, { kinds: ["cli.command"] })).lines;
    expect(lines.map((e) => e.source)).toEqual(["fin", "fin"]);
    // One run per connection, across transports.
    expect(new Set(lines.map((e) => e.session)).size).toBe(2);
    // An MCP adapter's events are `mcp` (no credential exists before Task 9: in process).
    await server!.stop();
    server = undefined;
    await inProcess(async (d) => {
      const from = { principal: mcp(), connection: "socket:1" };
      expect(await Effect.runPromise(d.record(from, { kind: "cli.command", kv: 1, payload: cli("mcp") }, {}))).toEqual({ recorded: true });
      const page = JSON.parse(await d.journal.export(JSON.stringify({ kinds: ["cli.command"] }), null, 10, null, false)) as ExportPage;
      expect(page.lines.at(-1)!.source).toBe("mcp");
      const refused = await Effect.runPromiseExit(d.record({ principal: CONSOLE, connection: "socket:2" }, { kind: "cli.command", kv: 1, payload: cli() }, {}));
      expect(Exit.isFailure(refused)).toBe(true);
    });
  });

  test("agent_actor_is_its_session", async () => {
    const claims = { "fin-actor-kind": "user", "fin-actor-session": "01JC00000000000000000000AA", "fin-actor-call": "tool-call-7" };
    expect(actorFor(agent(), claims)).toEqual({ actor: { kind: "agent", session: "01JC000000000000000000AGNT", call: "tool-call-7" } });
    expect(actorFor(mcp(), claims)).toEqual({ actor: { kind: "agent", session: "01JC0000000000000000000MCP", call: "tool-call-7" } });
    expect(actorFor(agent(), {})).toEqual({ actor: { kind: "agent", session: "01JC000000000000000000AGNT" } });
    // A person: the console is the user; a device is its own session.
    expect(actorFor(CONSOLE, claims)).toEqual({ actor: { kind: "user" } });
    expect(actorFor(device, claims)).toEqual({ actor: { kind: "user", session: device.id } });
    await inProcess(async (d) => {
      await Effect.runPromise(d.record({ principal: agent(), connection: "socket:1" }, { kind: "cli.command", kv: 1, payload: cli() }, claims));
      const page = JSON.parse(await d.journal.export(JSON.stringify({ kinds: ["cli.command"] }), null, 10, null, false)) as ExportPage;
      expect(page.lines.at(-1)!.ctx?.actor).toMatchObject({ kind: "agent", session: "01JC000000000000000000AGNT" });
    });
  });

  test("local_actor_keeps_headers", () => {
    const claims = { "fin-actor-kind": "agent", "fin-actor-session": "s1", "fin-actor-call": "c1" };
    expect(actorFor(LOCAL, claims)).toEqual({ actor: { kind: "agent", session: "s1", call: "c1" } });
    expect(actorFor(LOCAL, { "fin-actor-kind": "system" })).toEqual({ actor: { kind: "unknown" } });
  });

  test("rate_limit_is_per_principal", async () => {
    await inProcess(async (d) => {
      const spend = async (principal: Principal, n: number, connection = "socket:1") => {
        let recorded = 0;
        for (let i = 0; i < n; i++) {
          const r = await Effect.runPromise(d.record({ principal, connection }, { kind: "cli.command", kv: 1, payload: cli(String(i)) }, {}));
          if (r.recorded) recorded++;
        }
        return recorded;
      };
      // The clock is frozen: each principal has its own burst, shared by its connections.
      expect(await spend(mcp("01JC0000000000000000000MCA"), RECORD_RATE.burst + 10)).toBe(RECORD_RATE.burst);
      expect(await spend(mcp("01JC0000000000000000000MCA"), 5, "socket:2")).toBe(0);
      expect(await spend(mcp("01JC0000000000000000000MCB"), 5)).toBe(5);
      expect(await spend(agent(), 5)).toBe(5);
      expect(d.rateLimited()).toBe(15);
    });
  });

  test("local is limited per connection, under a cap for local as a whole (review of Task 7, M1)", async () => {
    await inProcess(async (d) => {
      const spend = async (connection: string, n: number) => {
        let recorded = 0;
        for (let i = 0; i < n; i++) {
          const r = await Effect.runPromise(d.record({ principal: LOCAL, connection }, { kind: "cli.command", kv: 1, payload: cli(String(i)) }, {}));
          if (r.recorded) recorded++;
        }
        return recorded;
      };
      // One looping `fin` spends its own burst; another `fin` still records.
      expect(await spend("socket:1", RECORD_RATE.burst + 10)).toBe(RECORD_RATE.burst);
      expect(await spend("socket:1", 5)).toBe(0);
      expect(await spend("socket:2", 5)).toBe(5);
      // Every `local` connection together stops at LOCAL_RECORD_RATE's burst.
      let total = RECORD_RATE.burst + 5;
      for (let c = 3; c < 20; c++) total += await spend(`socket:${c}`, RECORD_RATE.burst);
      expect(total).toBe(LOCAL_RECORD_RATE.burst);
    });
  });

  test("rpc_methods_table_matches_contracts", () => {
    expect(rpcMethods().filter((m) => m !== "other").sort()).toEqual([...FinRpcs.requests.keys()].sort());
  });
});

describe("human-only diagnostics", () => {
  test("training_on_refused_for_every_non_device", async () => {
    // Through the chain, for every principal kind: only a device reaches the handler.
    const reached: string[] = [];
    const projects: ChainProjects = { isOpen: () => false, dirOf: () => undefined, resolveBranch: () => Effect.die("unused"), branches: () => Effect.succeed([]) };
    for (const p of [LOCAL, agent(), mcp(), CONSOLE, device]) {
      const connections = new Connections("socket", () => "/sd/run/fin.sock");
      connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} }, p);
      const chained = withChain(
        { "diagnostics.consent": (_: unknown) => Effect.sync(() => void reached.push(p.kind)) },
        { connections, projects },
      ) as unknown as Record<string, (payload: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
      const exit = await Effect.runPromiseExit(chained["diagnostics.consent"]!({ purpose: "training", value: "on" }, { client: { id: 1 }, headers: {} }));
      const code = Exit.isFailure(exit) ? (Exit.findErrorOption(exit) as { value?: FinError }).value?.code : undefined;
      expect(code, p.kind).toBe(p.kind === "device" ? undefined : "CONSENT_UI_ONLY");
    }
    expect(reached).toEqual(["device"]);
    // And against a server: `fin`'s socket, a `local` ticket's WebSocket, the console (even with a code).
    await start();
    const c = await client();
    const refused = await c.call("diagnostics.consent", { purpose: "training", value: "on" });
    expect(errorCode(refused)).toBe("CONSENT_UI_ONLY");
    if (!refused.ok) {
      expect(refused.error.fix).toMatch(/Finisher app.*settings file.*FIN_TRAINING/);
      expect(refused.error.fix).not.toMatch(/`|\bfinisherd [a-z-]+|\bfin [a-z-]+/);
    }
    const ws = await connectWs(server!);
    try {
      expect(wsCode(await ws.call("diagnostics.consent", { purpose: "training", value: "on" }))).toBe("CONSENT_UI_ONLY");
    } finally {
      ws.close();
    }
    const person = await client("console");
    expect(errorCode(await confirmed(person, "diagnostics.consent", { purpose: "training", value: "on" }))).toBe("CONSENT_UI_ONLY");
    expect(existsSync(settingsPath(serverData))).toBe(false);
  });

  test("consent_off_and_unset_need_a_human", async () => {
    await start();
    const c = await client();
    for (const [purpose, value] of [["diagnostics", "off"], ["diagnostics", "unset"], ["diagnostics", "on"], ["training", "off"], ["training", "unset"]]) {
      const refused = await c.call("diagnostics.consent", { purpose, value });
      expect(errorCode(refused), `${purpose} ${value}`).toBe("HUMAN_ONLY");
      if (!refused.ok) expect(refused.error.details).toMatchObject({ scope: "diagnostics.admin", where: "console" });
    }
    // `local` cannot ask for a code either.
    expect(errorCode(await c.call("console.challenge", {}))).toBe("HUMAN_ONLY");
    expect(existsSync(settingsPath(serverData))).toBe(false);
    // The console: no code, a wrong code, another connection's code, an expired one: refused.
    const person = await client("console");
    const other = await client("console");
    expect(errorCode(await person.call("diagnostics.consent", { purpose: "training", value: "off" }))).toBe("CONFIRMATION_REQUIRED");
    const { code } = ok<{ code: string; expiresAt: number }>(await person.call("console.challenge", {}));
    expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{6}$/);
    expect(errorCode(await other.call("diagnostics.consent", { purpose: "training", value: "off", challenge: code }))).toBe("CONFIRMATION_REQUIRED");
    const wrong = code === "000000" ? "111111" : "000000";
    expect(errorCode(await person.call("diagnostics.consent", { purpose: "training", value: "off", challenge: wrong }))).toBe("CONFIRMATION_REQUIRED");
    // A wrong answer voided the code.
    expect(errorCode(await person.call("diagnostics.consent", { purpose: "training", value: "off", challenge: code }))).toBe("CONFIRMATION_REQUIRED");
    expect(existsSync(settingsPath(serverData))).toBe(false);
    // With a fresh code, typed back in lower case: it applies, once.
    const fresh = ok<{ code: string }>(await person.call("console.challenge", {})).code;
    const consent = ok<ConsentState>(await person.call("diagnostics.consent", { purpose: "training", value: "off", challenge: fresh.toLowerCase() }));
    expect(consent.training).toBe("off");
    expect(JSON.parse(readFileSync(settingsPath(serverData), "utf8")).privacy).toEqual({ training: "off" });
    expect(errorCode(await person.call("diagnostics.consent", { purpose: "training", value: "unset", challenge: fresh }))).toBe("CONFIRMATION_REQUIRED");
    expect(ok<ConsentState>(await confirmed(person, "diagnostics.consent", { purpose: "training", value: "unset" })).training).toBe("undecided");
    // A console ticket's WebSocket confirms on its own connection too.
    const ws = await connectWs(server!, "console");
    try {
      expect(wsCode(await ws.confirmed("diagnostics.consent", { purpose: "diagnostics", value: "off" }))).toBeUndefined();
    } finally {
      ws.close();
    }
  });

  test("purge_project_default_and_content_export_need_a_human, project_never_open_to_writers", async () => {
    await start();
    const c = await client();
    const projectId = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    ok(await c.call("diagnostics.record", { kind: "cli.command", kv: 1, ctx: { project: projectId }, payload: cli() }));
    for (const selector of ["all", { project: projectId }, { before: Date.now() }]) {
      expect(errorCode(await c.call("diagnostics.purge", { selector })), JSON.stringify(selector)).toBe("HUMAN_ONLY");
    }
    expect(errorCode(await c.call("diagnostics.export", { content: true }))).toBe("HUMAN_ONLY");
    // Setting `never` stays open to writers; clearing it is a person's.
    expect(ok(await c.call("diagnostics.project", { projectId, collect: "never" }))).toEqual({ project: projectId, collect: "never" });
    expect(errorCode(await c.call("diagnostics.project", { projectId, collect: "default" }))).toBe("HUMAN_ONLY");
    expect(JSON.parse(readFileSync(join(tmp, "P", "finisher.project.json"), "utf8")).collect).toBe("never");
    const person = await client("console");
    expect(errorCode(await person.call("diagnostics.project", { projectId, collect: "default" }))).toBe("CONFIRMATION_REQUIRED");
    ok(await confirmed(person, "diagnostics.project", { projectId, collect: "default" }));
    expect(JSON.parse(readFileSync(join(tmp, "P", "finisher.project.json"), "utf8")).collect).toBeUndefined();
    // Exporting is not a change: the console needs no code for content.
    expect((await exported(person, {}, { content: true })).redacted).toBeUndefined();
    expect(errorCode(await person.call("diagnostics.purge", { selector: "all" }))).toBe("CONFIRMATION_REQUIRED");
    expect(ok<{ removed: number }>(await confirmed(person, "diagnostics.purge", { selector: "all" })).removed).toBeGreaterThan(0);
  });

  test("non_human_export_is_redacted", async () => {
    await start({ FIN_TRAINING: "on" });
    const c = await client();
    ok(await c.call("diagnostics.record", { kind: "error.panic", kv: 1, payload: panic }));
    const find = (page: ExportPage): Envelope => page.lines.find((e) => e.kind === "error.panic")!;
    // `fin` (and every principal that is not a person): content removed, and said so.
    const redacted = await exported(c, { kinds: ["error.panic"] });
    expect(redacted.redacted).toBe(true);
    expect(find(redacted).payload).not.toHaveProperty("message");
    expect(find(redacted).payload).toHaveProperty("messageHash");
    expect(JSON.stringify(redacted)).not.toContain("secret caption");
    // A person sees what was stored, but only when asking for content (owner decision, review
    // of Task 7, L4): an export without `content: true` is redacted for a person too.
    const person = await client("console");
    const plain = await exported(person, { kinds: ["error.panic"] });
    expect(plain.redacted).toBe(true);
    expect(JSON.stringify(plain)).not.toContain("secret caption");
    const full = await exported(person, { kinds: ["error.panic"] }, { content: true });
    expect(full.redacted).toBeUndefined();
    expect(JSON.stringify(find(full).payload)).toContain("secret caption");
  });
});
