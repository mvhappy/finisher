// The napi `Journal` (plan D, Task D4): open under resolved settings, records, the report,
// paged export, purge, the project flags the engines set, and the chained panic hook.
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from "node:fs";
import { hostname, tmpdir } from "node:os";
import { join } from "node:path";
import type { ConsentState, DiagnosticsReport, Envelope, ExportPage, JournalStatus } from "@finisher/core-types";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { Engine, type FinNodeError, Journal, kindNames, validatePrivacy } from "../index.js";

let tmp: string;
let serverData: string;
let journal: Journal | undefined;
let engine: Engine | undefined;

beforeEach(() => {
  // Canonical: the journal registry is keyed by canonical server data, and `Journal.open` runs
  // before the folder exists (macOS's temp dir is a symlink, so the keys would differ).
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "fin-node-journal-")));
  serverData = join(tmp, "server-data");
});

afterEach(async () => {
  await engine?.close();
  engine = undefined;
  await journal?.close();
  journal = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

function open(privacy: unknown = null): Journal {
  journal = Journal.open(serverData, privacy === null ? null : JSON.stringify(privacy), hostname());
  return journal;
}

function thrown(f: () => unknown): FinNodeError {
  try {
    f();
  } catch (e) {
    return e as FinNodeError;
  }
  throw new Error("expected a throw");
}

async function all(j: Journal, filter: object = {}, asUpload?: "diagnostics" | "training"): Promise<Envelope[]> {
  const out: Envelope[] = [];
  let cursor: string | undefined;
  for (;;) {
    const page = JSON.parse(await j.export(JSON.stringify(filter), cursor, 1000, asUpload)) as ExportPage;
    out.push(...page.lines);
    if (page.next === undefined) return out;
    cursor = page.next;
  }
}

const START = JSON.stringify({
  app: "0.0.0",
  protocol: 1,
  os: "linux",
  osMajor: 6,
  arch: "x86_64",
  cores: 4,
  memGiB: 8,
  profile: "release",
});

const cli = (marker = "x") => ({
  name: "item.trim",
  flags: ["project", "dry-run"],
  argsHash: `blake3:${"ab".repeat(32)}`,
  micros: 1200,
  exit: 0,
  online: true,
  dryRun: false,
  txIds: [`idem-${marker}`],
});

describe("validatePrivacy and kindNames", () => {
  test("validatePrivacy names the first bad key and accepts good or absent sections", () => {
    expect(validatePrivacy("null")).toBeUndefined();
    expect(validatePrivacy(JSON.stringify({ diagnostics: "off", training: "on", retentionDays: 7, maxMiB: 64, extra: 1 }))).toBeUndefined();
    expect(validatePrivacy(JSON.stringify({ diagnostics: "off", retentionDays: 0 }))).toMatch(/^privacy\.retentionDays/);
    expect(validatePrivacy(JSON.stringify({ maxMiB: 1 }))).toMatch(/^privacy\.maxMiB/);
    expect(validatePrivacy(JSON.stringify({ training: "maybe" }))).toMatch(/^privacy\.training/);
    expect(validatePrivacy(JSON.stringify("on"))).toMatch(/not an object/);
  });

  test("kindNames lists the registry", () => {
    expect(kindNames()).toEqual(expect.arrayContaining(["cli.command", "error.panic", "log.commit", "session.start"]));
  });
});

describe("Journal", () => {
  test("opens under the defaults: diagnostics on, training undecided", () => {
    const j = open();
    expect(j.enabled).toBe(true);
    expect(j.openWarning).toBeUndefined();
    expect(j.session).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    const c = JSON.parse(j.consent()) as ConsentState;
    expect(c).toMatchObject({ diagnostics: "on", training: "undecided", basis: "default" });
  });

  test("off settings create nothing, even for a session start", async () => {
    const j = open({ diagnostics: "off" });
    expect(j.enabled).toBe(false);
    j.startSession(START);
    j.record("cli.command", 1, null, JSON.stringify(cli()), "fin");
    expect((JSON.parse(await j.status()) as DiagnosticsReport).state).toBe("off");
    expect(await j.endSession(1)).toBe(false);
    expect(existsSync(join(serverData, "events"))).toBe(false);
  });

  test("an unparsable settings file at open is off with a warning; applySettings keeps the last good", () => {
    const j = Journal.open(serverData, "{ not json", hostname());
    journal = j;
    expect(j.enabled).toBe(false);
    expect(j.openWarning).toMatch(/settings\.json does not parse/);
    expect(j.applySettings(JSON.stringify({ diagnostics: "on" }))).toBeUndefined();
    expect(j.enabled).toBe(true);
    expect(j.applySettings("{ broken")).toMatch(/keeping the last good/);
    expect(j.enabled).toBe(true);
    expect(j.applySettings(JSON.stringify({ retentionDays: 0 }))).toMatch(/retentionDays/);
    expect(JSON.parse(j.consent()).diagnostics).toBe("on");
  });

  test("records wire kinds keyed, refuses unknown kinds and foreign sources", async () => {
    const j = open();
    const session = "01JC0000000000000000000042";
    const ctx = { actor: { kind: "agent", session: "my-agent-session", call: "01JC0000000000000000000007" } };
    j.record("cli.command", 1, JSON.stringify(ctx), JSON.stringify(cli()), "fin", session);
    expect(thrown(() => j.record("no.such", 1, null, "{}", "fin")).code).toBe("EVENT_KIND_UNKNOWN");
    expect(thrown(() => j.record("log.commit", 1, null, "{}", "fin")).code).toBe("EVENT_INVALID");
    expect(thrown(() => j.record("cli.command", 1, "{\"bogus\":1}", JSON.stringify(cli()), "fin")).code).toBe("EVENT_INVALID");
    const e = (await all(j)).find((x) => x.kind === "cli.command")!;
    expect(e.source).toBe("fin");
    expect(e.session).toBe(session);
    expect(e.ctx?.actor).toEqual({ kind: "agent", session: expect.stringMatching(/^k1:/), call: "01JC0000000000000000000007" });
    const p = e.payload as { argsHash: string; txIds: string[]; flags: string[] };
    expect(p.argsHash).toMatch(/^k1:[0-9a-f]{32}$/);
    expect(p.txIds[0]).toMatch(/^k1:/);
    expect(p.flags).toEqual(["dry-run", "project"]);
  });

  test("status reports kinds; export pages; purge removes", async () => {
    const j = open();
    for (let i = 0; i < 5; i++) j.record("cli.command", 1, null, JSON.stringify(cli(String(i))), "fin");
    const report = JSON.parse(await j.status()) as DiagnosticsReport;
    expect(report.kinds["cli.command"]).toBe(5);
    expect(report.consent.diagnostics).toBe("on");
    const page = JSON.parse(await j.export("{}", null, 2)) as ExportPage;
    expect(page.lines).toHaveLength(2);
    expect(page.next).toBeDefined();
    expect(await all(j)).toHaveLength(5);
    const now = JSON.parse(j.statusNow()) as JournalStatus;
    expect(now.state).toBe("on");
    const purged = JSON.parse(await j.purge(JSON.stringify("all")));
    expect(purged.removed).toBe(5);
    expect(await all(j)).toHaveLength(0);
    const bad = await j.export('{"since":"soon"}', null, 2).catch((e: FinNodeError) => e);
    expect((bad as FinNodeError).code).toBe("INVALID_ARGS");
  });

  test("the training upload view holds only entries stamped training", async () => {
    // Recorded before the opt-in: stamped diagnostics alone.
    let j = open({ diagnostics: "on", training: "off" });
    for (let i = 0; i < 3; i++) j.record("cli.command", 1, null, JSON.stringify(cli(String(i))), "fin");
    await j.close();
    // Reopened with training on, the earlier file is closed (the upload view reads only those).
    j = open({ diagnostics: "on", training: "on" });
    expect((await all(j, {}, "diagnostics")).map((e) => e.consent.purposes)).toEqual([["diagnostics"], ["diagnostics"], ["diagnostics"]]);
    expect(await all(j, {}, "training")).toEqual([]);
    // Recorded with training on, metadata is stamped for both; its training view says training.
    for (let i = 0; i < 2; i++) j.record("cli.command", 1, null, JSON.stringify(cli(String(i))), "fin");
    await j.close();
    j = open({ diagnostics: "on", training: "on" });
    expect((await all(j, {}, "training")).map((e) => e.consent.purposes)).toEqual([["training"], ["training"]]);
  });

  test("an engine's project is known while open and unknown after close; setCollect never purges", async () => {
    const j = open();
    engine = await Engine.create(serverData, join(tmp, "Project"), "Demo", "30");
    const id = engine.projectId;
    await j.status();
    expect(JSON.parse(j.projectPolicies())).toEqual({ [id]: "default" });
    j.record("cli.command", 1, JSON.stringify({ project: id }), JSON.stringify(cli()), "fin");
    expect((await all(j, { project: id })).length).toBe(1);
    expect(JSON.parse(await engine.setCollect("never"))).toEqual({ project: id, collect: "never" });
    // The policy reaches the writer on its control channel; `status` waits for it to catch up
    // (`projectPolicies` is a snapshot that does not).
    await j.status();
    expect(JSON.parse(j.projectPolicies())[id]).toBe("never");
    expect(await all(j, { project: id })).toHaveLength(0);
    j.record("cli.command", 1, JSON.stringify({ project: id }), JSON.stringify(cli()), "fin");
    expect(await all(j, { project: id })).toHaveLength(0);
    await engine.setCollect("default");
    await engine.close();
    engine = undefined;
    // Closed: unknown again, and new entries for it are dropped.
    await j.status();
    expect(JSON.parse(j.projectPolicies())[id]).toBe("unknown");
    j.record("cli.command", 1, JSON.stringify({ project: id }), JSON.stringify(cli()), "fin");
    expect(await all(j, { project: id })).toHaveLength(0);
  });

  test("engines find a journal opened before its server data exists, behind a symlink", async () => {
    // The directory's canonical path differs from the one given (a link here; macOS temp dirs
    // are behind `/var` → `/private/var`, Windows canonical paths are `\\?\` ones).
    mkdirSync(join(tmp, "real"));
    symlinkSync(join(tmp, "real"), join(tmp, "link"), "junction");
    serverData = join(tmp, "link", "server-data");
    expect(existsSync(serverData)).toBe(false);
    const j = open();
    engine = await Engine.create(serverData, join(tmp, "Project"), "Demo", "30");
    await j.status();
    expect(JSON.parse(j.projectPolicies())).toEqual({ [engine.projectId]: "default" });
  });

  test("sessions: start, end and unclean", async () => {
    let j = open();
    expect(j.startSession(START, 1000)).toBe(true);
    // Acknowledged: on disk before any flush.
    expect(existsSync(join(serverData, "events", "state.json"))).toBe(true);
    const first = j.session;
    // Not ended: the next run reports it.
    await j.close();
    j = open();
    j.startSession(START);
    expect(await j.endSession(2)).toBe(true);
    const kinds = (await all(j)).map((e) => e.kind);
    expect(kinds).toEqual(["session.start", "session.unclean", "session.start", "session.end"]);
    const unclean = (await all(j)).find((e) => e.kind === "session.unclean");
    expect((unclean?.payload as { previous: string }).previous).toBe(first);
  });

  test("fin_node_panic_hook_chains_previous_and_skips_writer_thread", async () => {
    const j = open();
    const before = j.previousHookCallsForTest();
    expect(j.panicOnThreadForTest("fin-node-worker")).toBe(true);
    expect(j.previousHookCallsForTest()).toBe(before + 1);
    const p = (await all(j)).find((e) => e.kind === "error.panic")!;
    const payload = p.payload as Record<string, unknown>;
    expect(payload).toMatchObject({ where: "finNode", thread: "worker", thirdParty: false });
    // The test hook's own panic site (`journal/test_api.rs`).
    expect(payload.location).toMatch(/^crates\/fin-node\/src\/journal\/test_api\.rs:\d+$/);
    expect(payload.messageHash).toMatch(/^k1:/);
    expect(payload.message).toBeUndefined();
    expect(JSON.stringify(p)).not.toContain(tmpdir());

    // A panic on the writer thread: the hook records nothing there and the journal suspends.
    j.panicWriterForTest();
    j.record("cli.command", 1, null, JSON.stringify(cli()), "fin");
    const report = JSON.parse(await j.status()) as DiagnosticsReport;
    expect(report.state).toBe("suspended");
    expect(report.code).toBe("JOURNAL_PANIC");
  });
});
