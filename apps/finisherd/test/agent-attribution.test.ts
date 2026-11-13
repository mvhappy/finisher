// Inferred attribution of CLI shell calls (plan 7D Task 12, Decision 20; spec §12.3): a shell
// call's window opens at the runtime's allow and closes at its tool result; a session's commit
// with no `FIN_CALL` is matched by `fin`'s own `fin-args-hash` against the hashes derived from
// the allowed command strings. Units for the shell reader, the hash (against the real `fin`
// binary), the matcher and the chain's after stage; the real runtime, store and `control.db`
// over `fake-claude` (its `Bash` runs the real `fin`) for the end-to-end cases.
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { execFile } from "node:child_process";
import { randomBytes } from "node:crypto";
import { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { Node, Principal, Session } from "@finisher/contracts";
import { Effect } from "effect";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from "vitest";
import { type FinCommand, finIndex } from "../src/agent/builtin/fin-tool.ts";
import { ARGS_HASH_HEADER, argsHashOf, type CommandIndex, derive, indexOf, ShellAttribution } from "../src/agent/claude/attribution.ts";
import { finRuns } from "../src/agent/claude/shell-words.ts";
import { agentChainHooks, type AgentHookDeps } from "../src/agent/reach.ts";
import { NO_JOURNAL, type RuntimeJournal } from "../src/agent/runtime-deps.ts";
import { resolveFinPath } from "../src/agent/settings.ts";
import type { SessionTx } from "../src/agent/store-tables.ts";
import { type SandboxStatus, setSandboxStatusSource } from "../src/sandbox/status.ts";
import { sandboxStatus } from "@finisher/fin-node";
import { AgentServer } from "./support/agent-server.ts";
import { addAsset, commit, confirmed, type NdjsonClient, ok, tx } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
const FAKE = fileURLToPath(new URL(`../../../target/debug/fake-claude${WIN ? ".exe" : ""}`, import.meta.url));
const FIN = resolveFinPath({ models: [] } as unknown as Parameters<typeof resolveFinPath>[0]);
const REQUIRE = process.env.FIN_REQUIRE_SERVER === "1";
const withFin = test.skipIf(WIN || (!existsSync(FIN) && !REQUIRE));
const withFake = test.skipIf(WIN || ((!existsSync(FAKE) || !existsSync(FIN)) && !REQUIRE));
const VERSION = "2.1.321";

let index: CommandIndex | undefined;
const idx = async (): Promise<CommandIndex> => (index ??= (await indexOf(() => finIndex(FIN))())!);

// ---- the shell reader ----

describe("finRuns", () => {
  test("static runs, wrappers, lists and redirections", () => {
    expect(finRuns("fin undo")).toEqual({ runs: [["undo"]], opaque: false });
    expect(finRuns(`cd x && FOO=1 /usr/local/bin/fin clip insert --at "1 s" 'a b' 2>&1 | tee out.txt; ls -la`)).toEqual({ runs: [["clip", "insert", "--at", "1 s", "a b"]], opaque: false });
    expect(finRuns("nohup fin undo > /dev/null 2>&1 &")).toEqual({ runs: [["undo"]], opaque: false });
    expect(finRuns("timeout 10 env -i A=b nice -n 5 fin redo\nfin undo # fin comment")).toEqual({ runs: [["redo"], ["undo"]], opaque: false });
    expect(finRuns("if [ -f x ]; then { fin undo; }; fi")).toEqual({ runs: [["undo"]], opaque: false });
    expect(finRuns("(sleep 1; fin undo) &")).toEqual({ runs: [["undo"]], opaque: false });
    expect(finRuns("git status; ls *.txt; echo $HOME")).toEqual({ runs: [], opaque: false });
    expect(finRuns(`fin view --json \\\n  "a\\"b"`)).toEqual({ runs: [["view", "--json", 'a"b']], opaque: false });
  });

  test("shell_expansion_ambiguous (unit): what cannot be read statically is opaque", () => {
    for (const c of [
      "fin $CMD",
      "fin clip insert --at ${T}",
      'fin bin add --name "$N"',
      "fin bin add --name *.mov",
      "fin bin add --name {a,b}",
      "fin bin add --name ~/x",
      "$FIN undo",
      "echo $(fin undo)",
      "x=`fin undo`",
      "bash -c 'fin undo'",
      "sh script.sh",
      "./do-edits.sh",
      "xargs fin undo < ids",
      "find . -name '*.mov' -exec fin asset add {} ;",
      "cat <<EOF | fin batch\n{}\nEOF",
      "case $x in a) fin undo;; esac",
      "fin undo 'unterminated",
      "eval fin undo",
    ]) {
      expect(finRuns(c).opaque, c).toBe(true);
    }
  });
});

// ---- the hash against fin's own ----

/**
 * The `fin-args-hash` header the real `fin` sends for `argv` under an agent session: a TCP
 * listener takes the first request frame (`server.hello`) and closes, so `fin` stops there.
 */
async function finsOwn(argvs: string[][]): Promise<(string | undefined)[]> {
  const dir = mkdtempSync(join(tmpdir(), "attribution-hash-"));
  const cred = join(dir, "credential.json");
  writeFileSync(cred, JSON.stringify({ v: 1, kind: "agent", id: "S1", key: randomBytes(32).toString("base64url") }));
  chmodSync(cred, 0o600);
  let frame: ((headers: unknown) => void) | undefined;
  const server: Server = createServer((s) => {
    let buf = "";
    s.on("data", (d) => {
      buf += String(d);
      const nl = buf.indexOf("\n");
      if (nl < 0) return;
      frame?.((JSON.parse(buf.slice(0, nl)) as { headers?: unknown }).headers);
      s.destroy();
    });
    s.on("error", () => {});
  });
  await new Promise<void>((res) => server.listen(0, "127.0.0.1", res));
  const port = (server.address() as { port: number }).port;
  const env = { PATH: process.env.PATH ?? "", HOME: dir, FIN_SERVER: `tcp:127.0.0.1:${port}`, FIN_SESSION: "S1", FIN_CREDENTIAL: cred, FIN_DIAGNOSTICS: "off", FIN_PROJECT_DIR: dir };
  const out: (string | undefined)[] = [];
  try {
    for (const argv of argvs) {
      let headers: unknown;
      frame = (h) => (headers ??= h);
      await new Promise<void>((res) => execFile(FIN, argv, { env, cwd: dir, timeout: 30_000 }, () => res()));
      const pair = Array.isArray(headers) ? (headers as [string, string][]).find((h) => h[0] === ARGS_HASH_HEADER) : undefined;
      out.push(pair?.[1]);
    }
  } finally {
    server.close();
    rmSync(dir, { recursive: true, force: true });
  }
  return out;
}

describe("the hash", () => {
  withFin("argshash_matches_fin_own: the derived hash is what fin sends, over a sample of the registry", async () => {
    const ix = await idx();
    // Hand-picked shapes: globals before and between the words, `=` values, switches, a
    // two-value flag, a repeated (Vec) flag, a comma list, `--`, `-`, Unicode and quotes.
    const sample: string[][] = [
      ["undo"],
      ["--branch", "b1", "undo", "--dry-run"],
      ["bin", "--branch", "b1", "add", "--name=x"],
      ["view", "--timeline=main"],
      ["transition", "set", "--between", "i1", "i2", "--dur", "1s"],
      ["bin", "add", "--name", "ü \"quoted\" \\ \t name"],
      ["review", "decide", "R1", "--accept", "t1,t2", "--accept", "t3", "--reject", "t4"],
      ["caption", "set-text", "c1", "--", "-dash text"],
      ["try", "b1"],
      ["try", "keep"],
      ["merge", "b2", "--idem-key", "k1"],
      ["log", "-"],
    ];
    const fins = await finsOwn(sample);
    sample.forEach((argv, i) => expect(fins[i], `fin sent no hash for ${JSON.stringify(argv)}`).toBeDefined());
    sample.forEach((argv, i) => expect(argsHashOf(argv, ix), JSON.stringify(argv)).toBe(fins[i]));
    // Every registry command: the shortest argv fin accepts (its words, then with one or two
    // positionals), then that with the global flags around it, and with each of its own flags.
    const bases = new Map<string, string[]>();
    // Last, every flag of its own given ("1"), for the commands with required flags.
    const own = (c: FinCommand) => (c.flags ?? []).filter((f) => f.long !== undefined && !ix.globals.some((g) => g.long === f.long));
    const allFlags = (c: FinCommand) => own(c).flatMap((f) => [f.long!, ...Array.from({ length: f.values ?? 0 }, () => "1")]);
    for (const extra of [(_: FinCommand) => [], () => ["x"], () => ["x", "y"], allFlags, (c: FinCommand) => ["x", ...allFlags(c)]]) {
      const todo = ix.commands.filter((c) => !bases.has(c.name)).map((c) => ({ name: c.name, argv: [...c.name.split("."), ...extra(c)] }));
      const got = await finsOwn(todo.map((t) => t.argv));
      todo.forEach(({ name, argv }, i) => {
        if (got[i] === undefined) return;
        bases.set(name, argv);
        expect(argsHashOf(argv, ix), JSON.stringify(argv)).toBe(got[i]);
      });
    }
    const variants: string[][] = [];
    for (const [name, base] of bases) {
      const words = name.split(".");
      variants.push(["--branch", "main", ...words, ...base.slice(words.length), "--dry-run", "--timeline=t1"]);
      for (const f of own(ix.commands.find((c) => c.name === name)!)) {
        if (!base.includes(f.long!)) variants.push([...base, f.long!, ...Array.from({ length: f.values ?? 0 }, (_, i) => `v${i},w`)]);
      }
    }
    const vown = await finsOwn(variants);
    let compared = 0;
    variants.forEach((argv, i) => {
      if (vown[i] === undefined) return; // fin refused this flag's value
      compared++;
      expect(argsHashOf(argv, ix), JSON.stringify(argv)).toBe(vown[i]);
    });
    // fin asks the server for most registry commands (not `schema`, `help`, offline ones).
    // (114 of 143 commands and 168 of 227 variants on 2026-11-13: the rest need values fin checks.)
    expect(bases.size).toBeGreaterThan(ix.commands.length * 0.75);
    expect(compared).toBeGreaterThan(variants.length * 0.6);
  }, 300_000);

  withFin("a parse the index cannot follow derives nothing (opaque), never a wrong hash", async () => {
    const ix = await idx();
    for (const argv of [["undo", "--no-such-flag"], ["undo", "-xyz"], ["clip", "insert", "--at"], ["nonsense"], ["undo", "--dry-run=true"], ["bin", "add", "--name", "-n"]]) {
      expect(argsHashOf(argv, ix), JSON.stringify(argv)).toBeUndefined();
    }
    expect(derive("fin undo --no-such-flag", ix)).toEqual({ hashes: new Set(), opaque: true });
    expect(derive("fin undo; fin undo", ix).hashes.size).toBe(1);
  });
});

// ---- the matcher, the journal wrapper and the after stage ----

const H = (argv: string[]) => argsHashOf(argv, index!)!;

function recording(): { journal: RuntimeJournal; results: { callId: string; ambiguous?: readonly string[] }[] } {
  const results: { callId: string; ambiguous?: readonly string[] }[] = [];
  return { journal: { ...NO_JOURNAL, toolResult: (r) => void results.push({ callId: r.callId, ...(r.ambiguous === undefined ? {} : { ambiguous: r.ambiguous }) }) }, results };
}

const SESSION = { id: "S1", projectId: "P1", role: "editor", rootBranch: "B1", mode: "draft" } as unknown as Session;
const AGENT: Principal = { kind: "agent", id: "S1" } as Principal;

/** The chain's after stage over a recording session source. */
function afterStage(attribution: ShellAttribution) {
  const rows: Parameters<AgentHookDeps["sessions"]["recordTx"]>[0][] = [];
  const sessions = { get: (id: string) => (id === "S1" ? SESSION : undefined), recordTx: (t: (typeof rows)[number]) => void rows.push(t) };
  const reach = { sessionOf: (p: Principal) => (p.kind === "agent" ? sessions.get(p.id) : undefined), covers: () => true };
  const hooks = agentChainHooks({ reach, paths: {}, sessions, attribution } as unknown as AgentHookDeps);
  const committed = (txId: string) => ({ rev: 1, tx: { txId }, bindings: {} });
  const commitOf = (txId: string, headers: [string, string][]) => Effect.runSync(hooks.after!("tx.commit", AGENT, { projectId: "P1" }, committed(txId), headers));
  return { rows, commitOf };
}

describe("the matcher", () => {
  withFin("window_opens_at_allow_closes_at_result (unit): before the allow none, inside inferred, after the result none", async () => {
    const ix = await idx();
    const a = new ShellAttribution({ index: async () => ix });
    const { journal, results } = recording();
    const j = a.journal(journal);
    const h = H(["undo"]);
    expect(a.match("S1", h, ["t0"])).toEqual({ callId: null, attribution: "none" });
    await a.allowed("S1", "C1", "Bash", { command: "fin undo" });
    expect(a.isOpen("S1", "C1")).toBe(true);
    expect(a.match("S1", h, ["t1"])).toEqual({ callId: "C1", attribution: "inferred" });
    // Another session's commit with the same hash is not this session's call.
    expect(a.match("S2", h, ["t2"])).toEqual({ callId: null, attribution: "none" });
    j.toolResult({ sessionId: "S1", runId: "R1", callId: "C1", ok: true, output: null, synthetic: false });
    expect(a.isOpen("S1", "C1")).toBe(false);
    expect(a.match("S1", h, ["t3"])).toEqual({ callId: null, attribution: "none" });
    expect(results).toEqual([{ callId: "C1" }]);
    // Not a shell tool, or no call announced: nothing opens.
    await a.allowed("S1", "C2", "Read", { command: "fin undo" });
    await a.allowed("S1", undefined, "Bash", { command: "fin undo" });
    expect(a.match("S1", h, ["t4"]).attribution).toBe("none");
    // A synthetic result (an interrupt, recovery) closes the window too.
    await a.allowed("S1", "C3", "Bash", { command: "fin undo" });
    j.toolResult({ sessionId: "S1", runId: "R1", callId: "C3", ok: false, output: null, synthetic: true });
    expect(a.isOpen("S1", "C3")).toBe(false);
  });

  withFin("two_open_calls_same_hash_ambiguous (unit): no call on the commit, its txId on both results", async () => {
    const ix = await idx();
    const a = new ShellAttribution({ index: async () => ix });
    const { journal, results } = recording();
    const j = a.journal(journal);
    const { rows, commitOf } = afterStage(a);
    await a.allowed("S1", "C1", "Bash", { command: "fin undo" });
    await a.allowed("S1", "C2", "Bash", { command: "sleep 1 && fin undo" });
    await a.allowed("S1", "C3", "Bash", { command: "fin redo" });
    commitOf("T1", [[ARGS_HASH_HEADER, H(["undo"])]]);
    expect(rows).toEqual([expect.objectContaining({ txId: "T1", sessionId: "S1", callId: null, attribution: "ambiguous" })]);
    // A hash only one open call shows is inferred to it.
    commitOf("T2", [[ARGS_HASH_HEADER, H(["redo"])]]);
    expect(rows[1]).toMatchObject({ txId: "T2", callId: "C3", attribution: "inferred" });
    // `FIN_CALL` wins: exact, whatever is open.
    commitOf("T3", [["fin-actor-call", "C9"], [ARGS_HASH_HEADER, H(["undo"])]]);
    expect(rows[2]).toMatchObject({ txId: "T3", callId: "C9", attribution: "exact" });
    for (const callId of ["C1", "C2", "C3"]) j.toolResult({ sessionId: "S1", runId: "R1", callId, ok: true, output: null, synthetic: false });
    expect(results).toEqual([{ callId: "C1", ambiguous: ["T1"] }, { callId: "C2", ambiguous: ["T1"] }, { callId: "C3" }]);
  });

  withFin("an opaque call is a candidate for every hash; a call that runs no fin is none", async () => {
    const ix = await idx();
    const a = new ShellAttribution({ index: async () => ix });
    const { rows, commitOf } = afterStage(a);
    await a.allowed("S1", "C1", "Bash", { command: "ls -la && git status" });
    commitOf("T1", [[ARGS_HASH_HEADER, H(["undo"])]]);
    expect(rows[0]).toMatchObject({ callId: null, attribution: "none" });
    await a.allowed("S1", "C2", "Bash", { command: "./edit.sh" });
    commitOf("T2", [[ARGS_HASH_HEADER, H(["undo"])]]);
    expect(rows[1]).toMatchObject({ callId: null, attribution: "ambiguous" });
    await a.allowed("S1", "C3", "Bash", { command: "fin undo" });
    commitOf("T3", [[ARGS_HASH_HEADER, H(["undo"])]]);
    expect(rows[2]).toMatchObject({ callId: null, attribution: "ambiguous" });
    // No header (an older fin): every call that may run fin is a candidate.
    commitOf("T4", []);
    expect(rows[3]).toMatchObject({ callId: null, attribution: "ambiguous" });
    // Without an index nothing is derivable: a fin run is opaque.
    const blind = new ShellAttribution({ index: async () => undefined });
    await blind.allowed("S1", "C1", "Bash", { command: "fin undo" });
    expect(blind.match("S1", H(["undo"]), ["T5"]).attribution).toBe("ambiguous");
  });
});

// ---- through the runtime, over the fake ----

const saved = { hooks: process.env.FIN_TEST_HOOKS, ready: process.env.FIN_TEST_SANDBOX_READY };
beforeAll(() => {
  process.env.FIN_TEST_HOOKS = "1";
  process.env.FIN_TEST_SANDBOX_READY = "agent";
});
afterAll(() => {
  for (const [k, v] of [["FIN_TEST_HOOKS", saved.hooks], ["FIN_TEST_SANDBOX_READY", saved.ready]] as const) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
});

let tmp: string;
let server: AgentServer | undefined;
let restore: (() => SandboxStatus) | undefined;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-attribution-")));
});
afterEach(async () => {
  await server?.stop();
  server = undefined;
  if (restore !== undefined) setSandboxStatusSource(restore);
  restore = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

/** The `agent` profile not ready: `direct` mode, every shell call asks a person (owner Q3a). */
const NOT_READY = (): SandboxStatus => ({ ...sandboxStatus(), agent: { kind: "disabled", reason: "profileNotReady" } });

interface Running {
  readonly local: NdjsonClient;
  readonly owner: NdjsonClient;
  readonly projectId: string;
}

/** One turn of `Bash` calls (each asked, run when allowed, its result written), then the turn's result. */
const bashTurn = (...inputs: Record<string, unknown>[]) => ({
  init: { apiKeySource: "/login managed key" },
  steps: [...inputs.map((input, i) => ({ ...(i === 0 ? { expect: "user" } : {}), ask: { tool: "Bash", input, bash: true } })), { cumulativeUsage: { inputTokens: 1, outputTokens: 1 } }],
});

/** Whether the real `agent` profile is enforced here (launcher mode); else skip (or fail under `FIN_REQUIRE_LANDLOCK=1`). */
function enforced(): boolean {
  if (sandboxStatus().agent.kind === "enforced") return true;
  expect(process.env.FIN_REQUIRE_LANDLOCK === "1", `FIN_REQUIRE_LANDLOCK=1 but the agent profile is ${JSON.stringify(sandboxStatus().agent)}`).toBe(false);
  return false;
}

async function claudeServer(script: Record<string, unknown>, status: () => SandboxStatus = NOT_READY): Promise<Running> {
  const previous = setSandboxStatusSource(status);
  restore ??= previous;
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  const dir = mkdtempSync(join(tmp, "cli-"));
  const binary = join(dir, "claude");
  copyFileSync(FAKE, binary);
  writeFileSync(join(dir, "script.json"), JSON.stringify(script));
  writeFileSync(join(dir, "fake-claude.json"), JSON.stringify({ script: "script.json", observed: "@config/observed.json", version: VERSION }));
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ agents: { claudeCode: { enabled: true, path: binary }, finPath: FIN } }));
  server = new AgentServer(serverData, { timers: false });
  await server.start();
  const local = await server.local();
  const owner = await server.console();
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaaP", 0, [addAsset()])));
  return { local, owner, projectId };
}

const start = async (r: Running) => ok<{ session: Session }>(await r.local.call("agent.session.start", { projectId: r.projectId, harness: "claudeCode", prompt: "go" })).session;

async function until<T>(f: () => T | undefined, ms = 20_000): Promise<T> {
  const end = Date.now() + ms;
  for (;;) {
    const v = f();
    if (v !== undefined) return v;
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((res) => setTimeout(res, 20));
  }
}
const store = () => server!.core.store;
const settle = (id: string) => until(() => (["idle", "interrupted", "ended"].includes(store().get(id)!.status) ? store().get(id)! : undefined));
const nodes = (id: string): Node[] => store().nodes(id, 1, 1000) as Node[];
const callIds = (id: string) => nodes(id).filter((n) => n.kind === "toolCall").map((n) => n.callId!);
const results = (id: string) => nodes(id).filter((n) => n.kind === "toolResult").map((n) => n.payload as { ok: boolean; output: unknown });

/** Answers each shell card allow as it opens (owner Q3a: a person's allow opens the window). */
async function allowCards(r: Running, id: string, n: number): Promise<void> {
  for (let i = 0; i < n; i++) {
    const card = await until(() => store().approvals(id).find((a) => a.status === "open"));
    ok(await confirmed(r.owner, "agent.approval.answer", { id: card.id, decision: "allow" }));
  }
}

const q = (s: string) => `'${s}'`;

describe("over fake-claude", () => {
  withFake("single_match_is_inferred: the fake's Bash runs fin with the allowed argv; the commit carries the call", async () => {
    const r = await claudeServer(bashTurn({ command: `${q(FIN)} bin add --name inferred > out.json`, description: "add a bin" }));
    const s = await start(r);
    await allowCards(r, s.id, 1);
    expect((await settle(s.id)).status).toBe("idle");
    expect(results(s.id)).toEqual([expect.objectContaining({ ok: true })]);
    const [call] = callIds(s.id);
    const txs = store().txsOfSession(s.id);
    expect(txs).toEqual([expect.objectContaining({ callId: call, attribution: "inferred" })]);
    expect(store().txsOfCall(call!).map((t: SessionTx) => t.txId)).toEqual(txs.map((t) => t.txId));
  }, 60_000);

  withFake("shell_expansion_ambiguous: a command built by expansion matches nothing exactly: ambiguous, no call", async () => {
    const r = await claudeServer(bashTurn({ command: `N=expanded; ${q(FIN)} bin add --name "$N"` }));
    const s = await start(r);
    await allowCards(r, s.id, 1);
    expect((await settle(s.id)).status).toBe("idle");
    expect(store().txsOfSession(s.id)).toEqual([expect.objectContaining({ callId: null, attribution: "ambiguous" })]);
  }, 60_000);

  withFake("background_process_after_result_is_none: a shell-backgrounded fin commits after the window: none, still the session's", async () => {
    // The shell's own `&`: the call's result comes once `sh` returns, the `fin` a second later.
    // (The CLI's `run_in_background` is refused under the launch's
    // `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1`, verified 2.1.321, Task 0: the next case.)
    const r = await claudeServer(bashTurn({ command: `(sleep 1; nohup ${q(FIN)} bin add --name late) > /dev/null 2>&1 &` }));
    const s = await start(r);
    await allowCards(r, s.id, 1);
    expect((await settle(s.id)).status).toBe("idle");
    expect(store().txsOfSession(s.id)).toEqual([]);
    const txs = await until(() => {
      const t = store().txsOfSession(s.id);
      return t.length > 0 ? t : undefined;
    });
    expect(txs).toEqual([expect.objectContaining({ callId: null, attribution: "none" })]);
  }, 60_000);

  withFake("the CLI's run_in_background is refused before any prompt: no card, no window, nothing runs", async () => {
    const r = await claudeServer(bashTurn({ command: `${q(FIN)} bin add --name never`, run_in_background: true }));
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    expect(store().approvals(s.id)).toEqual([]);
    expect(results(s.id)).toEqual([expect.objectContaining({ ok: false })]);
    await new Promise((res) => setTimeout(res, 500));
    expect(store().txsOfSession(s.id)).toEqual([]);
  }, 60_000);

  withFake("enforced (launcher): a policy allow opens the window too", async () => {
    if (!enforced()) return;
    const real = sandboxStatus();
    const r = await claudeServer(bashTurn({ command: `${q(FIN)} bin add --name policy` }), () => real);
    const s = await start(r);
    expect((await settle(s.id)).status).toBe("idle");
    expect(store().approvals(s.id)).toEqual([]);
    const [call] = callIds(s.id);
    expect(store().txsOfSession(s.id)).toEqual([expect.objectContaining({ callId: call, attribution: "inferred" })]);
  }, 60_000);

  withFake("two allowed calls in turn: each commit is inferred to its own call", async () => {
    const r = await claudeServer(bashTurn({ command: `${q(FIN)} bin add --name one` }, { command: `${q(FIN)} bin add --name two` }));
    const s = await start(r);
    await allowCards(r, s.id, 2);
    expect((await settle(s.id)).status).toBe("idle");
    const calls = callIds(s.id);
    expect(store().txsOfSession(s.id).map((t: SessionTx) => [t.callId, t.attribution])).toEqual([
      [calls[0], "inferred"],
      [calls[1], "inferred"],
    ]);
  }, 60_000);
});
