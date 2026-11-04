// Unit tests for the e2e script's own assertions (the full run is `pnpm e2e`).
import { test } from "node:test";
import assert from "node:assert/strict";
import { checkRevs, parseFrames } from "./e2e.mjs";

test("parseFrames splits one message carrying several NDJSON frames", () => {
  const text = `${JSON.stringify({ _tag: "Chunk", requestId: "1", values: [] })}\n${JSON.stringify({ _tag: "Exit", requestId: "1" })}\n`;
  assert.deepEqual(
    parseFrames(text).map((f) => f._tag),
    ["Chunk", "Exit"],
  );
  assert.deepEqual(parseFrames("\n"), []);
});

test("checkRevs accepts every committed rev exactly once, in order", () => {
  assert.equal(checkRevs([1, 2, 3], [1, 2, 3]), undefined);
});

test("checkRevs rejects a gap, a duplicate and a reordering", () => {
  assert.match(checkRevs([1, 3], [1, 2, 3]), /subscriber saw revs \[1,3\]/);
  assert.match(checkRevs([1, 2, 2, 3], [1, 2, 3]), /subscriber saw revs/);
  assert.match(checkRevs([2, 1, 3], [1, 2, 3]), /subscriber saw revs/);
});

test("checkRevs rejects non-contiguous commits from fin", () => {
  assert.match(checkRevs([1, 3], [1, 3]), /not contiguous/);
});
