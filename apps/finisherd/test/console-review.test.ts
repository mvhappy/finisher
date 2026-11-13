// `finisherd review policy show|set` (5E plan Task 10, Decision 25): text and `--json`, a terminal
// and `Proceed? [y/N]` for a change, and `--auto-merge on` refused before anything is asked.
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { runConsole } from "../src/console/main.ts";
import type { Prompt } from "../src/console/prompt.ts";
import { AgentServer } from "./support/agent-server.ts";
import { person } from "./support/person.ts";
import { confirmed, ok } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;

beforeEach(async () => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-console-review-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
  await server.start();
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

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

async function project(): Promise<string> {
  const local = await server.local();
  return ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
}

describe("finisherd review policy", () => {
  test("show prints the default policy as text and as json", async () => {
    const id = await project();
    const text = await run(["review", "policy", "show", "--project", id]);
    expect(text.code, text.err).toBe(0);
    expect(text.out).toContain(`Review policy of project ${id}:`);
    expect(text.out).toContain("protected branches: main");
    expect(text.out).toContain("require review: off");
    expect(text.out).toContain("auto merge: off");
    const json = await run(["review", "policy", "show", "--project", id, "--json"]);
    expect(JSON.parse(json.out)).toEqual({ policy: { protected: ["main"], requireReview: false, autoMerge: { enabled: false } } });
  });

  test("set needs a terminal, asks y/N, and changes only what was named", async () => {
    const id = await project();
    const args = ["review", "policy", "set", "--project", id, "--require-review", "on"];
    // CONSOLE_NEEDS_TTY (exit 7), checked before anything connects or changes.
    const refused = await run(args);
    expect(refused.code).toBe(7);
    expect(refused.err).toContain("CONSOLE_NEEDS_TTY");
    // Anything but y cancels and nothing is written.
    const no = person("n");
    const cancelled = await run(args, no);
    expect(cancelled.err).toContain("CONFIRMATION_REQUIRED");
    expect(no.asked).toEqual(["Proceed? [y/N] "]);
    expect((JSON.parse((await run(["review", "policy", "show", "--project", id, "--json"])).out) as { policy: { requireReview: boolean } }).policy.requireReview).toBe(false);
    // y writes it; protected branches stay as they were.
    const yes = person();
    const done = await run([...args, "--json"], yes);
    expect(done.code, done.err).toBe(0);
    expect(yes.shown[0]).toContain(`for project ${id}`);
    expect(yes.shown[0]).toContain('turns "require review" on');
    expect(JSON.parse(done.out)).toMatchObject({
      policy: { protected: ["main"], requireReview: true, autoMerge: { enabled: false } },
      old: { requireReview: false },
    });
    const text = await run(["review", "policy", "show", "--project", id]);
    expect(text.out).toContain("require review: on");
    // --unprotect removes one; the audit records the change.
    const unprotect = await run(["review", "policy", "set", "--project", id, "--unprotect", "main"], person());
    expect(unprotect.code, unprotect.err).toBe(0);
    expect(unprotect.out).toContain("protected branches: none");
  });

  test("protect and unprotect by name, deltas only, unknown and both before the prompt", async () => {
    const id = await project();
    const local = await server.local();
    const mk = async (name: string) =>
      ok<{ branch: { id: string } }>(await local.call("branch.create", { projectId: id, from: "main", name, txId: `b-${name}`, author: "test" })).branch.id;
    const work = await mk("work");
    const other = await mk("other");
    // By name; the answer shows name (id).
    const p = person();
    const done = await run(["review", "policy", "set", "--project", id, "--protect", "work"], p);
    expect(done.code, done.err).toBe(0);
    expect(done.out).toContain(`work (${work})`);
    expect(p.shown[0]).toContain(`protects work (${work})`);
    const json = async () => JSON.parse((await run(["review", "policy", "show", "--project", id, "--json"])).out).policy.protected;
    expect(await json()).toEqual(["main", work]);
    // A concurrent protect between the read and the write is not lost.
    const racing: Prompt = {
      isInteractive: () => true,
      show: () => undefined,
      ask: async () => {
        ok(await confirmed(await server.console(), "review.policy", { projectId: id, protect: [other] }));
        return "y";
      },
    };
    const raced = await run(["review", "policy", "set", "--project", id, "--unprotect", "work"], racing);
    expect(raced.code, raced.err).toBe(0);
    expect(await json()).toEqual(["main", other]);
    expect((await run(["review", "policy", "show", "--project", id])).out).toContain(`other (${other})`);
    // Unknown, and the same branch in both: refused before anything is asked.
    for (const [argv, code, text] of [
      [["--unprotect", "nope"], 3, "BRANCH_NOT_FOUND"],
      [["--protect", "work", "--unprotect", "work"], 2, "USAGE"],
      [["--protect", "work", "--unprotect", work], 2, "USAGE"],
    ] as const) {
      const q = person();
      const r = await run(["review", "policy", "set", "--project", id, ...argv], q);
      expect(r.code, argv.join(" ")).toBe(code);
      expect(r.err).toContain(text);
      expect(q.asked).toEqual([]);
    }
    expect(await json()).toEqual(["main", other]);
  });

  test("--auto-merge on is refused before anything is asked", async () => {
    const id = await project();
    const p = person();
    const r = await run(["review", "policy", "set", "--project", id, "--auto-merge", "on"], p);
    expect(r.code).toBe(4);
    expect(r.err).toContain("AUTO_MERGE_UNAVAILABLE");
    expect(p.asked).toEqual([]);
    expect(p.shown).toEqual([]);
    // Not even the terminal check comes first.
    expect((await run(["review", "policy", "set", "--project", id, "--auto-merge", "on"])).err).toContain("AUTO_MERGE_UNAVAILABLE");
    // Off is allowed.
    const off = await run(["review", "policy", "set", "--project", id, "--auto-merge", "off"], person());
    expect(off.code, off.err).toBe(0);
  });

  test("usage errors", async () => {
    const id = await project();
    for (const argv of [
      ["review"],
      ["review", "policy"],
      ["review", "policy", "show"],
      ["review", "policy", "set", "--project", id],
      ["review", "policy", "set", "--project", id, "--require-review", "maybe"],
      ["review", "policy", "show", "--project", id, "--protect", "x"],
    ]) {
      const r = await run(argv, person());
      expect(r.code, argv.join(" ")).toBe(2);
      expect(r.err).toContain("USAGE");
    }
  });
});
