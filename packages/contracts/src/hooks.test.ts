// The hooks' configuration schema (7F plan Task 1, Decision 6) and the generated event names.
import { readFileSync } from "node:fs";
import { describe, expect, test } from "vitest";
import { HOOK_LIMITS, HookEvent, decodeHookSpec } from "./hooks.ts";

const ok = (raw: unknown) => decodeHookSpec(raw)._tag === "Success";

describe("HookSpec", () => {
  test("hook_spec_decodes_minimal_and_full", () => {
    expect(ok({ id: "lint", event: "PreCommand", run: ["node"] })).toBe(true);
    const full = {
      id: "no-friday-trims",
      event: "PreCommand",
      run: ["node", "fake-hook.mjs", "--reply", "block", "an arg with \"quotes\" and \\ backslashes"],
      file: "fake-hook.mjs",
      match: "item.trim",
      principals: "all",
      timeoutS: 60,
      onTimeout: "block",
      fin: true,
    };
    const r = decodeHookSpec(full);
    expect(r._tag).toBe("Success");
    if (r._tag === "Success") expect(r.value).toEqual(full);
    // Each event that names something takes a `match`: command names, a job kind, a status or
    // budget, an approval kind.
    for (const [event, match] of [["PostCommand", "item.*"], ["JobDone", "render.export"], ["Notification", "status.waitingApproval"], ["Notification", "budget.money"], ["Approval", "merge"]]) {
      expect(ok({ id: "m", event, run: ["node"], match }), `${event} ${match}`).toBe(true);
    }
  });

  test("hook_spec_refuses_unknown_keys", () => {
    for (const extra of [{ modify: true }, { net: true }, { env: { A: "1" } }, { reply: "allow" }]) {
      expect(ok({ id: "x", event: "SessionEnd", run: ["node"], ...extra }), JSON.stringify(extra)).toBe(false);
    }
    // Missing or malformed required fields.
    for (const bad of [
      { event: "SessionEnd", run: ["node"] },
      { id: "x", run: ["node"] },
      { id: "x", event: "SessionEnd" },
      { id: "x", event: "PreOp", run: ["node"] },
      { id: "Has Spaces", event: "SessionEnd", run: ["node"] },
      { id: "a".repeat(65), event: "SessionEnd", run: ["node"] },
      { id: "x", event: "SessionEnd", run: [""] },
      { id: "x", event: "SessionEnd", run: ["node", "a\u0000b"] },
      { id: "x", event: "SessionEnd", run: ["node"], principals: "people" },
      { id: "x", event: "SessionEnd", run: ["node"], onTimeout: "ask" },
      { id: "x", event: "SessionEnd", run: ["node"], fin: false },
      { id: "x", event: "SessionEnd", run: ["node"], file: "" },
    ]) {
      expect(ok(bad), JSON.stringify(bad)).toBe(false);
    }
  });

  test("match_refused_on_events_without_names", () => {
    for (const event of ["SessionStart", "SessionEnd", "UserPrompt", "BranchReady", "PreMerge", "PostMerge", "PreRender", "PostRender"]) {
      expect(ok({ id: "x", event, run: ["node"] }), event).toBe(true);
      expect(ok({ id: "x", event, run: ["node"], match: "item.trim" }), event).toBe(false);
    }
  });

  test("timeout_bounds", () => {
    for (const t of [1, 10, 60]) expect(ok({ id: "x", event: "PreMerge", run: ["node"], timeoutS: t }), String(t)).toBe(true);
    for (const t of [0, 61, 1.5, -1]) expect(ok({ id: "x", event: "PreMerge", run: ["node"], timeoutS: t }), String(t)).toBe(false);
    expect(HOOK_LIMITS.timeoutS).toEqual({ default: 10, max: 60 });
  });

  test("run_limits", () => {
    const arg = (n: number) => "a".repeat(n);
    expect(ok({ id: "x", event: "SessionEnd", run: Array.from({ length: 64 }, () => "a") })).toBe(true);
    expect(ok({ id: "x", event: "SessionEnd", run: Array.from({ length: 65 }, () => "a") })).toBe(false);
    expect(ok({ id: "x", event: "SessionEnd", run: [] })).toBe(false);
    expect(ok({ id: "x", event: "SessionEnd", run: ["node", arg(4096)] })).toBe(true);
    expect(ok({ id: "x", event: "SessionEnd", run: ["node", arg(4097)] })).toBe(false);
    // Bytes, not characters: 2,049 two-byte characters are 4,098 bytes.
    expect(ok({ id: "x", event: "SessionEnd", run: ["node", "é".repeat(2049)] })).toBe(false);
    expect(HOOK_LIMITS).toEqual({
      perLayer: 64,
      runArgs: 64,
      argBytes: 4096,
      reasonBytes: 1024,
      textBytes: 16384,
      contextBytes: 65536,
      stdoutBytes: 65536,
      payloadBytes: 1048576,
      timeoutS: { default: 10, max: 60 },
      processes: 8,
      queue: 256,
    });
  });

  test("event_literals_match_json_schema", () => {
    const schema = JSON.parse(readFileSync(new URL("../../core-types/schema/fin-core.schema.json", import.meta.url), "utf8")) as {
      $defs: Record<string, { enum?: string[] }>;
    };
    expect([...HookEvent.literals]).toEqual(schema.$defs.HookEvent!.enum);
    // Exhaustive: a fourteenth event fails to compile here until it is listed.
    const all = {
      SessionStart: true,
      SessionEnd: true,
      UserPrompt: true,
      PreCommand: true,
      PostCommand: true,
      BranchReady: true,
      PreMerge: true,
      PostMerge: true,
      PreRender: true,
      PostRender: true,
      JobDone: true,
      Approval: true,
      Notification: true,
    } satisfies Record<HookEvent, true>;
    expect(Object.keys(all)).toEqual([...HookEvent.literals]);
  });
});
