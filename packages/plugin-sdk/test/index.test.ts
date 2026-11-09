import { expect, test } from "vitest";
import { ENVELOPE_VERSION } from "../src/index.ts";

test("the SDK speaks envelope version 1", () => {
  expect(ENVELOPE_VERSION).toBe(1);
});
