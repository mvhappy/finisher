// The built-in agent's tools (plan 7A Task 8b; Decisions 11, 12, 14b, 14c): the native `fin`
// tool against a stand-in `fin` (a script that answers by its first word) and the real binary
// when it is built, and the workspace file tools.
import { existsSync, mkdirSync, mkdtempSync, openSync, readdirSync, readFileSync, readSync, closeSync, realpathSync, rmSync, symlinkSync, writeFileSync, chmodSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import type { FinError } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { FinIndex, pathValues, runFinTool, type SessionEnv } from "../src/agent/builtin/fin-tool.ts";
import { builtinEnvironment } from "../src/agent/builtin/environment.ts";
import { builtinTools, isWrite, systemHash } from "../src/agent/builtin/tools.ts";
import { confine, workspaceTool } from "../src/agent/builtin/workspace-tools.ts";
import { childEnv } from "../src/env.ts";

let tmp: string;
let ws: string;
let project: string;
let serverData: string;

beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "agent-tools-")));
  project = join(tmp, "film");
  ws = join(project, "workspace", "S1");
  serverData = join(tmp, "server-data");
  mkdirSync(ws, { recursive: true });
  mkdirSync(join(serverData, "auth"), { recursive: true });
  writeFileSync(join(serverData, "auth", "local.key"), "secret-key");
});

afterEach(() => rmSync(tmp, { recursive: true, force: true }));

const posix = process.platform !== "win32";

/** A stand-in `fin`: `echo` prints its argv and env, `json`/`text` print, `fail` exits 3, `spawn` hangs with a child. */
function fakeFin(): string {
  const path = join(tmp, "fin");
  writeFileSync(
    path,
    `#!${process.execPath}
const [cmd, ...rest] = process.argv.slice(2);
const { spawn } = require("node:child_process");
const fs = require("node:fs");
if (cmd === undefined) { process.stdout.write('{"commands":["read: status view"]}\\n'); }
else if (cmd === "echo") { process.stdout.write(JSON.stringify({ argv: process.argv.slice(2), env: process.env, cwd: process.cwd(), stdin: fs.readFileSync(0, "utf8") })); }
else if (cmd === "json") { process.stdout.write('{"ok":1}\\n'); }
else if (cmd === "text") { process.stdout.write("plain words\\n"); }
else if (cmd === "fail") { process.stderr.write("e".repeat(5000) + "THE-END"); process.stdout.write('{"error":{"code":"USAGE"}}'); process.exit(3); }
else if (cmd === "spawn") { const c = spawn("sleep", ["30"], { stdio: "ignore" }); fs.writeFileSync(rest[0], String(c.pid)); setInterval(() => {}, 1000); }
`,
  );
  chmodSync(path, 0o755);
  return path;
}

const env = (finPath: string, over: Partial<SessionEnv> = {}): SessionEnv => ({
  finPath,
  cwd: ws,
  env: { ...childEnv({ FIN_SERVER: "/run/finisherd.sock", FIN_SESSION: "S1" }), FIN_CREDENTIAL: join(serverData, "agents", "S1", "credential.json") },
  ...over,
});

const CALL = "01JC0000000000000000000006";
const code = async (p: Promise<unknown>) => ((await p.then(() => undefined, (e: FinError) => e)) as FinError | undefined)?.code;
const alive = (pid: number) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

describe.skipIf(!posix)("fin tool", () => {
  test("argv_without_shell", async () => {
    const fin = fakeFin();
    const argv = ["echo", "; rm -rf x", "$(touch pwned)", "`id`", "a b", "*"];
    const r = await runFinTool({ callId: CALL, argv, stdin: "line\n" }, env(fin));
    const out = (r.output as { stdout: { argv: string[]; cwd: string; stdin: string } }).stdout;
    expect(out.argv).toEqual(argv);
    expect(out.cwd).toBe(ws);
    expect(out.stdin).toBe("line\n");
    expect(existsSync(join(ws, "pwned"))).toBe(false);
    // argv[0] is a command word, never a path or an option.
    for (const bad of ["/bin/sh", "./fin", "--project", "", "VIEW"]) {
      expect(await code(runFinTool({ callId: CALL, argv: [bad] }, env(fin)))).toBe("USAGE");
    }
  });

  test.skipIf(!existsSync(fileURLToPath(new URL("../../../target/debug/fin", import.meta.url))))("argv_reaches_the_real_fin_verbatim", async () => {
    const real = fileURLToPath(new URL("../../../target/debug/fin", import.meta.url));
    const r = await runFinTool({ callId: CALL, argv: ["view", "; rm -rf x"] }, env(real));
    expect(r.ok).toBe(false);
    expect(r.output).toMatchObject({ exit: 2, stdout: { error: { code: "USAGE" } } });
  });

  test("env_is_allow_list_plus_session_vars", async () => {
    const fin = fakeFin();
    const parent = { ...process.env, FIN_TRAINING: "on", FIN_SERVER_DATA: serverData, FIN_AUTHOR: "someone", OPENROUTER_API_KEY: "sk-leak", FIN_CALL_IDEM: "0" };
    const base = childEnv({ FIN_SERVER: "/s.sock", FIN_SESSION: "S1" }, parent);
    // Even when the caller passes them, these never reach `fin`.
    const passed = { ...base, FIN_CREDENTIAL: "/c.json", FIN_SERVER_DATA: serverData, FIN_AUTHOR: "x", FIN_TRAINING: "on" };
    const r = await runFinTool({ callId: CALL, argv: ["echo"] }, env(fin, { env: passed }));
    const seen = (r.output as { stdout: { env: Record<string, string> } }).stdout.env;
    const allowed = new Set(["PATH", "HOME", "LANG", "TMPDIR", "FIN_SERVER", "FIN_SESSION", "FIN_CREDENTIAL", "FIN_CALL", "FIN_CALL_IDEM"]);
    for (const k of Object.keys(seen)) expect(allowed.has(k) || k.startsWith("LC_"), k).toBe(true);
    expect(seen).toMatchObject({ FIN_CALL: CALL, FIN_CALL_IDEM: "1", FIN_CREDENTIAL: "/c.json", FIN_SERVER: "/s.sock", FIN_SESSION: "S1" });
    for (const k of ["FIN_TRAINING", "FIN_SERVER_DATA", "FIN_AUTHOR", "OPENROUTER_API_KEY"]) expect(seen[k], k).toBeUndefined();
    expect(base.FIN_SERVER_DATA).toBeUndefined();
  });

  test("timeout_kills_tree", async () => {
    const fin = fakeFin();
    const pidFile = join(tmp, "child.pid");
    const r = await runFinTool({ callId: CALL, argv: ["spawn", pidFile] }, env(fin, { timeoutMs: 400 }));
    expect(r).toMatchObject({ ok: false, output: { error: { code: "TOOL_TIMEOUT" } } });
    const child = Number(readFileSync(pidFile, "utf8"));
    const t0 = Date.now();
    while (alive(child) && Date.now() - t0 < 3000) await new Promise((res) => setTimeout(res, 20));
    expect(alive(child)).toBe(false);
  });

  test("an_abort_stops_the_call_and_its_tree", async () => {
    // A budget stop past its grace aborts the running calls (6b review): the executor answers at
    // once and `fin`'s process tree is gone.
    const fin = fakeFin();
    const pidFile = join(tmp, "child.pid");
    const exec = builtinTools({ finPath: fin, projectDirOf: () => project });
    const ac = new AbortController();
    const base = { sessionId: "S1", projectId: "P1", runId: "R1", callId: CALL, providerId: "p", cwd: ws, env: { PATH: process.env.PATH ?? "" } };
    const running = exec.run({ ...base, tool: "fin", input: { argv: ["spawn", pidFile] } }, ac.signal);
    while (!existsSync(pidFile) || readFileSync(pidFile, "utf8") === "") await new Promise((res) => setTimeout(res, 10));
    const t0 = Date.now();
    ac.abort(new Error("budget"));
    expect(await code(running)).toBe("SESSION_INTERRUPTED");
    expect(Date.now() - t0).toBeLessThan(1000);
    const child = Number(readFileSync(pidFile, "utf8"));
    while (alive(child) && Date.now() - t0 < 3000) await new Promise((res) => setTimeout(res, 20));
    expect(alive(child)).toBe(false);
    // An aborted signal stops a call before it starts, the workspace tools' too.
    expect(await code(exec.run({ ...base, tool: "workspace_write", input: { path: "late.md", content: "x" } }, ac.signal))).toBe("SESSION_INTERRUPTED");
    expect(existsSync(join(ws, "late.md"))).toBe(false);
    expect(await code(runFinTool({ callId: CALL, argv: ["json"] }, env(fin, { signal: ac.signal })))).toBe("SESSION_INTERRUPTED");
  });

  test("json_stdout_parsed", async () => {
    const fin = fakeFin();
    expect(await runFinTool({ callId: CALL, argv: ["json"] }, env(fin))).toEqual({ ok: true, output: { exit: 0, stdout: { ok: 1 } } });
    expect(await runFinTool({ callId: CALL, argv: ["text"] }, env(fin))).toEqual({ ok: true, output: { exit: 0, stdout: "plain words" } });
  });

  test("exit_and_stderr_tail", async () => {
    const fin = fakeFin();
    const r = await runFinTool({ callId: CALL, argv: ["fail"] }, env(fin));
    const out = r.output as { exit: number; stdout: unknown; stderrTail: string };
    expect(r.ok).toBe(false);
    expect(out.exit).toBe(3);
    expect(out.stdout).toEqual({ error: { code: "USAGE" } });
    expect(out.stderrTail.length).toBe(1024);
    expect(out.stderrTail.endsWith("THE-END")).toBe(true);
  });

  test("recognised_path_arguments_are_confined", async () => {
    const fin = fakeFin();
    const index = new FinIndex([
      { name: "caption.import", write: true, paths: [{ arg: "words", flag: "--words" }] },
      { name: "asset.add", write: true, paths: [{ arg: "path", position: 0 }] },
      { name: "echo", write: false, paths: [{ arg: "file", flag: "--file" }] },
    ]);
    expect(pathValues(["caption", "import", "--track", "t2", "--words=w.json"], index)).toEqual(["w.json"]);
    expect(pathValues(["asset", "add", "take.mp4"], index)).toEqual(["take.mp4"]);
    const roots = [ws, project];
    const key = join(serverData, "auth", "local.key");
    writeFileSync(join(ws, "w.json"), "[]");
    symlinkSync(key, join(ws, "key.json"));
    const run = (argv: string[]) => runFinTool({ callId: CALL, argv }, env(fin, { index, roots }));
    expect((await run(["echo", "--file", "w.json"])).ok).toBe(true);
    expect((await run(["echo", "--file", "../../other.mp4"])).ok).toBe(true); // the project folder
    for (const bad of [key, "../../../server-data/auth/local.key", "key.json", "/etc/hosts", "~/x"]) {
      expect(await code(run(["echo", "--file", bad])), bad).toBe("PATH_NOT_ALLOWED");
    }
  });

  test("the_executor_runs_fin_and_workspace_tools", async () => {
    const fin = fakeFin();
    const exec = builtinTools({ finPath: fin, projectDirOf: () => project });
    const ac = new AbortController();
    const base = { sessionId: "S1", projectId: "P1", runId: "R1", callId: CALL, providerId: "p", cwd: ws, env: { PATH: process.env.PATH ?? "" } };
    const r = await exec.run({ ...base, tool: "fin", input: { argv: ["json"] } }, ac.signal);
    expect(r).toEqual({ ok: true, output: { exit: 0, stdout: { ok: 1 } } });
    expect(await exec.run({ ...base, tool: "workspace_write", input: { path: "n.md", content: "hi" } }, ac.signal)).toMatchObject({ ok: true });
    expect(readFileSync(join(ws, "n.md"), "utf8")).toBe("hi");
    expect(await code(exec.run({ ...base, tool: "shell", input: {} }, ac.signal))).toBe("USAGE");
    expect(await code(exec.run({ ...base, tool: "fin", input: { argv: "view" } }, ac.signal))).toBe("USAGE");
    expect(isWrite("workspace_read", {}, undefined)).toBe(false);
    expect(isWrite("fin", { argv: ["item", "trim", "--dry-run"] }, undefined)).toBe(false);
    expect(isWrite("fin", { argv: ["view"] }, undefined)).toBe(true);
  });
});

describe.skipIf(!posix)("session environment", () => {
  test("system_prompt_is_role_and_map_only_and_the_env_is_the_allow_list", async () => {
    const fin = fakeFin();
    const e = builtinEnvironment({ finPath: fin, server: "/run/f.sock", projectDirOf: () => project, rolePrompt: (r) => `You are the ${r}.` });
    const session = (id: string) => ({ id, projectId: "P1", role: "editor" }) as Parameters<typeof e.prepare>[0];
    const [a, b] = [await e.prepare(session("S1")), await e.prepare(session("S2"))];
    expect(a.system).toBe(b.system);
    expect(systemHash(a.system)).toBe(systemHash(b.system));
    expect(a.system).toContain("You are the editor.");
    expect(a.system).toContain("read: status view");
    expect(a.cwd).toBe(join(project, "workspace", "S1"));
    expect(existsSync(b.cwd)).toBe(true);
    expect(a.env).toMatchObject({ FIN_SERVER: "/run/f.sock", FIN_SESSION: "S1" });
    expect(a.env.FIN_SERVER_DATA).toBeUndefined();
  });
});

describe("workspace tools", () => {
  test("rejects_absolute_and_dotdot", async () => {
    for (const bad of ["/etc/passwd", "C:\\Windows\\x", "c:x", "\\\\server\\share\\x", "\\x", "../x", "a/../../x", "a/../b", "a\\..\\b", "x\0y"]) {
      expect(await code(workspaceTool("workspace_read", { path: bad }, ws)), bad).toBe("PATH_NOT_ALLOWED");
      expect(await code(workspaceTool("workspace_write", { path: bad, content: "x" }, ws)), bad).toBe("PATH_NOT_ALLOWED");
    }
    expect(await code(workspaceTool("workspace_list", { path: ".." }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(await code(workspaceTool("workspace_read", { path: 7 }, ws))).toBe("USAGE");
  });

  test.skipIf(!posix)("rejects_symlink_escape", async () => {
    const key = join(serverData, "auth", "local.key");
    symlinkSync(key, join(ws, "key"));
    symlinkSync(join(serverData, "auth"), join(ws, "auth"));
    expect(await code(workspaceTool("workspace_read", { path: "key" }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(await code(workspaceTool("workspace_read", { path: "auth/local.key" }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(await code(workspaceTool("workspace_write", { path: "key", content: "pwned", mode: "overwrite" }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(await code(workspaceTool("workspace_write", { path: "auth/new", content: "x" }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(await code(workspaceTool("workspace_list", { path: "auth" }, ws))).toBe("PATH_NOT_ALLOWED");
    expect(readFileSync(key, "utf8")).toBe("secret-key");
    expect(readdirSync(join(serverData, "auth"))).toEqual(["local.key"]);
    // A link that stays inside is followed.
    writeFileSync(join(ws, "real.md"), "inside");
    symlinkSync(join(ws, "real.md"), join(ws, "alias.md"));
    expect(await workspaceTool("workspace_read", { path: "alias.md" }, ws)).toMatchObject({ ok: true, output: { content: "inside" } });
  });

  test.skipIf(!posix)("case_folding_on_windows", async () => {
    // On a case-insensitive volume `WS` and `ws` are one folder; on Linux they are two. A link to
    // the other spelling is inside when paths fold (Windows, macOS) and outside when they do not.
    const lower = join(tmp, "ws");
    const upper = join(tmp, "WS");
    mkdirSync(lower);
    mkdirSync(upper);
    writeFileSync(join(upper, "f.md"), "x");
    symlinkSync(join(upper, "f.md"), join(lower, "f.md"));
    expect(await confine("f.md", lower, { fold: true })).toMatchObject({ rel: "f.md" });
    expect(await code(confine("f.md", lower, { fold: false }))).toBe("PATH_NOT_ALLOWED");
  });

  test("size_caps", async () => {
    writeFileSync(join(ws, "big.txt"), "a".repeat(1000));
    const r = await workspaceTool("workspace_read", { path: "big.txt" }, ws, { readMax: 400 });
    expect(r.output).toMatchObject({ size: 1000, offset: 0, truncated: true, next: 400 });
    expect((r.output as { content: string }).content).toHaveLength(400);
    const tail = await workspaceTool("workspace_read", { path: "big.txt", offset: 900, limit: 500 }, ws, { readMax: 400 });
    expect(tail.output).toMatchObject({ offset: 900, content: "a".repeat(100) });
    expect(await code(workspaceTool("workspace_write", { path: "x", content: "b".repeat(101) }, ws, { fileMax: 100 }))).toBe("FILE_TOO_LARGE");
    expect(await code(workspaceTool("workspace_write", { path: "y", content: "b".repeat(50) }, ws, { workspaceMax: 1020 }))).toBe("WORKSPACE_FULL");
    expect(existsSync(join(ws, "x")) || existsSync(join(ws, "y"))).toBe(false);
  });

  test("write_is_temp_then_rename", async () => {
    expect(await workspaceTool("workspace_write", { path: "notes/a.md", content: "one" }, ws)).toEqual({ ok: true, output: { path: "notes/a.md", size: 3, mode: "create" } });
    expect(await code(workspaceTool("workspace_write", { path: "notes/a.md", content: "two" }, ws))).toBe("ALREADY_EXISTS");
    // A reader holding the old file still reads the old bytes: the new file replaced it whole.
    const fd = openSync(join(ws, "notes", "a.md"), "r");
    await workspaceTool("workspace_write", { path: "notes/a.md", content: "replaced", mode: "overwrite" }, ws);
    const buf = Buffer.alloc(16);
    const n = readSync(fd, buf, 0, 16, 0);
    closeSync(fd);
    expect(buf.subarray(0, n).toString()).toBe("one");
    await workspaceTool("workspace_write", { path: "notes/a.md", content: "+more", mode: "append" }, ws);
    expect(readFileSync(join(ws, "notes", "a.md"), "utf8")).toBe("replaced+more");
    // No temp file is left behind.
    expect(readdirSync(join(ws, "notes"))).toEqual(["a.md"]);
    const listed = await workspaceTool("workspace_list", {}, ws);
    expect(listed.output).toEqual({ path: ".", entries: [{ name: "notes", kind: "dir" }] });
    expect((await workspaceTool("workspace_list", { path: "notes" }, ws)).output).toEqual({ path: "notes", entries: [{ name: "a.md", kind: "file", size: 13 }] });
    expect(await code(workspaceTool("workspace_read", { path: "nope.md" }, ws))).toBe("NOT_FOUND");
  });
});
