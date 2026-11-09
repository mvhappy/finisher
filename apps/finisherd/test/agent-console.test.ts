// The agent console commands (plan 7A Task 12): sessions from the console (`agent start|ls|show|
// prompt|end|watch|approve|delete|providers`), the OpenRouter key read without echo and never
// from the command line, the local endpoint, and the counsel gate. Commands run in-process
// against a real server through an injected `Prompt`; sessions run on the `fake` harness.
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Session } from "@finisher/contracts";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import type { FakeScript } from "../src/agent/fake.ts";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { AgentServer } from "./support/agent-server.ts";
import { type Person, person as confirming } from "./support/person.ts";
import { addAsset, commit, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;
const hooks = process.env.FIN_TEST_HOOKS;

const msg = (text: string) => ({ type: "message", role: "assistant", blocks: [{ type: "text", text }] });
/** Turn 1 answers; turn 2 reports more usage than any small budget holds. */
const SCRIPT: FakeScript = {
  name: "console",
  turns: [
    [{ emit: [msg("hello from the agent"), { type: "turnCompleted", stopReason: "endTurn" }] }],
    [{ emit: [{ type: "usage", input: 400, output: 400, cacheRead: 0, cacheWrite: 0, reasoning: 0 }, msg("spent"), { type: "turnCompleted", stopReason: "endTurn" }] }],
  ],
};

beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
});
afterAll(() => {
  if (hooks === undefined) delete process.env.FIN_TEST_HOOKS;
  else process.env.FIN_TEST_HOOKS = hooks;
});

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-console-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData, { fakeScripts: [SCRIPT] });
  await server.start();
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

/** A person at a terminal (`support/person.ts`) whose answer to the hidden key question is `secret`. */
function person(secret = "", ...answers: string[]): Person & { secrets: number } {
  const p = Object.assign(confirming(...answers), { secrets: 0 });
  p.secret = async () => {
    p.secrets++;
    return secret;
  };
  return p;
}

const NOT_A_TERMINAL: Prompt = { isInteractive: () => false, show: () => undefined, ask: async () => "" };

async function run(argv: string[], prompt: Prompt = NOT_A_TERMINAL): Promise<{ code: number; out: string; err: string }> {
  let out = "";
  let err = "";
  const code = await runConsole([...argv, "--server-data", server.serverData], {
    prompt,
    output: { out: (t) => void (out += `${t}\n`), err: (t) => void (err += `${t}\n`) },
  });
  return { code, out, err };
}

async function project(local: NdjsonClient): Promise<{ id: string; dir: string }> {
  const dir = join(tmp, "P");
  const id = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
  ok(await commit(local, id, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
  return { id, dir };
}

async function until(local: NdjsonClient, sessionId: string, status: Session["status"]): Promise<Session> {
  for (let i = 0; i < 200; i++) {
    const { session } = ok<{ session: Session }>(await local.call("agent.session.get", { sessionId }));
    if (session.status === status) return session;
    await new Promise((r) => setTimeout(r, 10));
  }
  throw new Error(`session ${sessionId} never became ${status}`);
}

describe("agent sessions from the console", () => {
  test("start by folder, ls, show, watch until the end, delete with the id typed back", async () => {
    const local = await server.local();
    const p = await project(local);
    const started = await run(["agent", "start", "--project", p.dir, "--harness", "fake", "--prompt", "say hello", "--json"]);
    expect(started.code, started.err).toBe(0);
    // The persistent warning goes to stderr; the answer to stdout.
    expect(started.err).toContain("SANDBOX_DISABLED");
    const { session } = JSON.parse(started.out) as { session: Session };
    expect(session.projectId).toBe(p.id);
    await until(local, session.id, "idle");
    const ls = await run(["agent", "ls"]);
    expect(ls.out).toContain(`${session.id}  idle  editor  draft  local/fake`);
    expect((await run(["agent", "ls", "--status", "ended"])).out).toContain("No agent sessions.");
    const show = await run(["agent", "show", session.id, "--nodes"]);
    expect(show.code, show.err).toBe(0);
    expect(show.out).toContain("userMessage  you: say hello");
    expect(show.out).toContain("assistantMessage  hello from the agent");
    // `agent watch` renders the nodes as lines and exits once the session ends.
    const watching = run(["agent", "watch", session.id]);
    await new Promise((r) => setTimeout(r, 100));
    expect((await run(["agent", "end", session.id])).out).toContain("(its branch stays)");
    const watched = await watching;
    expect(watched.code, watched.err).toBe(0);
    const lines = watched.out.trim().split("\n");
    expect(lines[0]).toMatch(/^#1 userMessage {2}you: say hello$/);
    expect(lines).toContain("-- ended (userEnded)");
    // Deleting is the owner's: a terminal and the session id's first 8 characters typed back.
    const refused = await run(["agent", "delete", session.id]);
    expect(refused.code).toBe(7);
    expect(refused.err).toContain("CONSOLE_NEEDS_TTY");
    expect((await run(["agent", "delete", session.id], person("", "y"))).err).toContain("CONFIRMATION_REQUIRED");
    const owner = person();
    const deleted = await run(["agent", "delete", session.id], owner);
    expect(deleted.code, deleted.err).toBe(0);
    expect(owner.shown[0]).toContain(`This deletes agent session ${session.id}`);
    expect(owner.shown).toContain(`To confirm, type the session id's first 8 characters: ${session.id.slice(0, 8)}`);
    expect((await run(["agent", "show", session.id])).code).toBe(3);
  });

  test("a budget approval is answered with --add, which resumes the session", async () => {
    const local = await server.local();
    const p = await project(local);
    const started = await run(["agent", "start", "--project", p.id, "--harness", "fake", "--budget-tokens", "500", "--prompt", "one", "--json"]);
    const { session } = JSON.parse(started.out) as { session: Session };
    await until(local, session.id, "idle");
    expect((await run(["agent", "prompt", session.id, "spend", "it"])).code).toBe(0);
    const stopped = await until(local, session.id, "interrupted");
    expect(stopped.statusReason).toBe("BUDGET_EXCEEDED");
    const shown = await run(["agent", "show", session.id]);
    const approval = /approval (\S+) {2}budget/.exec(shown.out)?.[1];
    expect(approval, shown.out).toBeDefined();
    expect((await run(["agent", "approve", approval!, "allow", "--add", "5k"])).code).toBe(7);
    const owner = person();
    const r = await run(["agent", "approve", approval!, "allow", "--add", "5k"], owner);
    expect(r.code, r.err).toBe(0);
    expect(owner.asked).toEqual(["Proceed? [y/N] "]);
    expect(owner.shown[0]).toContain("raising its tokens budget by 5k, and resumes the session");
    expect(r.out).toContain("(allow)");
    expect((await until(local, session.id, "idle")).status).toBe("idle");
  });

  test("a rule approval is answered with --remember, which adds a rule (7B Task 5)", async () => {
    const local = await server.local();
    const p = await project(local);
    const started = await run(["agent", "start", "--project", p.id, "--harness", "fake", "--prompt", "one", "--json"]);
    const { session } = JSON.parse(started.out) as { session: Session };
    await until(local, session.id, "idle");
    server.core.store.startRun(session.id, { trigger: "prompt" });
    const subject = { command: "checkpoint", rpc: "history.checkpoint", args: {}, branches: [], rule: "r1", layer: "local", text: "checkpoint {}" };
    const { approval } = server.core.store.openApproval({ sessionId: session.id, kind: "rule", subject, argsHash: "hmac-sha256:00", expiresAt: server.time.now() + 60_000 });
    expect((await run(["agent", "approve", approval.id, "deny", "--remember", "forever"], person())).code).toBe(2);
    const owner = person();
    const r = await run(["agent", "approve", approval.id, "deny", "--remember", "session"], owner);
    expect(r.code, r.err).toBe(0);
    expect(owner.shown[0]).toContain("It also adds a rule: deny checkpoint for this session.");
    expect(r.out).toContain("(deny)");
    const rule = /Rule (\S+) added \(local layer\)/.exec(r.out)?.[1];
    expect(rule, r.out).toBeDefined();
    expect(server.agents.ruleStore.get(rule!)).toMatchObject({ effect: "deny", command: "checkpoint", sessionId: session.id, source: `approval:${approval.id}` });
  });

  test("agent providers", async () => {
    const r = await run(["agent", "providers"]);
    expect(r.code, r.err).toBe(0);
    expect(r.out).toContain("builtin  ");
    expect(r.out).toContain("fake  available");
    expect(r.out).toContain("openrouter key: not set for the server");
    expect(r.out).toContain("local endpoint: not set");
  });
});

describe("provider keys, the local endpoint and the counsel gate from the console", () => {
  test("provider key set reads the key without echo and refuses one on the command line", async () => {
    const KEY = "sk-or-v1-consoletest0123456789";
    // On the command line: refused before anything is asked or sent.
    const argv = person(KEY);
    const inline = await run(["provider", "key", "set", "openrouter", KEY], argv);
    expect(inline.code).toBe(2);
    expect(inline.err).toContain("never from the command line");
    expect(argv.secrets).toBe(0);
    expect((await run(["provider", "key", "set", "openrouter"])).code).toBe(7);
    const anthropic = await run(["provider", "key", "set", "anthropic"], person(KEY));
    expect(anthropic.code).toBe(5);
    expect(anthropic.err).toContain("PROVIDER_UNAVAILABLE");
    const local = await run(["provider", "key", "set", "local"], person(KEY));
    expect(local.code).toBe(5);
    expect(local.err).toContain("finisherd provider local set");
    const owner = person(KEY);
    const set = await run(["provider", "key", "set", "openrouter", "--label", "team"], owner);
    expect(set.code, set.err).toBe(0);
    expect(owner.secrets).toBe(1);
    expect(owner.asked).toEqual(["Proceed? [y/N] "]);
    expect(set.out).toMatch(/Stored key \S+ \("team"\) as the server's default\./);
    const ls = await run(["provider", "key", "ls"]);
    expect(ls.out).toContain('"team"  server default');
    for (const r of [set, ls, await run(["provider", "key", "ls", "--json"]), await run(["agent", "providers", "--json"])]) {
      expect(r.out + r.err).not.toContain(KEY);
    }
    expect(owner.shown.join("\n")).not.toContain(KEY);
    expect((await run(["agent", "providers"])).out).toContain("openrouter key: set for the server");
    const id = /^(\S+)/.exec(ls.out)![1]!;
    // Removing takes the key id's first 8 characters typed back.
    expect((await run(["provider", "key", "rm", id], person("", "y"))).err).toContain("CONFIRMATION_REQUIRED");
    const removing = person();
    expect((await run(["provider", "key", "rm", id], removing)).out).toContain(`Removed key ${id}.`);
    expect(removing.asked).toEqual(["Type the key id's first 8 characters to confirm: "]);
  });

  test("provider key set refuses a terminal that cannot hide input instead of asking with an echo (L4)", async () => {
    const KEY = "sk-or-v1-noecho0123456789";
    let asked = 0;
    const shown: string[] = [];
    // Interactive, but only the echoing `ask`: no `secret`.
    const echoing: Prompt = {
      isInteractive: () => true,
      show: (text) => void shown.push(text),
      ask: async () => {
        asked++;
        return KEY;
      },
    };
    const r = await run(["provider", "key", "set", "openrouter"], echoing);
    expect(r.code).toBe(7);
    expect(r.err).toContain("CONSOLE_NEEDS_TTY");
    expect(asked).toBe(0);
    expect(r.out + r.err + shown.join("\n")).not.toContain(KEY);
    expect((await run(["provider", "key", "ls"])).out).not.toContain("server default");
  });

  test("provider local set, show, test and rm", async () => {
    const models = createServer((req, res) => {
      if (req.url === "/v1/models") res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify({ data: [{ id: "qwen3:8b" }] }));
      else res.writeHead(404).end();
    });
    await new Promise<void>((r) => models.listen(0, "127.0.0.1", r));
    const url = `http://127.0.0.1:${(models.address() as AddressInfo).port}/v1`;
    try {
      expect((await run(["provider", "local", "set", url])).code).toBe(7);
      const bad = await run(["provider", "local", "set", "ftp://127.0.0.1/v1"], person());
      expect(bad.code).toBe(5);
      const set = await run(["provider", "local", "set", url], person());
      expect(set.code, set.err).toBe(0);
      expect((await run(["provider", "local", "show"])).out.trim()).toBe(`custom  ${url}`);
      const t = await run(["provider", "local", "test"]);
      expect(t.code, t.err).toBe(0);
      expect(t.out).toContain("Models: qwen3:8b");
      expect((await run(["agent", "providers"])).out).toContain("local endpoint: custom (answers)");
    } finally {
      await new Promise((r) => models.close(r));
    }
    const down = await run(["provider", "local", "test"]);
    expect(down.code).toBe(5);
    expect(down.err).not.toContain(new URL(url).port);
    const removing = person();
    expect((await run(["provider", "local", "rm"], removing)).out).toContain("Removed the local endpoint.");
    expect(removing.shown).toContain("To confirm, type the word: local");
    expect((await run(["provider", "local", "show"])).out).toContain("No local endpoint is set.");
  });

  test("training closed-targets shows the gate and records it with a y", async () => {
    expect((await run(["training", "closed-targets"])).out).toMatch(/^pendingCounsel: /);
    expect((await run(["training", "closed-targets", "cleared"])).code).toBe(7);
    expect((await run(["training", "closed-targets", "maybe"], person())).code).toBe(2);
    const owner = person();
    const r = await run(["training", "closed-targets", "excluded"], owner);
    expect(r.code, r.err).toBe(0);
    expect(owner.shown[0]).toContain("It is audited.");
    expect(owner.asked).toEqual(["Proceed? [y/N] "]);
    expect((await run(["training", "closed-targets", "cleared"], person("", "n"))).err).toContain("CONFIRMATION_REQUIRED");
    expect((await run(["training", "closed-targets", "--json"])).out.trim()).toBe(JSON.stringify({ state: "excluded" }));
    const settings = JSON.parse(readFileSync(join(server.serverData, "settings.json"), "utf8")) as { training: unknown };
    expect(settings.training).toEqual({ closedModelTargets: "excluded" });
  });
});
