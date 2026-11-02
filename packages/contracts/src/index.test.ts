import { expect, test } from "vitest";
import { PROTOCOL_VERSION } from "./index.ts";

test("protocol version starts at 1", () => {
  expect(PROTOCOL_VERSION).toBe(1);
});
