// Claude Code's recorded-transcript contract suite (plan 7D Task 15, Decisions 9 and 13).
//
// `fixtures/claude/<version>/<scenario>.jsonl` are real stdout/stdin transcripts of that CLI
// version (recorded signed in, scrubbed by `scripts/scrub-transcript.mjs`, never raw). Each line
// is `{"dir":"out"|"in","line":{…}}`: `out` is what the CLI wrote, `in` what the host wrote.
// `<scenario>.expected.json` is the committed `ProviderEvent` sequence the adapter's decode and
// map must make of it. Regenerate one only after reading the diff against the recorded
// behaviour: `UPDATE_CONTRACT_EXPECTED=1 pnpm vitest run test/agent-claude-contract.test.ts`.
//
// Replay: every `out` line goes through `decodeLine`, then `LineMapper` (or, for a control
// request, the permission port `claudePermissions`); every recorded host answer to a
// `can_use_tool` must be what `port.respond` makes of the same decision, and every host user
// line and interrupt what `encodeUser` / `encodeControl` write, so a CLI version that stops
// accepting what Finisher sends, or sends what Finisher no longer reads, fails here.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { hostname, homedir, userInfo } from "node:os";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";
import { FinError, ProviderEvent } from "@finisher/contracts";
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import { CLAUDE_FLOOR, compareVersions } from "../src/agent/claude/detect.ts";
import { LineMapper } from "../src/agent/claude/map.ts";
import { parseClaudeInitialize } from "../src/agent/catalogue/cli-models.ts";
import { claudePermissions } from "../src/agent/claude/permissions.ts";
import { TESTED_VERSIONS } from "../src/agent/claude/probe-check.ts";
import { decodeLine, encodeControl, encodeUser } from "../src/agent/claude/protocol.ts";
import { ClaudeUsage, type UsageTotals } from "../src/agent/claude/usage.ts";

/** The floor never goes below the `--setting-sources` leak fix (Decision 9, changelog 2.1.314). */
const CHANGELOG_FLOOR = "2.1.314";
/** The version Task 0's signed-in half validated (Decision 9: the floor is never below it). */
const TASK0_VALIDATED = "2.1.321";
/** The synthetic session id the scrubber writes. */
const SESSION_ID = "01890a5d-ac96-774b-bcce-b302099a8057";

const ROOT = fileURLToPath(new URL("./fixtures/claude/", import.meta.url));
const decodeEvent = Schema.decodeUnknownSync(ProviderEvent);

interface Rec {
  readonly dir: "out" | "in";
  readonly line: Record<string, unknown>;
}

const versions = (): string[] =>
  readdirSync(ROOT, { withFileTypes: true })
    .filter((d) => d.isDirectory() && /^\d+\.\d+\.\d+$/.test(d.name))
    .map((d) => d.name)
    .sort(compareVersions);
const scenarios = (version: string): string[] =>
  readdirSync(`${ROOT}${version}`)
    .filter((f) => f.endsWith(".jsonl"))
    .map((f) => f.slice(0, -".jsonl".length))
    .sort();
const recording = (version: string, scenario: string): Rec[] =>
  readFileSync(`${ROOT}${version}/${scenario}.jsonl`, "utf8")
    .split("\n")
    .filter((l) => l !== "")
    .map((l) => JSON.parse(l) as Rec);
const expectedPath = (version: string, scenario: string) => `${ROOT}${version}/${scenario}.expected.json`;
const committed = (version: string, scenario: string): unknown[] => JSON.parse(readFileSync(expectedPath(version, scenario), "utf8")) as unknown[];

interface Replayed {
  readonly events: ProviderEvent[];
  /** What the host wrote that the adapter's encoders did not reproduce (empty when all match). */
  readonly hostMismatches: string[];
  /** The `out` lines `decodeLine` read as unknown (a line type the adapter does not know). */
  readonly unknownLines: string[];
  /** The `can_use_tool` requests, the host's recorded decision on each, in order. */
  readonly decisions: { readonly tool: string; readonly behavior: string }[];
  /** `user` lines the host wrote after the CLI's first output, and how many `result`s there were. */
  readonly hostUserLines: number;
  readonly results: number;
  /** The CLI's cumulative totals at its last `result`: what a process resumed after it counts from. */
  readonly cliTotals: UsageTotals;
}

/** The first host line's id never matters: the adapter's own is `finisher-initialize`. */
const asObject = (v: unknown): Record<string, unknown> => v as Record<string, unknown>;

function replay(recs: readonly Rec[], resumedFrom?: UsageTotals): Replayed {
  const port = claudePermissions();
  // A resumed process counts the totals the CLI restored from the last process's baseline
  // (adapter.ts: `restoredOffset` of `provider.pid`'s last baseline); a fresh one from zero.
  const usage = new ClaudeUsage(resumedFrom === undefined ? {} : { resumed: true, resumeTotals: "restored", baseline: resumedFrom });
  const mapper = new LineMapper(usage, { asked: (d) => port.asked?.(d) === true, turnEnded: () => port.turnEnded?.() });
  const events: ProviderEvent[] = [];
  const hostMismatches: string[] = [];
  const unknownLines: string[] = [];
  const decisions: { tool: string; behavior: string }[] = [];
  /** The CLI request id -> the runtime's id, for the prompts waiting on the host. */
  const waiting = new Map<string, { runtimeId: string; tool: string }>();
  /** The answers the adapter itself gives at once (the SDK server's handshake), by request id. */
  const immediate = new Map<string, string>();
  let interrupting = false;
  let hostUserLines = 0;
  let results = 0;

  for (const rec of recs) {
    const raw = JSON.stringify(rec.line);
    if (rec.dir === "out") {
      const l = decodeLine(raw);
      if (l instanceof FinError) throw new Error(`recorded line does not decode: ${l.code}: ${l.message} ${raw.slice(0, 200)}`);
      if (l.kind === "unknown") unknownLines.push(l.type);
      if (l.kind === "controlRequest") {
        const r = port.request(l.request_id, l.request);
        events.push(...(r.events ?? []));
        if (r.answer !== undefined) immediate.set(l.request_id, r.answer);
        for (const e of r.events ?? []) {
          if (e.type === "request" && l.request.subtype === "can_use_tool") waiting.set(l.request_id, { runtimeId: e.id, tool: l.request.tool_name });
        }
        continue;
      }
      if (l.kind === "controlCancel") {
        events.push(...port.cancel(l.request_id));
        continue;
      }
      if (l.kind === "result") results++;
      events.push(...mapper.map(l, interrupting));
      if (l.kind === "result") interrupting = false;
      continue;
    }

    // A host line: it must be what the adapter's own encoders write for the same intent.
    const type = rec.line.type;
    if (type === "user") {
      hostUserLines++;
      const content = asObject(rec.line.message).content as { text: string }[];
      const mine = JSON.parse(encodeUser(content.map((c) => c.text), String(rec.line.session_id))) as unknown;
      if (!isDeepStrictEqual(mine, rec.line)) hostMismatches.push("user");
    } else if (type === "control_request") {
      const req = asObject(rec.line.request);
      if (req.subtype === "interrupt") {
        interrupting = true;
        if (!isDeepStrictEqual(JSON.parse(encodeControl(String(rec.line.request_id), { subtype: "interrupt" })), rec.line)) hostMismatches.push("interrupt");
      } else if (req.subtype === "initialize") {
        const mine = JSON.parse(encodeControl(String(rec.line.request_id), { subtype: "initialize" })) as Record<string, unknown>;
        if (mine.type !== "control_request" || asObject(mine.request).subtype !== "initialize") hostMismatches.push("initialize");
      } else hostMismatches.push(`control_request/${String(req.subtype)}`);
    } else if (type === "control_response") {
      const resp = asObject(rec.line.response);
      const id = String(resp.request_id);
      const w = waiting.get(id);
      if (w !== undefined) {
        waiting.delete(id);
        const body = asObject(resp.response);
        const allow = body.behavior === "allow";
        decisions.push({ tool: w.tool, behavior: String(body.behavior) });
        const answer = port.respond(w.runtimeId, { kind: "approval", allow, ...(allow ? {} : { message: String(body.message) }) });
        if (answer instanceof FinError) throw answer;
        if (!isDeepStrictEqual(JSON.parse(answer), rec.line)) hostMismatches.push(`can_use_tool answer ${id}`);
      } else if (immediate.has(id)) {
        // The recorder answered the SDK server's handshake with its own stub (a fixed tool list),
        // so only what the CLI depends on is compared: a success for this request id whose
        // `mcp_response` echoes the JSON-RPC id of the CLI's message and, for `initialize`,
        // negotiates the same protocol version.
        const mine = asObject(asObject(JSON.parse(immediate.get(id)!)).response);
        const theirs = asObject(resp);
        const a = asObject(asObject(mine.response).mcp_response);
        const b = asObject(asObject(theirs.response).mcp_response);
        const sameVersion = asObject(b.result).protocolVersion === undefined || asObject(a.result).protocolVersion === asObject(b.result).protocolVersion;
        if (mine.subtype !== "success" || mine.request_id !== id || (a.id ?? null) !== (b.id ?? null) || !sameVersion) hostMismatches.push(`mcp answer ${id}`);
        immediate.delete(id);
      }
      // The answer to the host's own `initialize` is an `out` control_response: nothing to match.
    }
  }
  return { events, hostMismatches, unknownLines, decisions, hostUserLines, results, cliTotals: usage.cliTotals() };
}

/** One scenario's replay; `resume-2` continues `resume-1`'s transcript, as the adapter does after a clean exit. */
const run = (version: string, scenario: string): Replayed => replay(recording(version, scenario), scenario === "resume-2" ? replay(recording(version, "resume-1")).cliTotals : undefined);

const all = (): { version: string; scenario: string }[] => versions().flatMap((version) => scenarios(version).map((scenario) => ({ version, scenario })));

/** Whether one recording still decodes and maps to its committed expectation. */
function scenarioPasses(version: string, scenario: string): boolean {
  try {
    const r = run(version, scenario);
    for (const e of r.events) decodeEvent(e);
    return r.hostMismatches.length === 0 && r.unknownLines.length === 0 && isDeepStrictEqual(JSON.parse(JSON.stringify(r.events)), committed(version, scenario));
  } catch {
    return false;
  }
}

const SCENARIOS = ["api-retry", "interrupt", "not-logged-in", "permission", "resume-1", "resume-2", "steer", "two-turn-edit"];

describe("recorded transcripts", () => {
  test("every recorded version has every scenario", () => {
    expect(versions().length).toBeGreaterThan(0);
    for (const v of versions()) {
      expect(scenarios(v)).toEqual(SCENARIOS);
      for (const s of SCENARIOS) expect(() => committed(v, s), `${v}/${s}.expected.json`).not.toThrow();
    }
  });

  describe.each(all())("$version $scenario", ({ version, scenario }) => {
    const r = run(version, scenario);

    test("decoding and mapping yield exactly the committed ProviderEvent sequence", () => {
      for (const e of r.events) expect(() => decodeEvent(e)).not.toThrow();
      const actual = JSON.parse(JSON.stringify(r.events)) as unknown[];
      if (process.env.UPDATE_CONTRACT_EXPECTED === "1") writeFileSync(expectedPath(version, scenario), `${JSON.stringify(actual, null, 2)}\n`);
      expect(actual).toEqual(committed(version, scenario));
    });

    test("every line is a line the adapter knows, and the host's lines are the adapter's own", () => {
      expect(r.unknownLines).toEqual([]);
      expect(r.hostMismatches).toEqual([]);
    });
  });
});

describe("what each scenario shows (2.1.321)", () => {
  const events = (s: string) => run("2.1.321", s).events;
  const types = (s: string) => events(s).map((e) => e.type);
  const ends = (s: string) => events(s).filter((e) => e.type === "turnCompleted" || e.type === "turnFailed");

  test("two_turn_edit: two turns, an Edit and a Write asked and run, each turn ends endTurn", () => {
    expect(ends("two-turn-edit")).toEqual([
      { type: "turnCompleted", stopReason: "endTurn" },
      { type: "turnCompleted", stopReason: "endTurn" },
    ]);
    const tools = events("two-turn-edit").filter((e) => e.type === "toolStarted").map((e) => (e as { tool: string }).tool);
    expect(tools).toContain("Edit");
    expect(tools).toContain("Write");
    // Every tool call the model made was asked of the host first and came back ok.
    const asked = events("two-turn-edit").filter((e) => e.type === "request").length;
    expect(asked).toBe(tools.length);
    expect(events("two-turn-edit").filter((e) => e.type === "toolCompleted").every((e) => (e as { ok: boolean }).ok)).toBe(true);
    expect(run("2.1.321", "two-turn-edit").hostUserLines).toBe(2);
    // The second turn's cost is what that `result` added, not the process's running total.
    const usage = events("two-turn-edit").filter((e): e is Extract<ProviderEvent, { type: "usage" }> => e.type === "usage");
    expect(usage.every((u) => (u.costMicros ?? 0) >= 0)).toBe(true);
  });

  test("permission: the first call is allowed and runs, the second is denied with the person's message", () => {
    const r = run("2.1.321", "permission");
    expect(r.decisions.map((d) => d.behavior)).toEqual(["allow", "deny"]);
    const done = events("permission").filter((e): e is Extract<ProviderEvent, { type: "toolCompleted" }> => e.type === "toolCompleted");
    expect(done.map((d) => d.ok)).toEqual([true, false]);
    // The host answered the denied call itself, so the CLI's own-denial notice does not repeat it.
    expect(events("permission").some((e) => e.type === "notice" && e.name === "cliDenied")).toBe(false);
    expect(ends("permission")).toEqual([{ type: "turnCompleted", stopReason: "endTurn" }]);
  });

  test("steer: the line written while a call was pending JOINED the running turn", () => {
    // Task 0 (2.1.321, 2026-11-13) saw a mid-stream steer queue as the next turn; this recording,
    // a user line written while a `can_use_tool` was pending, joined the running turn: one
    // `result`, and the reply carries the steer's word. The adapter never depends on either:
    // steers are held host-side (session.ts, Decision 13) and written only at a `result`, so it
    // never writes a line mid-turn. This transcript pins what the CLI does if one ever arrives.
    const r = run("2.1.321", "steer");
    expect(r.hostUserLines).toBe(2);
    expect(r.results).toBe(1);
    expect(ends("steer")).toEqual([{ type: "turnCompleted", stopReason: "endTurn" }]);
    const said = events("steer").filter((e): e is Extract<ProviderEvent, { type: "message" }> => e.type === "message");
    expect(JSON.stringify(said.at(-1))).toContain("steered");
  });

  test("interrupt: the interrupted turn ends as interrupted, not failed, with the tool's result in order", () => {
    expect(ends("interrupt")).toEqual([{ type: "turnCompleted", stopReason: "interrupted" }]);
    const t = types("interrupt");
    expect(t.indexOf("request")).toBeLessThan(t.indexOf("toolCompleted"));
    expect(events("interrupt").filter((e) => e.type === "toolCompleted")).toEqual([expect.objectContaining({ ok: false })]);
    // Without Finisher's own interrupt in flight the same `result` is a failure (fail closed).
    const lines = recording("2.1.321", "interrupt").filter((x) => x.dir === "out");
    const m = new LineMapper(new ClaudeUsage());
    const last = lines.map((x) => decodeLine(JSON.stringify(x.line))).findLast((l) => !(l instanceof FinError) && l.kind === "result");
    expect(m.map(last as never, false).at(-1)).toMatchObject({ type: "turnFailed" });
  });

  test("resume: both processes end endTurn under the same session id; the resumed one reports the account's e-mail placeholder", () => {
    const one = recording("2.1.321", "resume-1");
    const two = recording("2.1.321", "resume-2");
    const sessions = (rs: Rec[]) => new Set(rs.filter((x) => x.dir === "out" && x.line.type === "system" && x.line.subtype === "init").map((x) => x.line.session_id));
    expect([...sessions(one)]).toEqual([SESSION_ID]);
    expect([...sessions(two)]).toEqual([SESSION_ID]);
    expect(ends("resume-1")).toEqual([{ type: "turnCompleted", stopReason: "endTurn" }]);
    expect(ends("resume-2")).toEqual([{ type: "turnCompleted", stopReason: "endTurn" }]);
    // Totals continue after a clean exit (Facts, 2026-11-13): the second `result` carries more cost.
    // Counted from the restored totals, the resumed process charges its own turn, not the
    // cumulative `total_cost_usd` (which includes the first process's spend): 155 then 193 µ$.
    const charged = (s: string) => events(s).filter((e): e is Extract<ProviderEvent, { type: "usage" }> => e.type === "usage").reduce((a, u) => a + (u.costMicros ?? 0), 0);
    expect(charged("resume-1")).toBe(155);
    expect(charged("resume-2")).toBe(193);
    const cost = (rs: Rec[]) => Number(rs.find((x) => x.dir === "out" && x.line.type === "result")!.line.total_cost_usd);
    expect(cost(two)).toBeGreaterThan(cost(one));
  });

  test("not_logged_in: the failed turn is PROVIDER_AUTH with the sign-in command as its fix", () => {
    expect(ends("not-logged-in")).toEqual([
      expect.objectContaining({ type: "turnFailed", error: expect.objectContaining({ code: "PROVIDER_AUTH", fix: "finisherd provider signin claudeCode", details: expect.objectContaining({ reason: "notLoggedIn" }) }) }),
    ]);
  });

  test("api_retry: two retry notices, then the failed turn", () => {
    const retries = events("api-retry").filter((e) => e.type === "notice" && e.name === "retry");
    expect(retries.map((e) => (e as { details: { attempt: number; code: string } }).details)).toEqual([expect.objectContaining({ attempt: 1, code: "PROVIDER_OVERLOADED" }), expect.objectContaining({ attempt: 2, code: "PROVIDER_OVERLOADED" })]);
    expect(ends("api-retry")).toHaveLength(1);
    expect(ends("api-retry")[0]).toMatchObject({ type: "turnFailed" });
  });

  test("initialize: the models list parses and the account carries no e-mail but the placeholder", () => {
    for (const s of SCENARIOS) {
      const init = recording("2.1.321", s).find((x) => x.dir === "out" && x.line.type === "control_response" && asObject(asObject(x.line.response).response).models !== undefined);
      const body = asObject(asObject(asObject(init!.line).response).response);
      expect(parseClaudeInitialize(body).length).toBeGreaterThan(0);
      expect(body.claude_code_version).toBe("2.1.321");
    }
  });
});

/** The scrubber's account-label fields and their placeholders: a fixture value must equal it exactly. */
const { LABEL_KEYS } = (await import(new URL("../../../scripts/scrub-transcript.mjs", import.meta.url).href)) as { LABEL_KEYS: ReadonlyMap<string, string> };
const PLACEHOLDER_EMAIL = "user@example.invalid";
const SYNTHETIC_UUID = /^(?:00000000-0000-4000-8000-[0-9a-f]{12}|01890a5d-ac96-774b-bcce-b302099a8057)$/;
const EMAIL_RE = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
/** An id of a recorded kind: a prefix, then one unbroken run of 16+ letters and digits holding a digit (capability names like `msg_lifecycle_v1` are not ids). */
const ID_LIKE = /(?<![A-Za-z0-9_])(?:msg|req|toolu)_(?=[A-Za-z]*\d)[A-Za-z0-9]{16,}(?![A-Za-z0-9_])/g;

const machineNames = (() => {
  const out = new Set<string>();
  const add = (n: string | undefined) => {
    if (n !== undefined && n.length >= 3) out.add(n.toLowerCase());
  };
  try {
    add(userInfo().username);
  } catch {
    // no passwd entry
  }
  add(homedir().split(/[\\/]/).filter(Boolean).at(-1));
  add(hostname());
  add(hostname().split(".")[0]);
  return [...out];
})();

/** Every `[key, value]` string pair in a parsed JSON value, nested to any depth. */
function* stringFields(v: unknown): Generator<[string, string]> {
  if (Array.isArray(v)) for (const x of v) yield* stringFields(x);
  else if (v !== null && typeof v === "object") {
    for (const [k, x] of Object.entries(v)) {
      if (typeof x === "string") yield [k, x];
      else yield* stringFields(x);
    }
  }
}

/** The JSON values in a fixture file: one per line for `.jsonl`, the whole text for `.json`. */
function jsonValues(name: string, raw: string): unknown[] {
  return name.endsWith(".jsonl") ? raw.split("\n").filter((l) => l !== "").map((l) => JSON.parse(l) as unknown) : [JSON.parse(raw) as unknown];
}

/** What is wrong with a fixture or expectation's text (empty when clean); never quotes a personal value. */
function scrubProblems(name: string, raw: string, names: readonly string[] = machineNames): string[] {
  const bad: string[] = [];
  if (/\/home\/|\/Users\/|[A-Za-z]:\\+Users/i.test(raw)) bad.push("a home directory");
  if (raw.includes(homedir())) bad.push("this machine's home directory");
  for (const m of raw.match(EMAIL_RE) ?? []) if (!/^(?:user@example\.invalid|cc-plugin-[a-z-]+@builtin)$/.test(m)) bad.push("an e-mail");
  for (const n of names) if (raw.toLowerCase().includes(n)) bad.push(`a name of this machine (${n.length} chars)`);
  for (const u of raw.match(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi) ?? []) if (!SYNTHETIC_UUID.test(u)) bad.push("a real uuid");
  if ((raw.match(ID_LIKE) ?? []).length > 0) bad.push("a real msg_/req_/toolu_ id");
  for (const p of raw.match(/"\/[A-Za-z0-9_.\-/]*"/g) ?? []) if (!/^"\/(?:srv\/finisher|tmp|usr|bin|dev|proc)\b/.test(p)) bad.push("a path outside the placeholders");
  for (const t of raw.match(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z/g) ?? []) if (t !== "2026-01-01T00:00:00.000Z") bad.push("a real timestamp");
  for (const r of raw.match(/"resetsAt":\s*[0-9.]+/g) ?? []) if (r !== '"resetsAt":1795093200') bad.push("a real resetsAt");
  if (/"(?:argv|env|note)"/.test(raw)) bad.push("a recorder note");
  for (const v of jsonValues(name, raw)) {
    for (const [k, x] of stringFields(v)) {
      const want = LABEL_KEYS.get(k);
      if (want !== undefined && x !== want) bad.push(`a non-placeholder ${k}`);
    }
  }
  return [...new Set(bad)];
}

/** Every file under `fixtures/claude/<version>/`: the transcripts and their expectations. */
const fixtureFiles = (): { version: string; file: string }[] => versions().flatMap((version) => readdirSync(`${ROOT}${version}`).sort().map((file) => ({ version, file })));

describe("fixtures_are_scrubbed", () => {
  test("the scrubber's label keys are the ones this check knows", () => {
    for (const k of ["email", "emailAddress", "subscriptionType", "orgId", "orgName", "displayName", "accountUuid", "organizationUuid"]) expect(LABEL_KEYS.has(k), k).toBe(true);
    expect(LABEL_KEYS.get("email")).toBe(PLACEHOLDER_EMAIL);
  });

  test("a dirty copy fails the check (account labels, e-mails, names, ids, paths)", () => {
    const line = (o: unknown) => `${JSON.stringify({ dir: "out", line: o })}\n`;
    const clean = line({ type: "control_response", response: { account: { email: PLACEHOLDER_EMAIL, subscriptionType: "Claude Plan", orgName: "Org", displayName: "User" } } });
    expect(scrubProblems("x.jsonl", clean, ["alice"])).toEqual([]);
    const dirty: [string, string, RegExp][] = [
      ["subscription type", line({ account: { subscriptionType: "Acme Gold" } }), /subscriptionType/],
      ["org name", line({ a: { b: { orgName: "Acme Inc" } } }), /orgName/],
      ["display name", line({ list: [{ displayName: "Alice A" }] }), /displayName/],
      ["org id", line({ orgId: "11111111-2222-4333-8444-555555555555" }), /orgId/],
      ["account e-mail field", line({ account: { email: "someone@corp.example" } }), /email/],
      ["e-mail anywhere", line({ text: "mail someone@corp.example" }), /e-mail/],
      ["machine name", line({ text: "built by Alice" }), /name of this machine/],
      ["home directory", line({ cwd: "/home/bob/w" }), /home directory/],
      ["real message id", line({ id: "msg_011Ch1i4Bed1aTV9K4SYsbsF" }), /id/],
    ];
    for (const [what, text, re] of dirty) expect(scrubProblems("x.jsonl", text, ["alice"]), what).toEqual(expect.arrayContaining([expect.stringMatching(re)]));
    // The same labels in an expectation file (`.json`) fail too.
    expect(scrubProblems("x.expected.json", JSON.stringify([{ details: { orgName: "Acme Inc" } }]), ["alice"])).toEqual(["a non-placeholder orgName"]);
  });

  test("capability names that look like ids are kept (msg_lifecycle_v1)", () => {
    for (const { version, scenario } of all()) expect(readFileSync(`${ROOT}${version}/${scenario}.jsonl`, "utf8"), `${version}/${scenario}`).toContain('"msg_lifecycle_v1"');
  });

  describe.each(fixtureFiles())("$version $file", ({ version, file }) => {
    const raw = readFileSync(`${ROOT}${version}/${file}`, "utf8");

    test("no personal data: paths, e-mails, names, ids, times, account labels, recorder notes", () => {
      expect(file).toMatch(/^[a-z0-9-]+(?:\.expected\.json|\.jsonl)$/);
      expect(scrubProblems(file, raw)).toEqual([]);
    });

    if (file.endsWith(".jsonl")) {
      test("every line is {dir, line}", () => {
        for (const l of raw.split("\n").filter((x) => x !== "")) {
          const o = JSON.parse(l) as Record<string, unknown>;
          expect(Object.keys(o).sort()).toEqual(["dir", "line"]);
          expect(["in", "out"]).toContain(o.dir);
        }
      });
    }
  });
});

describe("floor_is_lowest_passing_version", () => {
  test("CLAUDE_FLOOR is the highest of the changelog's 2.1.314, the version Task 0 validated and the lowest recorded version that passes (Decision 9)", () => {
    const passing = versions().filter((v) => scenarios(v).every((s) => scenarioPasses(v, s)));
    expect(passing.length).toBeGreaterThan(0);
    const lowest = passing[0]!;
    const floor = [CHANGELOG_FLOOR, TASK0_VALIDATED, lowest].reduce((a, b) => (compareVersions(b, a) > 0 ? b : a));
    expect(CLAUDE_FLOOR).toBe(floor);
    // Every version with a passing suite is a tested one (no `untestedVersion` notice).
    expect([...TESTED_VERSIONS]).toEqual(passing);
  });

  test("a recording that no longer maps to its expectation does not count", () => {
    // `scenarioPasses` is the suite's own judge: a changed expectation fails it.
    const rec = recording("2.1.321", "permission");
    const r = replay(rec);
    expect(isDeepStrictEqual(JSON.parse(JSON.stringify(r.events)), [...committed("2.1.321", "permission")].reverse())).toBe(false);
  });
});
