// 5B Task 4: typed confirmations for every human principal, as the owner narrowed Decision 22
// (2026-11-08): a paired device confirms `server.stop` with its own connection's code, and
// nothing else; the console keeps 5A's rule (every human-only mutation, `service` and
// `readOnly` opt-outs, the `self` pairing offer's opt-out).
import { type FinError, HUMAN_ONLY, type Principal, RPC_SCOPES, requiredScopes, ruleOf, SCOPES } from "@finisher/contracts";
import { auditDetailsValid, needsConfirmation } from "@finisher/contracts";
import { Effect, Exit } from "effect";
import { describe, expect, test } from "vitest";
import { confirmationRequired, issueChallenge, mayChallenge } from "../src/auth/challenges.ts";
import { withChain } from "../src/chain/chain.ts";
import type { ChainProjects } from "../src/chain/resolve.ts";
import { Connections } from "../src/connections.ts";
import type { Audit, AuditInput } from "../src/control/audit.ts";
import { CONSOLE, LOCAL } from "../src/principal.ts";
import { SAMPLE_PAYLOADS } from "./support/payloads.ts";

const DEVICE: Principal = { kind: "device", id: "01device00000000000000000a", scopes: [...SCOPES], projects: "*", branches: "*", human: true };
const AGENT: Principal = { kind: "agent", id: "01agent000000000000000000a", scopes: ["server.read"], projects: "*", branches: "*", human: false };

const projects: ChainProjects = {
  isOpen: () => true,
  dirOf: () => undefined,
  resolveBranch: (_p, ref) => Effect.succeed(ref),
  branches: () => Effect.succeed([]),
};

/** A chain over recording handlers, with two connections (1 and 2) of `principal`. */
function harness(principal: Principal) {
  const reached: string[] = [];
  const audited: AuditInput[] = [];
  const connections = new Connections("ws", () => "ws://127.0.0.1:1");
  for (const id of [1, 2]) {
    connections.attach(id, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(id)!.principal = principal;
  }
  const handler = (tag: string) => () => Effect.sync(() => void reached.push(tag));
  const handlers: Record<string, unknown> = {};
  for (const tag of ["server.stop", "auth.pair.approve", "auth.device.revoke", "auth.pair.offer", "settings.write", "auth.audit.purge", "diagnostics.consent", "auth.device.list"]) handlers[tag] = handler(tag);
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const chained = withChain(handlers, { connections, projects, audit }) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<unknown, FinError>>;
  const call = (tag: string, payload: unknown, client = 1) => Effect.runPromiseExit(chained[tag]!(payload, { client: { id: client }, headers: {} }));
  const challenge = (client = 1) => issueChallenge(connections.get(client)!).code;
  return { reached, audited, call, challenge };
}

const failure = (exit: Exit.Exit<unknown, FinError>): FinError | undefined =>
  Exit.isFailure(exit) && exit.cause.reasons[0]?._tag === "Fail" ? (exit.cause.reasons[0].error as FinError) : undefined;
const codeOf = (exit: Exit.Exit<unknown, FinError>) => failure(exit)?.code;
const reasonOf = (exit: Exit.Exit<unknown, FinError>) => (failure(exit)?.details as { reason?: string } | undefined)?.reason;

describe("a device's confirmations", () => {
  test("device_server_stop_needs_code", async () => {
    const h = harness(DEVICE);
    const missing = await h.call("server.stop", {});
    expect(codeOf(missing)).toBe("CONFIRMATION_REQUIRED");
    expect(reasonOf(missing)).toBe("missing");
    // The fix speaks to the person on the device, in words.
    expect(failure(missing)?.details).toMatchObject({ where: "app" });
    expect(failure(missing)?.fix).toContain("this device");
    expect(h.reached).toEqual([]);
    expect(codeOf(await h.call("server.stop", { challenge: h.challenge() }))).toBeUndefined();
    expect(h.reached).toEqual(["server.stop"]);
  });

  test("device_code_is_per_connection", async () => {
    const h = harness(DEVICE);
    const other = h.challenge(2);
    const exit = await h.call("server.stop", { challenge: other }, 1);
    expect(codeOf(exit)).toBe("CONFIRMATION_REQUIRED");
    expect(reasonOf(exit)).toBe("wrong");
    // Connection 2 can still use its own.
    expect(codeOf(await h.call("server.stop", { challenge: other }, 2))).toBeUndefined();
  });

  test("device_wrong_code_voids_codes", async () => {
    const h = harness(DEVICE);
    const good = h.challenge();
    expect(reasonOf(await h.call("server.stop", { challenge: "ZZZZZZ" }))).toBe("wrong");
    // The guess voided every code of the connection.
    expect(reasonOf(await h.call("server.stop", { challenge: good }))).toBe("wrong");
    expect(codeOf(await h.call("server.stop", { challenge: h.challenge() }))).toBeUndefined();
  });

  test("device_confirms_only_server_stop (owner decision, the matrix over RPC_SCOPES)", () => {
    const tags = Object.keys(RPC_SCOPES) as Array<keyof typeof RPC_SCOPES>;
    const confirmed = tags.filter((t) => needsConfirmation(t, SAMPLE_PAYLOADS[t], {}, DEVICE));
    expect(confirmed).toEqual(["server.stop"]);
    // Whatever escalations ask for, e.g. a human-only kind's scopes or training on.
    expect(needsConfirmation("jobs.submit", { kind: "k", params: {} }, { kindScopes: ["library.admin"] }, DEVICE)).toBe(false);
    expect(needsConfirmation("diagnostics.consent", { purpose: "training", value: "on" }, {}, DEVICE)).toBe(false);
    // The console still confirms every human-only mutation (5A's rule stands).
    const consoleConfirmed = tags.filter((t) => needsConfirmation(t, SAMPLE_PAYLOADS[t], {}, CONSOLE));
    expect(consoleConfirmed).not.toContain("server.stop");
    for (const t of consoleConfirmed) expect(requiredScopes(t, SAMPLE_PAYLOADS[t]).some((s) => HUMAN_ONLY.has(s)), t).toBe(true);
    expect(consoleConfirmed).toEqual(expect.arrayContaining(["auth.pair.approve", "auth.device.revoke", "settings.write", "env.reset"]));
  });

  test("human-only mutations from a device go ahead without a code", async () => {
    const h = harness(DEVICE);
    for (const tag of ["auth.pair.approve", "auth.device.revoke", "settings.write", "auth.audit.purge", "diagnostics.consent"]) {
      expect(codeOf(await h.call(tag, SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS])), tag).toBeUndefined();
    }
    expect(h.audited.filter((e) => e.kind === "console.refused")).toEqual([]);
  });

  test("read_only_opt_outs_hold_for_devices", async () => {
    const h = harness(DEVICE);
    for (const tag of Object.keys(RPC_SCOPES) as Array<keyof typeof RPC_SCOPES>) {
      if (ruleOf(tag).unconfirmed === "readOnly") expect(needsConfirmation(tag, SAMPLE_PAYLOADS[tag], {}, DEVICE), tag).toBe(false);
    }
    expect(codeOf(await h.call("auth.device.list", {}))).toBeUndefined();
  });

  test("console_refused_audited_for_devices", async () => {
    const h = harness(DEVICE);
    await h.call("server.stop", { challenge: "WRONG1" });
    expect(h.audited).toHaveLength(1);
    expect(h.audited[0]).toMatchObject({ kind: "console.refused", principal: { kind: "device", id: DEVICE.id }, code: "CONFIRMATION_REQUIRED" });
    // Never the code it was sent, and the row fits the audit's closed details.
    expect(JSON.stringify(h.audited)).not.toContain("WRONG1");
    expect(auditDetailsValid("console.refused", h.audited[0]!.details ?? {})).toBe(true);
  });
});

describe("the console", () => {
  test("console_stop_stays_service", async () => {
    const h = harness(CONSOLE);
    expect(needsConfirmation("server.stop", {}, {}, CONSOLE)).toBe(false);
    expect(codeOf(await h.call("server.stop", {}))).toBeUndefined();
    // Its other human-only mutations still need the code.
    expect(codeOf(await h.call("auth.device.revoke", { id: "01device00000000000000000a" }))).toBe("CONFIRMATION_REQUIRED");
    expect(codeOf(await h.call("auth.device.revoke", { id: "01device00000000000000000a", challenge: h.challenge() }))).toBeUndefined();
  });

  test("self_offer_needs_no_code_only_from_console (Decision 21's payload opt-out)", async () => {
    expect(needsConfirmation("auth.pair.offer", { self: true }, {}, CONSOLE)).toBe(false);
    expect(needsConfirmation("auth.pair.offer", {}, {}, CONSOLE)).toBe(true);
    expect(needsConfirmation("auth.pair.offer", { self: false }, {}, CONSOLE)).toBe(true);
    // A device confirms no offer (and Task 8 refuses its `self` offer outright).
    expect(needsConfirmation("auth.pair.offer", { self: true }, {}, DEVICE)).toBe(false);
    const h = harness(CONSOLE);
    expect(codeOf(await h.call("auth.pair.offer", { self: true }))).toBeUndefined();
    expect(codeOf(await h.call("auth.pair.offer", {}))).toBe("CONFIRMATION_REQUIRED");
  });

  test("its refusals keep 5A's wording", () => {
    const e = confirmationRequired("auth.device.revoke", "missing");
    expect(e.details).toMatchObject({ where: "console" });
    expect(e.fix).toContain("console");
  });
});

describe("who may ask for a code", () => {
  test("non_human_gets_no_challenge", () => {
    expect(mayChallenge(CONSOLE)).toBe(true);
    expect(mayChallenge(DEVICE)).toBe(true);
    expect(mayChallenge(LOCAL)).toBe(false);
    expect(mayChallenge(AGENT)).toBe(false);
    expect(mayChallenge({ ...AGENT, kind: "mcp" })).toBe(false);
    // Never a non-human, whatever it claims.
    expect(mayChallenge({ ...DEVICE, human: false })).toBe(false);
    for (const p of [LOCAL, AGENT]) expect(needsConfirmation("server.stop", {}, {}, p)).toBe(false);
    // A human kind this code does not know is held to the console's rule: it fails closed.
    const future = { kind: "plugin" as const, human: true };
    expect(needsConfirmation("auth.device.revoke", { id: "x" }, {}, future)).toBe(true);
  });
});
