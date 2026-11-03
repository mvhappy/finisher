import { readFileSync } from "node:fs";
import { expect, test } from "vitest";

const ts = readFileSync(new URL("./generated/index.ts", import.meta.url), "utf8");
const block = (name: string) => ts.match(new RegExp(`export interface ${name} \\{[\\s\\S]*?\n\\}`))?.[0] ?? "";

test("Rational-typed fields keep the Rational type", () => {
  expect(block("Clip")).toMatch(/start: Rational;/);
  expect(block("Clip")).toMatch(/sourceIn: Rational;/);
  expect(block("Project")).toMatch(/rate: Rational;/);
});

test("CoreError.details is required", () => {
  expect(block("CoreError")).toMatch(/\n  details: unknown;/);
});
