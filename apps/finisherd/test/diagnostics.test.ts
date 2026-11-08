// The server's diagnostics (plan D, Task D4): settings before the journal, the live watch and
// its last-good rule, env overrides, `finisherd.json`, sessions, uncaught errors, RPC timing,
// `diagnostics.record` and the `diagnostics.*` RPCs. Sources, actors and the human-only rules
// from principals (5A Task 7) are `diagnostics-principals.test.ts`.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";
import { FinRpcs } from "@finisher/contracts";
import type { ConsentState, DiagnosticsReport, Envelope, ExportPage } from "@finisher/core-types";
import { rpcMethods } from "@finisher/fin-node";
import { Effect } from "effect";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { settingsPath } from "../src/config.ts";
import { actorFor, locationOf, makeDiagnostics, TokenBucket } from "../src/diagnostics.ts";
import { CAPABILITIES } from "../src/handlers.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { confirmed, connectProved, NdjsonClient, ok, type RunningServer, startServer } from "./support/ndjson-client.ts";

const WIN = process.platform === "win32";
let tmp: string;
let serverData: string;
let server: RunningServer | undefined;
const clients: NdjsonClient[] = [];

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-diag-"));
  serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
});

afterEach(async () => {
  await Promise.all(clients.splice(0).map((c) => c.close().catch(() => undefined)));
  await server?.stop();
  server = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

/** The environment without the caller's own diagnostics switches (a developer may set them). */
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

function writeSettings(v: unknown): void {
  writeFileSync(settingsPath(serverData), typeof v === "string" ? v : JSON.stringify(v));
}

/** Every entry on disk (the server stopped or flushed), oldest first. */
function onDisk(): Envelope[] {
  const dir = join(serverData, "events");
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.startsWith("j-"))
    .sort()
    .flatMap((f) =>
      readFileSync(join(dir, f), "utf8")
        .split("\n")
        .filter((l) => l.length > 0)
        .map((l) => JSON.parse(l) as Envelope),
    );
}

async function exported(c: NdjsonClient, filter: object = {}): Promise<Envelope[]> {
  const out: Envelope[] = [];
  let cursor: string | undefined;
  for (;;) {
    const page = ok<ExportPage>(await c.call("diagnostics.export", { filter, ...(cursor ? { cursor } : {}) }));
    out.push(...page.lines);
    if (page.next === undefined) return out;
    cursor = page.next;
  }
}

async function until(f: () => boolean | Promise<boolean>, ms = 5000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!(await f())) {
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise((r) => setTimeout(r, 20));
  }
}

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

const record = (c: NdjsonClient, payload: unknown = cli(), kind = "cli.command", headers: ReadonlyArray<[string, string]> = []) =>
  c.call("diagnostics.record", { kind, kv: 1, payload }, headers);

/** Runs `f` against an in-process `Diagnostics` (closed afterwards). */
function inProcess<A>(f: (d: Effect.Success<ReturnType<typeof makeDiagnostics>>) => Promise<A>, pollMs = 20): Promise<A> {
  return Effect.runPromise(
    Effect.scoped(Effect.flatMap(makeDiagnostics(serverData, { settingsPollMs: pollMs }), (d) => Effect.promise(() => f(d)))),
  );
}

describe("settings and the journal", () => {
  test("settings_read_before_journal_opens: off in settings → no events/, not even session.start", async () => {
    writeSettings({ privacy: { diagnostics: "off" } });
    await inProcess(async (d) => {
      expect(d.enabled()).toBe(false);
      expect(JSON.parse(d.journal.consent())).toMatchObject({ diagnostics: "off", training: "undecided" });
    });
    expect(existsSync(join(serverData, "events"))).toBe(false);
  });

  test("settings_privacy_applies_live and the listener hears it", async () => {
    await inProcess(async (d) => {
      const seen: boolean[] = [];
      d.onEnabledChange((e) => seen.push(e));
      expect(d.enabled()).toBe(true);
      writeSettings({ privacy: { diagnostics: "off" } });
      await until(() => !d.enabled());
      writeSettings({ fonts: { autoFetch: false }, privacy: {} });
      await until(() => d.enabled());
      expect(seen).toEqual([false, true]);
    });
  });

  test("bad_privacy_section_keeps_last_good_and_warns", async () => {
    const warn = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    try {
      writeSettings({ privacy: { diagnostics: "off" } });
      await inProcess(async (d) => {
        writeSettings({ privacy: { diagnostics: "off", maxMiB: 1 } });
        await until(() => warn.mock.calls.some(([m]) => String(m).includes("maxMiB")));
        expect(d.enabled()).toBe(false);
        writeSettings("{ truncated");
        await until(() => warn.mock.calls.some(([m]) => String(m).includes("keeping the last good")));
        expect(d.enabled()).toBe(false);
      });
    } finally {
      warn.mockRestore();
    }
  });

  test("bad_privacy_before_first_good_read_is_off", async () => {
    const warn = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    try {
      writeSettings({ privacy: { diagnostics: "maybe" } });
      await inProcess(async (d) => expect(d.enabled()).toBe(false));
      writeSettings("{ not json");
      await inProcess(async (d) => expect(d.enabled()).toBe(false));
      expect(warn.mock.calls.some(([m]) => String(m).includes("recording nothing"))).toBe(true);
    } finally {
      warn.mockRestore();
    }
    expect(existsSync(join(serverData, "events"))).toBe(false);
  });

  test("consent_rpc_refuses_a_change_beside_an_invalid_key (the opt-out applies or fails)", async () => {
    writeSettings({ privacy: { diagnostics: "on" } });
    const warn = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
    try {
      await inProcess(async (d) => {
        // A hand edit adds a key the journal refuses: it keeps the last good settings.
        writeSettings({ privacy: { diagnostics: "on", retentionDays: 0 } });
        await until(() => warn.mock.calls.some(([m]) => String(m).includes("retentionDays")));
        const before = readFileSync(settingsPath(serverData), "utf8");
        const e = await Effect.runPromise(Effect.flip(d.consent(CONSOLE, "diagnostics", "off")));
        expect(e.code).toBe("SETTINGS_INVALID");
        expect(e.fix).toContain("privacy.retentionDays");
        expect(readFileSync(settingsPath(serverData), "utf8")).toBe(before);
        expect(d.enabled()).toBe(true);
        // Fixed by hand: the opt-out applies and says so.
        writeSettings({ privacy: { diagnostics: "on" } });
        const c = await Effect.runPromise(d.consent(CONSOLE, "diagnostics", "off"));
        expect(c.diagnostics).toBe("off");
        expect(d.enabled()).toBe(false);
      });
    } finally {
      warn.mockRestore();
    }
  });

  test("a_vanished_settings_file_does_not_turn_an_opt_out_on", async () => {
    writeSettings({ privacy: { diagnostics: "off" } });
    await inProcess(async (d) => {
      const seen: boolean[] = [];
      d.onEnabledChange((e) => seen.push(e));
      // An editor saving by rename: the file is gone for less than two polls.
      for (let i = 0; i < 5; i++) {
        rmSync(settingsPath(serverData));
        await new Promise((r) => setTimeout(r, 60));
        writeSettings({ privacy: { diagnostics: "off" } });
        await new Promise((r) => setTimeout(r, 120));
      }
      expect(seen).toEqual([]);
      expect(d.enabled()).toBe(false);
      // Removed for good: the defaults apply after the polls agree.
      rmSync(settingsPath(serverData));
      await until(() => d.enabled());
      expect(seen).toEqual([true]);
    }, 50);
  });

  test("env_overrides_settings and server_info_reflects_gate", async () => {
    writeSettings({ privacy: { diagnostics: "off" } });
    const { info } = await start({ FIN_DIAGNOSTICS: "on" });
    expect(info).toMatchObject({ diagnostics: { enabled: true } });
    const c = await client("console");
    const consent = ok<ConsentState>(await confirmed(c, "diagnostics.consent", { purpose: "diagnostics", value: "off" }));
    // The setting is written, but the environment still decides.
    expect(consent).toMatchObject({ diagnostics: "on", basis: "env", env: ["diagnostics"] });
    expect(JSON.parse(readFileSync(settingsPath(serverData), "utf8")).privacy).toEqual({ diagnostics: "off" });
  });

  test("do_not_track_forces_off; finisherd.json follows the gate", { timeout: 20_000 }, async () => {
    await start({ DO_NOT_TRACK: "1" });
    expect(server!.info).toMatchObject({ diagnostics: { enabled: false } });
    const c = await client();
    const s = ok<DiagnosticsReport>(await c.call("diagnostics.status", {}));
    expect(s.consent).toMatchObject({ diagnostics: "off", training: "off", doNotTrack: true });
    await Promise.all(clients.splice(0).map((x) => x.close()));
    await server!.stop();
    server = undefined;
    expect(existsSync(join(serverData, "events"))).toBe(false);

    writeSettings({ privacy: { diagnostics: "off" } });
    await start();
    expect(server!.info.diagnostics.enabled).toBe(false);
    writeSettings({ privacy: { diagnostics: "on" } });
    const file = join(serverData, "finisherd.json");
    await until(() => JSON.parse(readFileSync(file, "utf8")).diagnostics.enabled === true);
  });
});

describe("sessions and errors", () => {
  test.skipIf(WIN)("session_start_end_and_unclean", { timeout: 20_000 }, async () => {
    await start();
    await server!.stop();
    server = undefined;
    // The first start also journals the bundled plugins it installs (`plugin.lifecycle`, 8A Task 10).
    const sessionKinds = () => onDisk().map((e) => e.kind).filter((k) => k.startsWith("session."));
    let kinds = sessionKinds();
    expect(kinds).toEqual(["session.start", "session.end"]);
    const state = JSON.parse(readFileSync(join(serverData, "events", "state.json"), "utf8"));
    expect(state.openSession).toBeUndefined();

    await start();
    // Ready means the writer confirmed the start; the test checks it rather than trusting it.
    await until(() => onDisk().filter((e) => e.kind === "session.start").length === 2, 2000);
    const open = JSON.parse(readFileSync(join(serverData, "events", "state.json"), "utf8")).openSession;
    expect(open).toBe(onDisk().at(-1)!.session);
    server!.child.kill("SIGKILL");
    await new Promise((r) => server!.child.once("exit", r));
    server = undefined;
    rmSync(join(serverData, "finisherd.json"), { force: true });
    await start();
    await server!.stop();
    server = undefined;
    kinds = sessionKinds();
    expect(kinds.slice(2)).toEqual(["session.start", "session.unclean", "session.start", "session.end"]);
    const sessions = onDisk().filter((e) => e.kind === "session.start").map((e) => e.session);
    expect(onDisk().find((e) => e.kind === "session.unclean")?.payload).toEqual({ previous: sessions[1] });
  });

  const crash = (extra: Record<string, string>) => {
    const diag = pathToFileURL(join(import.meta.dirname, "../src/diagnostics.ts")).href;
    const node = pathToFileURL(join(import.meta.dirname, "../../../packages/fin-node/index.js")).href;
    const script = `
      const { Journal } = await import(${JSON.stringify(node)});
      const { installUncaughtHandlers } = await import(${JSON.stringify(diag)});
      const j = Journal.open(${JSON.stringify(serverData)}, null, "test-host");
      installUncaughtHandlers(j);
      setTimeout(() => { throw new Error("boom in " + ${JSON.stringify(tmp)}); }, 10);
    `;
    return spawnSync(process.execPath, ["--input-type=module", "-e", script], { encoding: "utf8", env: env(extra) });
  };
  // The handler records within 250 ms and gives up past that (a crash must never hang on its
  // journal), which a loaded machine can overrun: run it again, up to five times, until one of
  // the crashes was recorded. What the recorded event holds is then asserted as ever.
  const crashRecorded = (extra: Record<string, string>) => {
    let r = crash(extra);
    for (let i = 1; i < 5 && !onDisk().some((e) => e.kind === "error.panic"); i++) r = crash(extra);
    return r;
  };

  test("uncaught_exception_is_error_panic_meta_only_under_diagnostics", () => {
    const r = crashRecorded({});
    expect(r.status).not.toBe(0);
    expect(r.stderr).toContain("boom in");
    const [p] = onDisk().filter((e) => e.kind === "error.panic");
    expect(p).toBeDefined();
    expect(p!.source).toBe("finisherd");
    const payload = p!.payload as Record<string, unknown>;
    expect(payload).toMatchObject({ where: "finisherd", thread: "main" });
    expect(payload.messageHash).toMatch(/^k1:/);
    expect(payload.message).toBeUndefined();
    // The throwing frame is the inline script, not ours: no location.
    expect(payload.thirdParty).toBe(true);
    expect(JSON.stringify(onDisk())).not.toContain(tmp);
  });

  test("uncaught_message_is_content_with_training", () => {
    crashRecorded({ FIN_TRAINING: "on" });
    const [p] = onDisk().filter((e) => e.kind === "error.panic");
    const message = (p!.payload as { message: string }).message;
    expect(message).toMatch(/^Error: boom in /);
    expect(message).not.toContain(tmp);
    expect(p!.consent.purposes).toContain("training");
  });

  test("locationOf keeps only repo-relative frames", () => {
    // Native paths: `/repo` on POSIX, `<drive>:\repo` on Windows (a drive-less file URL is
    // not a Windows path).
    const root = resolve("/repo") + sep;
    const main = pathToFileURL(join(root, "apps", "finisherd", "src", "main.ts")).href;
    const dep = join(root, "node_modules", "y", "z.js");
    const e = { stack: `Error: x\n    at f (node:internal/x:1:1)\n    at g (${dep}:2:3)\n    at h (${main}:12:5)` };
    expect(locationOf(e, root)).toBe("apps/finisherd/src/main.ts:12");
    expect(locationOf({ stack: `Error\n    at ${resolve("/elsewhere/a.js")}:1:1` }, root)).toBeUndefined();
  });
});

describe("rpc timing", () => {
  test("rpc_timing_reaches_rpc_stats and rpc_stats_excludes_streams", { timeout: 20_000 }, async () => {
    await start({ FIN_TEST_JOURNAL_STATS_MS: "200" });
    const c = await client();
    for (let i = 0; i < 3; i++) ok(await c.call("server.hello", {}));
    ok(await c.call("fonts.status", {}));
    const s = c.stream("project.events", { projectId: "01jb0000000000000000000001" }, () => undefined);
    await s.done;
    await new Promise((r) => setTimeout(r, 400));
    ok(await c.call("server.hello", {})); // rolls the window
    await until(async () => (await exported(c, { kinds: ["rpc.stats"] })).length >= 2);
    const stats = await exported(c, { kinds: ["rpc.stats"] });
    const keys = stats.map((e) => (e.payload as { key: string }).key);
    expect(keys).toContain("server.hello");
    expect(keys).toContain("fonts.status");
    expect(keys).not.toContain("project.events");
    // The calls may straddle windows: one entry per key per window.
    const hellos = stats.filter((e) => (e.payload as { key: string }).key === "server.hello");
    const count = hellos.reduce((n, e) => n + (e.payload as { histogram: { count: number } }).histogram.count, 0);
    expect(count).toBeGreaterThanOrEqual(3);
  });

  test("rpc_methods_table_matches_capabilities", () => {
    expect(rpcMethods().filter((m) => m !== "other").sort()).toEqual([...CAPABILITIES].sort());
    expect([...FinRpcs.requests.keys()].sort()).toEqual([...CAPABILITIES].sort());
  });
});

describe("sources and diagnostics.record", () => {
  test("record_rpc_refuses_unlisted_kind and record_rpc_size_cap", async () => {
    await start();
    const c = await client();
    const unlisted = await record(c, { record: "edit" }, "log.commit");
    expect(!unlisted.ok && unlisted.error.code).toBe("EVENT_INVALID");
    const unknown = await record(c, {}, "no.suchKind");
    expect(!unknown.ok && unknown.error.code).toBe("EVENT_KIND_UNKNOWN");
    const badKv = await c.call("diagnostics.record", { kind: "cli.command", kv: 1.5, payload: cli() });
    expect(badKv.ok).toBe(false);
    const big = await record(c, { ...cli(), padding: "x".repeat(70 * 1024) });
    expect(!big.ok && big.error.code).toBe("EVENT_TOO_LARGE");
    const bad = await record(c, { name: 1 });
    expect(!bad.ok && bad.error.code).toBe("EVENT_INVALID");
  });

  test("record_rate_limited_per_local_connection", async () => {
    await start();
    const [a, b] = [await client(), await client()];
    const recorded = async (c: NdjsonClient, n: number, tag: string) => {
      const rs = await Promise.all(Array.from({ length: n }, (_, i) => record(c, cli(`${tag}${i}`))));
      return rs.map((r) => ok<{ recorded: boolean }>(r).recorded).filter(Boolean).length;
    };
    // The first connection spends its burst; a second `fin` has its own (review of Task 7, M1:
    // every `fin` is `local`, so one looping client must not silence the others).
    // Its bucket (burst 200, 50 a second) refills while the calls are on their way, and on a
    // loaded machine that takes long: the bound is the burst plus what the elapsed time refilled.
    const t0 = performance.now();
    const first = await recorded(a, 260, "a");
    const refill = Math.ceil((50 * (performance.now() - t0)) / 1000);
    expect(first).toBeLessThanOrEqual(200 + refill + 10);
    expect(await recorded(b, 60, "b")).toBeGreaterThanOrEqual(55);
  });

  test("local_actor_from_headers_validates_actor", async () => {
    expect(actorFor(LOCAL, { "fin-actor-kind": "agent", "fin-actor-session": "s1", "fin-actor-call": "c".repeat(300) })).toEqual({
      actor: { kind: "agent", session: "s1" },
    });
    expect(actorFor(LOCAL, [["fin-actor-kind", "root"]])).toEqual({ actor: { kind: "unknown" } });
    expect(actorFor(LOCAL, undefined)).toEqual({ actor: { kind: "unknown" } });
    // `local` may claim only a person or an agent.
    expect(actorFor(LOCAL, { "fin-actor-kind": "system" })).toEqual({ actor: { kind: "unknown" } });
    expect(actorFor(LOCAL, { "fin-actor-kind": "plugin" })).toEqual({ actor: { kind: "unknown" } });
    await start();
    const c = await client();
    const headers: Array<[string, string]> = [
      ["fin-actor-kind", "agent"],
      ["fin-actor-session", "01JC00000000000000000000AA"],
      ["fin-actor-call", "tool-call-7"],
    ];
    ok(await record(c, cli(), "cli.command", headers));
    const [e] = await exported(c, { kinds: ["cli.command"] });
    expect(e!.ctx?.actor).toEqual({ kind: "agent", session: "01JC00000000000000000000AA", call: expect.stringMatching(/^k1:/) });
    expect(e!.session).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    expect(e!.session).not.toBe(onDisk()[0]?.session);
  });

  test("TokenBucket refills at its rate", () => {
    let t = 0;
    const b = new TokenBucket(50, 2, () => t);
    expect([b.take(), b.take(), b.take()]).toEqual([true, true, false]);
    t += 20;
    expect(b.take()).toBe(true);
  });
});

describe("diagnostics RPCs", () => {
  test("diagnostics_status_export_purge_project_rpcs and export_pages_with_cursor_and_limit", async () => {
    await start();
    const c = await client();
    const projectId = ok<{ projectId: string }>(await c.call("project.create", { dir: join(tmp, "P"), name: "P", rate: "30" })).projectId;
    for (let i = 0; i < 5; i++) {
      ok(await c.call("diagnostics.record", { kind: "cli.command", kv: 1, ctx: { project: projectId }, payload: cli(String(i)) }));
    }
    const status = ok<DiagnosticsReport>(await c.call("diagnostics.status", {}));
    expect(status.state).toBe("on");
    expect(status.kinds["cli.command"]).toBe(5);
    expect(status.projects[projectId]).toBe("default");

    const page = ok<ExportPage>(await c.call("diagnostics.export", { filter: { kinds: ["cli.command"] }, limit: 2 }));
    expect(page.lines).toHaveLength(2);
    // `fin` is not a person: its export is redacted (5A Decision 22).
    expect(page.redacted).toBe(true);
    const second = ok<ExportPage>(await c.call("diagnostics.export", { filter: { kinds: ["cli.command"] }, limit: 2, cursor: page.next }));
    expect(second.lines[0]!.id > page.lines[1]!.id).toBe(true);
    expect(await exported(c, { kinds: ["cli.command"] })).toHaveLength(5);
    const badCursor = await c.call("diagnostics.export", { cursor: "nope" });
    expect(!badCursor.ok && badCursor.error.code).toBe("INVALID_ARGS");
    // The upload view reads closed files only: the active one is not in it.
    expect(ok<ExportPage>(await c.call("diagnostics.export", { asUpload: "diagnostics" })).lines).toEqual([]);

    expect(ok(await c.call("diagnostics.project", { projectId, collect: "never" }))).toEqual({ project: projectId, collect: "never" });
    expect(ok<DiagnosticsReport>(await c.call("diagnostics.status", {})).projects[projectId]).toBe("never");
    expect(await exported(c, { project: projectId })).toHaveLength(0);
    const project = JSON.parse(readFileSync(join(tmp, "P", "finisher.project.json"), "utf8"));
    expect(project.collect).toBe("never");
    // Clearing the flag and purging are a person's (5A Task 7): the console, with a challenge code.
    const person = await client("console");
    ok(await confirmed(person, "diagnostics.project", { projectId, collect: "default" }));

    const purged = ok<{ removed: number }>(await confirmed(person, "diagnostics.purge", { selector: "all" }));
    expect(purged.removed).toBeGreaterThan(0);
    expect(existsSync(join(serverData, "events", "key"))).toBe(false);
    const badSel = await confirmed(person, "diagnostics.purge", { selector: { project: "not-an-id" } });
    expect(!badSel.ok && badSel.error.code).toBe("INVALID_ARGS");
  });
});
