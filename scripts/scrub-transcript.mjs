// Raw Claude Code stream transcript -> committed fixture (plan 7D Task 15, Task 0 Step 6).
//
//   node scripts/scrub-transcript.mjs <raw.log> <fixture.jsonl>
//
// The raw log is what the recorder wrote: one JSON object per line, `{t, dir: "in"|"out"|"err",
// line|text}` for protocol traffic and `{t, note, v}` for the recorder's own notes. The fixture
// keeps only the protocol lines, each as `{"dir":"out"|"in","line":{...}}`:
//   - `out` lines are what the CLI wrote to stdout (the bytes `decodeLine` reads);
//   - `in` lines are what the host wrote to the CLI's stdin (initialize, user lines, answers to
//     `can_use_tool`, the interrupt), in order, so a test can replay the exchange.
// Notes (argv, environment, working directory), stderr and anything that is not a JSON object
// are dropped. Never commit a raw log: it holds the home directory, the account's e-mail,
// request ids and the sign-in's timestamps.
//
// The scrub is deterministic and fails closed: the home directory, user name and host name of
// the machine it runs on are detected at run time (never written into this file), every path
// below a home directory becomes a placeholder under `/srv/finisher`, e-mails, account labels
// and names become placeholders, each id kind gets a stable counter in order of first
// appearance, timestamps and rate-limit numbers are normalised, and the result is checked
// again before it is written: a leftover identity makes the run fail.
import { readFileSync, writeFileSync } from "node:fs";
import { homedir, hostname, userInfo } from "node:os";

/** The synthetic session id, a v7-shaped UUID as in the hand-written fixtures. */
export const SESSION_ID = "01890a5d-ac96-774b-bcce-b302099a8057";
export const EMAIL = "user@example.invalid";
export const TIMESTAMP = "2026-01-01T00:00:00.000Z";
/** A `resetsAt` in seconds: only the shape (an integer) is kept. */
export const RESETS_AT = 1795093200;
export const UTILIZATION = 0.5;
const PLAN_LABEL = "Claude Plan";
const SIGNATURE = "c2lnbmF0dXJl";
const REDACTED = "cmVkYWN0ZWQ=";

const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;
const EMAIL_RE = /[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
const ISO_TIME = /\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z\b/g;

/**
 * What a real id looks like: the prefix, then one unbroken run of 16+ letters and digits holding
 * a digit, and nothing id-like after it. A capability or enum name (`msg_lifecycle_v1`) has
 * underscores and no such run, so it survives.
 */
const idShape = (prefix) => new RegExp(`(?<![A-Za-z0-9_])${prefix}_(?=[A-Za-z]*\\d)[A-Za-z0-9]{16,}(?![A-Za-z0-9_])`, "g");
const ID_SHAPE = { toolu: idShape("toolu"), msg: idShape("msg"), req: idShape("req") };

const escapeRe = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Keys whose string value is an account label: replaced whole, wherever they occur. */
export const LABEL_KEYS = new Map([
  ["email", EMAIL],
  ["emailAddress", EMAIL],
  ["displayName", "User"],
  ["fullName", "User"],
  ["orgName", "Org"],
  ["organizationName", "Org"],
  ["orgId", "00000000-0000-4000-8000-0000000000ff"],
  ["organizationUuid", "00000000-0000-4000-8000-0000000000ff"],
  ["accountUuid", "00000000-0000-4000-8000-0000000000fe"],
  ["subscriptionType", PLAN_LABEL],
  ["planTier", "plan"],
]);
const TIME_KEYS = new Set(["timestamp", "credential_expires_at", "loginExpiresAt"]);
const COUNTER_KEYS = new Set(["pid", "messaging_pid"]);

/** The identity of the machine the scrub runs on: what must not reach a fixture. */
export function detectIdentity() {
  const names = new Set();
  const add = (s) => {
    if (typeof s === "string" && s.length >= 3) names.add(s);
  };
  let user = "";
  try {
    user = userInfo().username;
  } catch {
    // no passwd entry: the home directory's last segment still names the user
  }
  const home = homedir();
  add(user);
  add(home.split(/[\\/]/).filter(Boolean).at(-1));
  add(hostname());
  add(hostname().split(".")[0]);
  return { homes: [home].filter((h) => h.length > 1), names: [...names] };
}

/** One scrub: the maps are per transcript, so a fixture is a function of its raw log alone. */
class Scrubber {
  #id;
  #sessions = new Set();
  #counters = new Map();
  #maps = new Map();
  #homeRes;
  #nameRes;

  constructor(identity) {
    this.#id = identity;
    this.#homeRes = identity.homes.map((h) => new RegExp(`${escapeRe(h)}(?![A-Za-z0-9_-])`, "g"));
    // Whole-token match with `-`/`_`/`.` as part of a token, so `.claude-<user>` is caught as
    // well as a bare name; the replacement never reuses the name.
    this.#nameRes = identity.names.map((n) => new RegExp(escapeRe(n), "gi"));
  }

  /** The session ids a log mentions: what the CLI reports and what the host passed. */
  collectSessions(value) {
    if (Array.isArray(value)) value.forEach((v) => this.collectSessions(v));
    else if (value !== null && typeof value === "object") {
      for (const [k, v] of Object.entries(value)) {
        if (k === "session_id" && typeof v === "string") this.#sessions.add(v.toLowerCase());
        else this.collectSessions(v);
      }
    }
  }

  /** `prefix` + a zero-padded counter, stable per original value. */
  #counted(kind, original, make) {
    let m = this.#maps.get(kind);
    if (m === undefined) this.#maps.set(kind, (m = new Map()));
    let v = m.get(original);
    if (v === undefined) {
      const n = (this.#counters.get(kind) ?? 0) + 1;
      this.#counters.set(kind, n);
      m.set(original, (v = make(n)));
    }
    return v;
  }

  #uuid(raw) {
    const u = raw.toLowerCase();
    if (this.#sessions.has(u)) return SESSION_ID;
    return this.#counted("uuid", u, (n) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`);
  }

  /** Paths first (so a placeholder never contains a name), then ids, e-mails and names. */
  string(s) {
    let out = s;
    // The recorder's per-run scratch tree: <anything>/t0-<run>/<scenario>/<ws|cfg|home|tmp|settings>.
    out = out.replace(/[^\s"'\\`<>]*?\/t0-[A-Za-z0-9_]+\/[A-Za-z0-9_-]+\/(ws|cfg|home|tmp|settings)(?![A-Za-z0-9_-])/g, (_, d) => (d === "ws" ? "/srv/finisher/workspace/s1" : `/srv/finisher/agents/s1/${d === "cfg" ? "provider" : d}`));
    for (const re of this.#homeRes) out = out.replace(re, "/srv/finisher/home");
    out = out.replace(/(?<![A-Za-z0-9_.-])\/(?:home|Users)\/[^/\s"'\\`<>]+/g, "/srv/finisher/home").replace(/[A-Za-z]:\\+Users\\+[^\\\s"'`<>]+/g, "/srv/finisher/home");
    out = out.replace(/cc-socks-\d+/g, "cc-socks-uid").replace(/\/cc-socks-uid\/\d+\.sock/g, "/cc-socks-uid/1.sock");
    out = out.replace(UUID, (m) => this.#uuid(m));
    out = out.replace(ID_SHAPE.toolu, (m) => this.#counted("toolu", m, (n) => `toolu_${String(n).padStart(4, "0")}`));
    out = out.replace(ID_SHAPE.msg, (m) => this.#counted("msg", m, (n) => `msg_${String(n).padStart(4, "0")}`));
    out = out.replace(ID_SHAPE.req, (m) => this.#counted("req", m, (n) => `req_${String(n).padStart(4, "0")}`));
    out = out.replace(EMAIL_RE, EMAIL);
    out = out.replace(ISO_TIME, TIMESTAMP);
    for (const re of this.#nameRes) out = out.replace(re, "user");
    return out;
  }

  /** The value with every string scrubbed and the keyed fields replaced. */
  value(v, key) {
    if (typeof v === "string") {
      if (LABEL_KEYS.has(key)) return LABEL_KEYS.get(key);
      if (TIME_KEYS.has(key)) return TIMESTAMP;
      if (key === "signature") return SIGNATURE;
      return this.string(v);
    }
    if (typeof v === "number") {
      if (key === "resetsAt") return RESETS_AT;
      if (key === "utilization") return UTILIZATION;
      if (COUNTER_KEYS.has(key)) return 1;
      return v;
    }
    if (Array.isArray(v)) return v.map((x) => this.value(x, key));
    if (v !== null && typeof v === "object") {
      // A redacted thinking block's payload is opaque model state.
      if (v.type === "redacted_thinking" && typeof v.data === "string") return { ...this.object({ ...v, data: undefined }), data: REDACTED };
      return this.object(v);
    }
    return v;
  }

  object(o) {
    const out = {};
    for (const [k, v] of Object.entries(o)) {
      if (v === undefined) continue;
      out[this.string(k)] = this.value(v, k);
    }
    return out;
  }
}

/**
 * The `initialize` answer's descriptions of the CLI's own agents are Anthropic's prose, not
 * protocol: only their names (and model) are kept.
 */
function trimInitialize(line) {
  const body = line?.response?.response;
  if (line?.type !== "control_response" || body === null || typeof body !== "object" || !Array.isArray(body.agents)) return line;
  body.agents = body.agents.map((a) => ({ name: a.name, description: "", ...(a.model === undefined ? {} : { model: a.model }) }));
  return line;
}

/** The field each streamed delta type carries its text in. */
const DELTA_FIELDS = new Map([
  ["input_json_delta", "partial_json"],
  ["text_delta", "text"],
  ["thinking_delta", "thinking"],
]);

/**
 * A block streams as pieces cut anywhere (`input_json_delta`, `text_delta`, `thinking_delta`),
 * so a path or a name can be split across lines and no single piece shows it whole. Each block's
 * pieces are joined and scrubbed as one string. A tool input always collapses into its first
 * piece (the pieces mean nothing alone, the complete `assistant` line has the input, and a fixed
 * cut could land inside a placeholder); text and thinking collapse the same way only when the
 * whole has something to scrub, else the original cuts stay. What is written is checked again
 * as a whole: a leftover identity fails the run.
 */
function rewriteDeltas(records, lines, s, identity) {
  const groups = new Map();
  let message = 0;
  records.forEach((r, i) => {
    const ev = r.dir === "out" && r.line.type === "stream_event" ? r.line.event : null;
    if (ev?.type === "message_start") message++;
    const field = ev?.type === "content_block_delta" ? DELTA_FIELDS.get(ev.delta?.type) : undefined;
    if (field !== undefined && typeof ev.delta[field] === "string") {
      const key = `${message}:${ev.index}:${ev.delta.type}`;
      if (!groups.has(key)) groups.set(key, { field, collapse: field === "partial_json", at: [] });
      groups.get(key).at.push(i);
    }
  });
  for (const { field, collapse, at } of groups.values()) {
    const original = at.map((i) => records[i].line.event.delta[field]).join("");
    const whole = s.string(original);
    if (collapse || whole !== original) {
      at.forEach((i, n) => {
        lines[i].line.event.delta[field] = n === 0 ? whole : "";
      });
    }
    assertScrubbed(at.map((i) => lines[i].line.event.delta[field]).join(""), identity);
  }
}

/** Whether the protocol line is worth keeping: a JSON object with a `type`. */
const keep = (rec) => (rec.dir === "out" || rec.dir === "in") && rec.line !== null && typeof rec.line === "object" && typeof rec.line.type === "string";

/** `rec.line` or `rec.text` as an object, or null. */
function parsed(rec) {
  const raw = rec.line ?? rec.text;
  if (raw !== null && typeof raw === "object") return raw;
  if (typeof raw !== "string") return null;
  try {
    const v = JSON.parse(raw);
    return v !== null && typeof v === "object" && !Array.isArray(v) ? v : null;
  } catch {
    return null;
  }
}

/** Throws when `text` still names a home directory, e-mail, user or host of this machine. */
export function assertScrubbed(text, identity = detectIdentity()) {
  const bad = [];
  if (/(?<![A-Za-z0-9_.-])\/(?:home|Users)\/|[A-Za-z]:\\+Users/.test(text)) bad.push("a home directory");
  for (const m of text.match(EMAIL_RE) ?? []) if (m !== EMAIL) bad.push("an e-mail");
  for (const h of identity.homes) if (text.includes(h)) bad.push("this machine's home directory");
  for (const n of identity.names) if (new RegExp(escapeRe(n), "i").test(text)) bad.push("this machine's user or host name");
  if (bad.length > 0) throw new Error(`scrub left ${[...new Set(bad)].join(", ")} in the fixture`);
}

/** The fixture's text for the raw log's text. */
export function scrubTranscript(raw, identity = detectIdentity()) {
  const records = [];
  for (const l of raw.split("\n")) {
    if (l.trim() === "") continue;
    let rec;
    try {
      rec = JSON.parse(l);
    } catch {
      continue;
    }
    if (rec === null || typeof rec !== "object" || "note" in rec) continue;
    const line = parsed(rec);
    if (line === null) continue;
    const r = { dir: rec.dir, line };
    if (keep(r)) records.push(r);
  }
  const s = new Scrubber(identity);
  for (const r of records) s.collectSessions(r.line);
  const lines = records.map((r) => ({ dir: r.dir, line: s.value(trimInitialize(r.line)) }));
  rewriteDeltas(records, lines, s, identity);
  const out = lines.map((l) => JSON.stringify(l));
  const text = `${out.join("\n")}\n`;
  assertScrubbed(text, identity);
  return text;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [src, dst] = process.argv.slice(2);
  if (src === undefined || dst === undefined) {
    console.error("usage: node scripts/scrub-transcript.mjs <raw.log> <fixture.jsonl>");
    process.exit(2);
  }
  writeFileSync(dst, scrubTranscript(readFileSync(src, "utf8")));
}
