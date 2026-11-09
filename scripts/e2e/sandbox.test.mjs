import { test } from "node:test";
import assert from "node:assert/strict";
import { childResults, children, script } from "./sandbox.mjs";

const kids = children({ serverData: "/s d", project: "/p's", outside: "/o.txt", track: "t3" });

test("each child is one sandboxed spawn, then the turn ends", () => {
  const [s] = script(kids);
  const steps = s.turns[0];
  assert.equal(steps.length, Object.keys(kids).length + 1);
  for (const [i, c] of Object.values(kids).entries()) assert.deepEqual(steps[i], { spawn: ["/bin/sh", "-c", c] });
  assert.deepEqual(steps.at(-1), { emit: [{ type: "turnCompleted", stopReason: "endTurn" }] });
});

test("paths are quoted for the shell", () => {
  assert.match(kids.key, /^cat '\/s d\/auth\/local\.key'$/);
  assert.match(kids.project, /'\/p'\\''s\/planted\.txt'$/);
  assert.match(kids.hosts, /--track t3 --words \/etc\/hosts/);
});

test("child notices are matched to their children, and a missing one fails", () => {
  const notice = (code) => ({ kind: "notice", payload: { child: { code, stdout: "", stderr: "" } } });
  const nodes = [{ kind: "message", payload: {} }, notice(0), notice(1)];
  assert.deepEqual(childResults(nodes, ["a", "b"]), { a: notice(0).payload.child, b: notice(1).payload.child });
  assert.throws(() => childResults(nodes, ["a", "b", "c"]), /expected 3 children, saw 2/);
});
