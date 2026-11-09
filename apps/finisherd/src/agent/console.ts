// `finisherd agent start|ls|show|prompt|steer|interrupt|resume|end|rollback|watch|approve|delete|providers`
// (plan 7A Task 12; sub-project 7 §3.5): the owner's agent sessions from the console. Driving and
// reading sessions needs no terminal (they are `agent.run`/`agent.read`, as `fin` has them);
// answering an approval and deleting a session are a person's, with a terminal and a confirmation
// (`confirmed`, owner decision of 2026-11-08): a `y` to answer an approval, the session id's first
// 8 characters typed back to delete one. Text for people, `--json` for scripts; errors as the console prints them.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Approval, Node, Run, Session, WatchItem } from "@finisher/contracts";
import { projectIdIn } from "../chain/resolve.ts";
import { type ConsoleConnection, consoleError, valueOf } from "../console/client.ts";
import { parseDuration } from "../console/commands/agent.ts";
import { type CommandContext, withConnection } from "../console/commands/context.ts";
import { confirmed, proceed, requireTty, typeName } from "../console/confirm.ts";
import { has, one, onlyFlags, usage } from "../console/format.ts";
import { safe } from "../console/safe.ts";

export const SESSION_COMMANDS = new Set(["start", "ls", "list", "show", "prompt", "steer", "interrupt", "resume", "end", "rollback", "watch", "approve", "delete", "providers"]);

const ULID = /^[0-9A-HJKMNP-TV-Za-hjkmnp-tv-z]{26}$/;
const STATUSES = new Set(["starting", "idle", "running", "waitingApproval", "interrupted", "ended"]);

/** A project id, or the id a project folder's `finisher.project.json` names. */
export async function projectArg(value: string): Promise<string> {
  if (ULID.test(value)) return value;
  const id = await projectIdIn(resolve(value));
  if (id === undefined) usage(`${value} is neither a project id nor a project folder`, "Pass the project's folder or its id.");
  return id;
}

const call = async <A>(c: ConsoleConnection, tag: string, payload: unknown) => valueOf<A>(await c.call(tag, payload));

export async function agentSessions(ctx: CommandContext, sub: string, rest: ReadonlyArray<string>): Promise<number> {
  switch (sub) {
    case "start":
      return start(ctx, rest);
    case "ls":
    case "list":
      return list(ctx, rest);
    case "show":
      return show(ctx, rest);
    case "prompt":
    case "steer":
      return send(ctx, sub, rest);
    case "interrupt":
    case "resume":
    case "end":
      return control(ctx, sub, rest);
    case "rollback":
      return rollback(ctx, rest);
    case "watch":
      return watch(ctx, rest);
    case "approve":
      return approve(ctx, rest);
    case "delete":
      return remove(ctx, rest);
    default:
      return providers(ctx, rest);
  }
}

function idOf(command: string, rest: ReadonlyArray<string>, more = false): string {
  const [id] = rest;
  if (id === undefined || (!more && rest.length > 1)) usage(`agent ${command} needs one session id (agent ls lists them)`);
  return id;
}

/** `5`, `2.50` (USD) as micros. */
function usdMicros(flag: string, v: string): number {
  if (!/^\d+(\.\d{1,6})?$/.test(v)) usage(`--${flag} needs an amount in USD such as 5 or 2.50`);
  return Math.round(Number(v) * 1_000_000);
}

/** `200000`, `200k`, `2m` as a count. */
function count(flag: string, v: string): number {
  const m = /^(\d+)([kKmM]?)$/.exec(v);
  if (m === null) usage(`--${flag} needs a whole number such as 200000, 200k or 2m`);
  return Number(m[1]) * ({ "": 1, k: 1_000, m: 1_000_000 } as const)[(m[2] ?? "").toLowerCase() as "" | "k" | "m"];
}

function warn(ctx: CommandContext, warnings: ReadonlyArray<{ code: string; message: string; fix?: string }> | undefined): void {
  for (const w of warnings ?? []) {
    ctx.output.err(`finisherd: warning: ${safe(w.code)}: ${safe(w.message)}`);
    if (w.fix !== undefined) ctx.output.err(`finisherd: fix: ${safe(w.fix)}`);
  }
}

export function sessionLine(s: Session): string {
  const parts = [s.id, s.status, s.role, s.mode, `${s.provider}/${s.model}`, `project ${s.projectId}`];
  if (s.rootBranch !== undefined) parts.push(`branch ${s.rootBranch}`);
  if (s.title !== undefined) parts.push(JSON.stringify(safe(s.title)));
  return parts.join("  ");
}

async function start(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent start", ["project", "model", "role", "mode", "base", "title", "prompt", "prompt-file", "budget-usd", "budget-tokens", "provider", "harness", "json"]);
  if (rest.length > 0) usage("agent start takes flags only (the prompt goes in --prompt or --prompt-file)");
  const project = one(ctx.args, "project");
  if (project === undefined) usage("agent start needs --project <dir|id>");
  const promptText = one(ctx.args, "prompt");
  const promptFile = one(ctx.args, "prompt-file");
  if (promptText !== undefined && promptFile !== undefined) usage("give --prompt or --prompt-file, not both");
  const mode = one(ctx.args, "mode");
  if (mode !== undefined && !["plan", "draft", "auto"].includes(mode)) usage("--mode is plan, draft or auto");
  const provider = one(ctx.args, "provider");
  if (provider !== undefined && provider !== "openrouter" && provider !== "local") usage("--provider is openrouter or local");
  let prompt = promptText;
  if (promptFile !== undefined) {
    try {
      prompt = readFileSync(resolve(promptFile), "utf8");
    } catch {
      usage(`cannot read ${promptFile}`);
    }
  }
  const usd = one(ctx.args, "budget-usd");
  const tokens = one(ctx.args, "budget-tokens");
  const budget = { ...(usd === undefined ? {} : { moneyMicros: usdMicros("budget-usd", usd) }), ...(tokens === undefined ? {} : { tokens: count("budget-tokens", tokens) }) };
  const opt = (flag: string, key: string) => (one(ctx.args, flag) === undefined ? {} : { [key]: one(ctx.args, flag) });
  const payload = {
    projectId: await projectArg(project),
    ...opt("model", "model"),
    ...opt("role", "role"),
    ...opt("mode", "mode"),
    ...opt("base", "base"),
    ...opt("title", "title"),
    ...opt("provider", "provider"),
    ...opt("harness", "harness"),
    ...(prompt === undefined || prompt.trim() === "" ? {} : { prompt }),
    ...(Object.keys(budget).length === 0 ? {} : { budget }),
  };
  const r = await withConnection(ctx, (c) => call<{ session: Session; run?: Run; warnings?: { code: string; message: string; fix?: string }[] }>(c, "agent.session.start", payload));
  warn(ctx, r.warnings);
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else {
    ctx.output.out(`Started agent session ${r.session.id}: ${sessionLine(r.session)}`);
    ctx.output.out(r.run === undefined ? `Give it work with: finisherd agent prompt ${r.session.id} <text>` : `Run ${r.run.id} started. Follow it with: finisherd agent watch ${r.session.id}`);
  }
  return 0;
}

async function list(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent ls", ["project", "status", "json"]);
  if (rest.length > 0) usage("agent ls takes --project, --status and --json only");
  const status = one(ctx.args, "status");
  if (status !== undefined && !STATUSES.has(status)) usage(`--status is one of ${[...STATUSES].join(", ")}`);
  const project = one(ctx.args, "project");
  const q = { ...(project === undefined ? {} : { projectId: await projectArg(project) }), ...(status === undefined ? {} : { status }) };
  const sessions = await withConnection(ctx, async (c) => {
    const all: Session[] = [];
    for (let cursor: string | undefined; ; ) {
      const page = await call<{ sessions: Session[]; next?: string }>(c, "agent.session.list", { ...q, limit: 200, ...(cursor === undefined ? {} : { cursor }) });
      all.push(...page.sessions);
      if (page.next === undefined || all.length >= 1000) return all;
      cursor = page.next;
    }
  });
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ sessions }));
  else if (sessions.length === 0) ctx.output.out("No agent sessions.");
  else for (const s of sessions) ctx.output.out(sessionLine(s));
  return 0;
}

/** One node as a line for a person (`agent watch`, `agent show --nodes`). */
export function nodeLine(n: Node): string {
  const p = (n.payload ?? {}) as Record<string, unknown>;
  const text = (v: unknown, max = 500) => {
    const s = typeof v === "string" ? v : JSON.stringify(v ?? null);
    return safe(s.length > max ? `${s.slice(0, max)}…` : s);
  };
  const body = (() => {
    if (n.blobRef !== undefined) return `(large: ${n.blobRef.slice(0, 12)}…)`;
    switch (n.kind) {
      case "userMessage":
        return `${p.trigger === "steer" ? "steer" : "you"}: ${text(p.text)}`;
      case "assistantMessage": {
        const blocks = Array.isArray(p.blocks) ? (p.blocks as { type?: unknown; text?: unknown }[]) : [];
        const said = blocks.filter((b) => b?.type === "text" && typeof b.text === "string").map((b) => b.text as string);
        return text(said.length > 0 ? said.join(" ") : p);
      }
      case "reasoning":
        return text(p.text ?? p.summary ?? "(reasoning)");
      case "toolCall":
        return `${text(p.tool, 64)} ${text(p.input, 300)}`;
      case "toolResult":
        return `${p.ok === true ? "ok" : "failed"} ${text(p.output, 300)}`;
      case "error":
        return `${text(p.code, 64)}: ${text(p.message)}`;
      default:
        return text(p, 300);
    }
  })();
  return `#${n.seq} ${n.kind}  ${body}${n.rolledBack === true ? "  (rolled back)" : ""}`;
}

function itemLine(i: WatchItem): string | undefined {
  switch (i.type) {
    case "nodeAdded":
      return nodeLine(i.node);
    case "statusChanged":
      return `-- ${i.status}${i.reason === undefined ? "" : ` (${safe(i.reason)})`}`;
    case "approvalOpened":
      return `-- approval ${i.approval.id} (${i.approval.kind}) waits: finisherd agent approve ${i.approval.id} allow|deny`;
    default:
      return undefined;
  }
}

async function show(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent show", ["nodes", "json"]);
  const id = idOf("show", rest);
  const r = await withConnection(ctx, async (c) => {
    const got = await call<{ session: Session; runs: Run[]; approvals: Approval[] }>(c, "agent.session.get", { sessionId: id });
    if (!has(ctx.args, "nodes")) return { ...got, nodes: undefined };
    // The stored nodes come first on a watch, then the status: stop at the status.
    const nodes: Node[] = [];
    let stopped = false;
    let stop = () => {};
    const s = c.stream("agent.watch", { sessionId: id }, (v) => {
      const item = v as WatchItem;
      if (stopped) return;
      if (item.type === "nodeAdded") nodes.push(item.node);
      else if (item.type === "statusChanged") {
        stopped = true;
        stop();
      }
    });
    stop = s.stop;
    const exit = await s.done;
    // Interrupted by us once the snapshot was in: how it ended does not matter.
    if (!stopped) valueOf(exit);
    return { ...got, nodes };
  });
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(r.nodes === undefined ? { session: r.session, runs: r.runs, approvals: r.approvals } : r));
    return 0;
  }
  const s = r.session;
  ctx.output.out(sessionLine(s));
  if (s.statusReason !== undefined) ctx.output.out(`reason: ${safe(s.statusReason)}`);
  ctx.output.out(`usage: ${s.usage.input} in, ${s.usage.output} out, ${(s.usage.moneyMicros / 1_000_000).toFixed(4)} USD${s.usage.costBasis === undefined ? "" : ` (${s.usage.costBasis})`}`);
  for (const run of r.runs) ctx.output.out(`run ${run.id}  ${run.status}  ${run.trigger}${run.stopReason === undefined ? "" : `  ${safe(run.stopReason)}`}`);
  for (const a of r.approvals) ctx.output.out(`approval ${a.id}  ${a.kind}  ${safe(JSON.stringify(a.subject))}  (finisherd agent approve ${a.id} allow|deny)`);
  for (const n of r.nodes ?? []) ctx.output.out(nodeLine(n));
  return 0;
}

async function send(ctx: CommandContext, verb: "prompt" | "steer", rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, `agent ${verb}`, ["json"]);
  const id = idOf(verb, rest, true);
  const text = rest.slice(1).join(" ");
  if (text.trim() === "") usage(`agent ${verb} needs the text after the session id`);
  const r = await withConnection(ctx, (c) => call<{ run?: Run; queued?: boolean }>(c, `agent.session.${verb}`, { sessionId: id, text }));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify(r));
  else if (verb === "prompt") ctx.output.out(`Run ${r.run!.id} started. Follow it with: finisherd agent watch ${id}`);
  else ctx.output.out(r.queued ? "Queued for the next run." : "Sent to the running turn.");
  return 0;
}

async function control(ctx: CommandContext, verb: "interrupt" | "resume" | "end", rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, `agent ${verb}`, ["json"]);
  const id = idOf(verb, rest);
  const { session } = await withConnection(ctx, (c) => call<{ session: Session }>(c, `agent.session.${verb}`, { sessionId: id }));
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ session }));
  else ctx.output.out(`${sessionLine(session)}${verb === "end" && session.rootBranch !== undefined ? "  (its branch stays)" : ""}`);
  return 0;
}

/** `agent rollback <id> --to-run <seq>` (7B Task 8): the session back to the end of that run. */
async function rollback(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent rollback", ["to-run", "json"]);
  const id = idOf("rollback", rest);
  const to = one(ctx.args, "to-run");
  if (to === undefined || !/^\d+$/.test(to)) usage("agent rollback needs --to-run <seq>, a run's number (0: before the first)", "List the runs with `finisherd agent show <id>`.");
  type Answer = { session: Session; undone: string[]; branches: string[]; conversation: "rolledBack" | "kept" };
  const r = await withConnection(ctx, (c) => call<Answer>(c, "agent.session.rollback", { sessionId: id, toRun: Number(to) }));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(r));
    return 0;
  }
  ctx.output.out(`Rolled ${id} back to run ${to}: ${r.undone.length} transaction(s) undone on ${r.branches.length} branch(es).`);
  if (r.conversation === "kept") ctx.output.out("Its provider keeps the later runs in its own history (it cannot roll back); the session goes on from there.");
  ctx.output.out(sessionLine(r.session));
  return 0;
}

async function watch(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent watch", ["json"]);
  const id = idOf("watch", rest);
  const json = has(ctx.args, "json");
  await withConnection(ctx, async (c) => {
    const s = c.stream("agent.watch", { sessionId: id }, (v) => {
      const item = v as WatchItem;
      if (json) return ctx.output.out(JSON.stringify(item));
      const line = itemLine(item);
      if (line !== undefined) ctx.output.out(line);
    });
    const stopOnSigint = () => s.stop();
    process.once("SIGINT", stopOnSigint);
    try {
      // Ends when the session ends (or the server does).
      valueOf(await s.done);
    } finally {
      process.off("SIGINT", stopOnSigint);
    }
  });
  return 0;
}

/** `--add`: USD for a money budget, a count for tokens, a duration for the wall clock. */
function addFor(kind: unknown, v: string): Record<string, number> {
  if (kind === "money") return { moneyMicros: usdMicros("add", v) };
  if (kind === "tokens") return { tokens: count("add", v) };
  if (kind === "wallMs") return { wallMs: (parseDuration("add", v, false) ?? 0) * 1000 };
  return usage(`--add raises a budget approval; this one is ${String(kind)}`);
}

async function approve(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent approve", ["add", "remember", "json"]);
  const [id, decision, ...more] = rest;
  if (id === undefined || (decision !== "allow" && decision !== "deny") || more.length > 0) usage("agent approve needs an approval id and allow or deny");
  const add = one(ctx.args, "add");
  if (add !== undefined && decision === "deny") usage("--add goes with allow");
  // 7B Task 5: a rule of exactly the card's command, for the session, the project's shared file or the project locally.
  const remember = one(ctx.args, "remember");
  if (remember !== undefined && remember !== "session" && remember !== "project" && remember !== "projectLocal") usage("--remember is session, project or projectLocal");
  requireTty(ctx.prompt, "agent approve");
  const { approval, rule, notice } = await withConnection(ctx, async (c) => {
    const { approvals } = await call<{ approvals: Approval[] }>(c, "agent.approval.list", { status: "open" });
    const found = approvals.find((a) => a.id === id);
    if (found === undefined) throw consoleError("APPROVAL_NOT_FOUND", "no open approval with that id", { id }, "list them with `finisherd agent show <session>`");
    const subject = (found.subject ?? {}) as { kind?: unknown; used?: unknown; limit?: unknown };
    const raise = add === undefined ? undefined : addFor(subject.kind, add);
    const what =
      decision === "deny"
        ? `This denies approval ${id} (${found.kind}) of session ${found.sessionId}.`
        : `This allows approval ${id} (${found.kind}) of session ${found.sessionId}${raise === undefined ? "" : `, raising its ${String(subject.kind)} budget by ${add}`}${found.kind === "budget" ? ", and resumes the session" : ""}.`;
    const remembered = remember === undefined ? "" : ` It also adds a rule: ${decision} ${safe(String((found.subject as { command?: unknown } | null)?.command ?? found.kind))} ${remember === "session" ? "for this session" : remember === "project" ? "in the project's shared finisher.rules.json" : "for this project, on this server only"}.`;
    return confirmed<{ approval: Approval; rule?: { id: string; layer: string }; notice?: string }>(c, ctx.prompt, `${what}${remembered}`, proceed, "agent.approval.answer", {
      id,
      decision,
      ...(raise === undefined ? {} : { add: raise }),
      ...(remember === undefined ? {} : { remember }),
    });
  });
  if (has(ctx.args, "json")) ctx.output.out(JSON.stringify({ approval, ...(rule === undefined ? {} : { rule }), ...(notice === undefined ? {} : { notice }) }));
  else {
    ctx.output.out(`Approval ${approval.id}: ${approval.status}${approval.decision === undefined ? "" : ` (${approval.decision})`}.`);
    if (rule !== undefined) {
      const how = rule.layer === "project" ? "edit the project's finisher.rules.json" : `\`finisherd agent rules rm ${rule.id}\``;
      ctx.output.out(`Rule ${rule.id} added (${rule.layer} layer); to remove it, ${how}.`);
    }
    if (notice !== undefined) ctx.output.out(safe(notice));
  }
  return 0;
}

async function remove(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent delete", []);
  const id = idOf("delete", rest);
  requireTty(ctx.prompt, "agent delete");
  await withConnection(ctx, async (c) => {
    const { session } = await call<{ session: Session }>(c, "agent.session.get", { sessionId: id });
    return confirmed(
      c,
      ctx.prompt,
      `This deletes agent session ${id} (${session.role}, ${session.status}): it ends now and its conversation, runs and approvals are removed. ${session.rootBranch === undefined ? "" : `Its branch ${session.rootBranch} stays. `}What the journal recorded under your consent stays too.`,
      typeName("session id's first 8 characters", id.slice(0, 8)),
      "agent.session.delete",
      { sessionId: id },
    );
  });
  ctx.output.out(`Deleted agent session ${id}.`);
  return 0;
}

async function providers(ctx: CommandContext, rest: ReadonlyArray<string>): Promise<number> {
  onlyFlags(ctx.args, "agent providers", ["json"]);
  if (rest.length > 0) usage("agent providers takes --json only");
  type Answer = {
    providers: { harness: string; detected: boolean; version?: string; signedIn?: boolean | null; problem?: { code: string; message: string } }[];
    openrouter: { keySet: boolean; projects?: string[] };
    local: { configured: boolean; preset?: string; reachable?: boolean };
    warnings?: { code: string; message: string; fix?: string }[];
  };
  const r = await withConnection(ctx, (c) => call<Answer>(c, "agent.providers", {}));
  if (has(ctx.args, "json")) {
    ctx.output.out(JSON.stringify(r));
    return 0;
  }
  for (const p of r.providers) {
    ctx.output.out(`${p.harness}  ${p.detected ? "available" : "not available"}${p.version === undefined ? "" : `  ${safe(p.version)}`}${p.problem === undefined ? "" : `  ${p.problem.code}: ${safe(p.problem.message)}`}`);
  }
  const own = r.openrouter.projects ?? [];
  ctx.output.out(`openrouter key: ${r.openrouter.keySet ? "set for the server" : "not set for the server"}${own.length === 0 ? "" : `; own keys for ${own.join(", ")}`}`);
  ctx.output.out(`local endpoint: ${r.local.configured ? `${r.local.preset} (${r.local.reachable ? "answers" : "does not answer"})` : "not set"}`);
  warn(ctx, r.warnings);
  return 0;
}
