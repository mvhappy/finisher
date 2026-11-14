import { test } from "node:test";
import assert from "node:assert/strict";
import { join } from "node:path";
import { childrenOf, contextsOf, projectHooks, script, STEPS, userHooks } from "./hooks-lib.mjs";

test("the scenario has the plan's seven steps, in order", () => {
  assert.equal(STEPS.length, 7);
  assert.match(STEPS[0], /untrusted/);
  assert.match(STEPS[2], /HOOK_BLOCKED/);
  assert.match(STEPS[4], /merge/);
  assert.match(STEPS[6], /server\.status\.hooks/);
});

test("the project hooks match item.trim and name a project-local script", () => {
  const { hooks } = projectHooks({ context: "ctx" });
  assert.deepEqual(hooks.map((h) => [h.id, h.event, h.match, h.file]), [["no-trim", "PreCommand", "item.trim", "hook.mjs"], ["style", "UserPrompt", undefined, "hook.mjs"]]);
  assert.ok(hooks.every((h) => h.run[0] === "node" && h.run[1] === "hook.mjs"));
});

test("the user layer has PreMerge, and the probe only on request", () => {
  assert.deepEqual(userHooks({ project: "/p", probe: false }).map((h) => h.event), ["PreMerge"]);
  const [, probe] = userHooks({ project: "/p", probe: true });
  assert.deepEqual(probe.run.slice(-2), ["--write", join("/p", "planted.txt")]);
});

test("the session script runs one trim per turn", () => {
  const [s] = script(3);
  assert.equal(s.turns.length, 3);
  assert.deepEqual(s.turns[0][0].spawn.slice(1), ["item", "trim", "c1", "--end", "-0.5s"]);
  assert.deepEqual(s.turns[2].at(-1), { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] });
});

test("children and contexts are read from a session's nodes", () => {
  const nodes = [
    { kind: "notice", payload: { child: { code: 7, stdout: '{"error":{"code":"HOOK_BLOCKED"}}', stderr: "" } } },
    { kind: "notice", payload: { child: { code: 0, stdout: "x", stderr: "" } } },
    { kind: "notice", payload: { hookContext: { event: "UserPrompt", hooks: ["style"], text: "t" } } },
    { kind: "message", payload: {} },
  ];
  assert.deepEqual(childrenOf(nodes).map((c) => [c.code, c.error]), [[7, "HOOK_BLOCKED"], [0, undefined]]);
  assert.equal(contextsOf(nodes).length, 1);
});
