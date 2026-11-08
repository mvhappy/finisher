// A remembered answer (sub-project 7B Task 5, Decision 12; spec §7.3): `agent.approval.answer`
// with `remember` adds a rule, never a standing grant: `allow` or `deny` of exactly the
// approval's command (its subject's, as the server wrote it), visible in `agent.rules.list`
// with its source (`approval:<id>`), revocable like any rule, audited `agent.rule.add {from:
// <approval id>, layer}`. Only a person answers (`agent.approve`, human-only), so only a
// person remembers. Where it is written (owner decision 2026-11-08):
//
// - `session`: the user's `local` layer (`control.db`, never travels), for that session only;
// - `projectLocal`: the `local` layer, for the session's project;
// - `project`: the SHARED `finisher.rules.json` beside `finisher.project.json` (it travels with
//   the project), written by the server as the person's answer: the file read, the rule
//   appended (its other content kept), then a temporary file renamed over it (atomic replace,
//   the file's mode kept). A `finisher.rules.json` that is not a regular file (a symlink, a
//   folder, a FIFO) is refused (`INVALID_REQUEST {reason: notRegular}`), never followed. A project trusted with the file's previous hash is trusted with the new one
//   (`project_trust.rulesHash` refreshed, audited `agent.project.trust`); otherwise the rule is
//   written and does not apply until a person trusts the project (`notice`), while the
//   answer's own one-time grant still does. A file that is not a rules file is not touched
//   (`INVALID_REQUEST`).
// - **Undo.** An answer refused after the write takes out only its own rule (by id): the file is
//   read again, so a rule another answer wrote meanwhile stays; with nothing in between the
//   previous bytes are put back. The trust ends covering what it covered before and nothing a
//   person did not review: back to the previous row when it still names this write's hash;
//   moved to the file without the rule when it names the file as it is now (another answer's
//   refresh, or a person's trust, covered all of it); else left alone.
//
// - **Proof.** Each rule written here is also recorded in `agent_remembered` (`RuleStore`: a
//   local rule in the same transaction as its row; a project rule just before the file is
//   replaced, dropped again when the write fails or is undone). That record, not the `approval:<id>` source, is what lets a
//   remembered `allow` lift a matching `ask` (owner decision 2026-11-08, `rules.ts`): from the
//   local, project, user and role layers, never a `deny`, never a managed `ask`. A remembered
//   `deny` is a rule like any other.
import { chmodSync, closeSync, constants, fstatSync, lstatSync, openSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { type Approval, type ApprovalDecision, type ListedRule, type Principal, type RememberScope, RuleCommand, type Session } from "@finisher/contracts";
import { Schema } from "effect";
import type { Audit } from "../control/audit.ts";
import { ulid } from "../control/ulid.ts";
import { finError } from "../errors.ts";
import type { Remembered } from "./approvals.ts";
import type { Clock } from "./clock.ts";
import type { ProjectTrust, RuleStore } from "./rule-store.ts";
import { hashOf, NO_FILE, RULES_FILE, RULES_FILE_MAX } from "./trust.ts";

export interface RememberDeps {
  readonly ruleStore: RuleStore;
  readonly sessions: { get(id: string): Session | undefined };
  /** The folder of a project (open, else indexed). */
  readonly dirOf: (projectId: string) => string | undefined;
  readonly audit: Audit;
  readonly clock: Clock;
}

const isObject = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);

const unreadable = () => finError("INVALID_REQUEST", `${RULES_FILE} cannot be read`, { field: "remember", reason: "unreadable" }, `fix or remove ${RULES_FILE}, or remember with projectLocal`);
const notRegular = () => finError("INVALID_REQUEST", `${RULES_FILE} is not a regular file (a symlink, a folder or a FIFO)`, { field: "remember", reason: "notRegular" }, `replace ${RULES_FILE} with a plain file, or remember with projectLocal`);
/** Never follow a link, never block on a FIFO (both 0 where the platform has none). */
const OPEN_FLAGS = constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0);

/** The file as it is: its bytes and mode, `undefined` when there is none; a non-regular file is refused. */
function readFile(path: string): { bytes: Buffer; mode: number } | undefined {
  try {
    if (!lstatSync(path).isFile()) throw notRegular();
  } catch (e) {
    const code = (e as { code?: unknown }).code;
    if (code === "ENOENT") return undefined;
    throw code === "INVALID_REQUEST" ? e : unreadable();
  }
  let fd: number;
  try {
    fd = openSync(path, OPEN_FLAGS);
  } catch (e) {
    throw (e as { code?: unknown }).code === "ELOOP" ? notRegular() : unreadable();
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) throw notRegular();
    if (st.size > RULES_FILE_MAX) throw finError("INVALID_REQUEST", `${RULES_FILE} is larger than ${RULES_FILE_MAX} bytes`, { field: "remember", reason: "fileFull" }, "remove rules from the file, or remember with projectLocal");
    return { bytes: readFileSync(fd), mode: st.mode & 0o7777 };
  } catch (e) {
    throw (e as { code?: unknown }).code === "INVALID_REQUEST" ? e : unreadable();
  } finally {
    closeSync(fd);
  }
}

/** The rules file's JSON (`undefined`: not a rules file). */
function parsed(bytes: Buffer): Record<string, unknown> & { rules: unknown[] } | undefined {
  let j: unknown;
  try {
    j = JSON.parse(bytes.toString("utf8"));
  } catch {
    return undefined;
  }
  return isObject(j) && Array.isArray(j.rules) ? (j as Record<string, unknown> & { rules: unknown[] }) : undefined;
}

const encode = (j: unknown) => Buffer.from(`${JSON.stringify(j, null, 2)}\n`, "utf8");

/** The file's JSON with `rule` appended to its `rules` (module docs). */
function appended(bytes: Buffer | undefined, rule: Record<string, unknown>): Record<string, unknown> {
  if (bytes === undefined) return { rules: [rule] };
  const j = parsed(bytes);
  if (j === undefined) {
    throw finError("INVALID_REQUEST", `${RULES_FILE} is not a rules file`, { field: "remember", reason: "invalidRules" }, `fix ${RULES_FILE}, or remember with projectLocal`);
  }
  return { ...j, rules: [...j.rules, rule] };
}

/** Writes `bytes` over `path` atomically, with `mode`: a temporary file beside it, then a rename. */
function replaceFile(path: string, bytes: Buffer, mode: number): void {
  const tmp = `${path}.${process.pid}.${ulid()}.tmp`;
  try {
    writeFileSync(tmp, bytes, { flag: "wx", mode });
    // The mode as it was, whatever the umask.
    chmodSync(tmp, mode);
    renameSync(tmp, path);
  } catch (e) {
    rmSync(tmp, { force: true });
    throw e;
  }
}

/** A new file's mode. */
const NEW_MODE = 0o644;

const NOTICE_UNTRUSTED = "The rule is written to finisher.rules.json and does not apply until a person trusts the project (finisherd project trust); this answer's one-time grant still applies.";

/** The rule a `remember` asks for, written where its scope says (module docs). */
export function remember(d: RememberDeps, a: Approval, decision: ApprovalDecision, scope: RememberScope, by: Principal): Remembered {
  const command = isObject(a.subject) && typeof a.subject.command === "string" ? a.subject.command : undefined;
  if (command === undefined) throw finError("INVALID_REQUEST", "this approval names no command to remember", { field: "remember", reason: "noCommand" }, "answer without remember");
  // Exactly the card's command: one a rule cannot hold, or holding a `*` (a rule would read it as
  // a glob and match more), is refused, so a remembered rule never breaks a layer or widens.
  if (command.includes("*") || !Schema.is(RuleCommand)(command)) {
    throw finError("INVALID_REQUEST", "this approval's command cannot be written as an exact rule", { field: "remember", reason: "notExact" }, "answer without remember, and add a rule by hand if you want one (`finisherd agent rules add`)");
  }
  const s = d.sessions.get(a.sessionId);
  if (s === undefined) throw finError("SESSION_NOT_FOUND", "the approval's session is gone", { sessionId: a.sessionId });
  const now = d.clock.now();
  const who = `${by.kind}:${by.id}`;
  const source = `approval:${a.id}`;
  const audit = (kind: "agent.rule.add" | "agent.project.trust", details: Record<string, string>) =>
    d.audit.append({ kind, principal: { kind: by.kind, id: by.id }, outcome: "ok", details: { rpc: "agent.approval.answer", ...details } });

  if (scope !== "project") {
    const rule = d.ruleStore.add({
      id: ulid(now),
      effect: decision,
      command,
      projectId: s.projectId,
      ...(scope === "session" ? { sessionId: s.id } : {}),
      source,
      createdBy: who,
      createdAt: now,
    }, a.id);
    audit("agent.rule.add", { ruleId: rule.id, layer: "local", value: decision, projectId: s.projectId, ...(scope === "session" ? { sessionId: s.id } : {}), from: a.id });
    const listed: ListedRule = { ...rule, applies: true };
    return { rule: listed, undo: () => void d.ruleStore.remove(rule.id) };
  }

  const dir = d.dirOf(s.projectId);
  if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "the session's project is not on this server", { projectId: s.projectId }, "open the project, or remember with projectLocal");
  const path = join(dir, RULES_FILE);
  const found = readFile(path);
  const before = found?.bytes;
  const mode = found?.mode ?? NEW_MODE;
  const id = ulid(now);
  const next = encode(appended(before, { id, effect: decision, command }));
  if (next.length > RULES_FILE_MAX) throw finError("INVALID_REQUEST", `${RULES_FILE} would pass ${RULES_FILE_MAX} bytes`, { field: "remember", reason: "fileFull" }, "remove rules from the file, or remember with projectLocal");
  // The record first: a failed write takes it back, so a rule in the file always has its proof.
  d.ruleStore.remembered({ ruleId: id, layer: "project", projectId: s.projectId, command, approvalId: a.id, createdAt: now });
  try {
    replaceFile(path, next, mode);
  } catch (e) {
    d.ruleStore.forget(s.projectId, id);
    throw e;
  }
  const trust = d.ruleStore.trustOf(s.projectId);
  const previous = before === undefined ? NO_FILE : hashOf(before);
  const wrote = hashOf(next);
  const refreshed = trust !== undefined && trust.rulesHash === previous;
  if (refreshed) d.ruleStore.trust(s.projectId, wrote, who, now);
  audit("agent.rule.add", { ruleId: id, layer: "project", value: decision, projectId: s.projectId, from: a.id });
  if (refreshed) audit("agent.project.trust", { projectId: s.projectId, rulesHash: wrote });
  const listed: ListedRule = { id, effect: decision, command, layer: "project", source: RULES_FILE, createdBy: who, createdAt: now, applies: refreshed };
  return {
    rule: listed,
    ...(refreshed ? {} : { notice: NOTICE_UNTRUSTED }),
    // An answer refused after the write takes its own rule back out (module docs).
    undo: () => undoWrite(d, { projectId: s.projectId, path, id, before, next, wrote, prior: refreshed ? trust : undefined }),
  };
}

/** What an undo needs of the write it takes back. */
interface Written {
  readonly projectId: string;
  readonly path: string;
  /** The rule's id in the file. */
  readonly id: string;
  /** The file before the write (`undefined`: none), and the write. */
  readonly before: Buffer | undefined;
  readonly next: Buffer;
  readonly wrote: string;
  /** The trust row the write refreshed (`undefined`: it did not). */
  readonly prior: ProjectTrust | undefined;
}

/** Takes the rule of `w` back out of the file, and the trust with it (module docs). Best effort. */
function undoWrite(d: RememberDeps, w: Written): void {
  d.ruleStore.forget(w.projectId, w.id);
  let now: { bytes: Buffer; mode: number } | undefined;
  try {
    now = readFile(w.path);
  } catch {
    return;
  }
  if (now === undefined) return;
  const j = parsed(now.bytes);
  if (j === undefined) return;
  const rules = j.rules.filter((r) => !(isObject(r) && r.id === w.id));
  if (rules.length === j.rules.length) return;
  // Nothing in between: the previous bytes exactly (or no file). Else the file as it is, less the rule.
  const untouched = now.bytes.equals(w.next);
  const after = untouched ? w.before : encode({ ...j, rules });
  if (after === undefined) rmSync(w.path, { force: true });
  else replaceFile(w.path, after, now.mode);
  const afterHash = after === undefined ? NO_FILE : hashOf(after);
  const trust = d.ruleStore.trustOf(w.projectId);
  if (trust === undefined) return;
  if (trust.rulesHash === w.wrote && w.prior !== undefined) d.ruleStore.trust(w.projectId, w.prior.rulesHash, w.prior.by, w.prior.trustedAt);
  else if (trust.rulesHash === hashOf(now.bytes)) d.ruleStore.trust(w.projectId, afterHash, trust.by, trust.trustedAt);
}
