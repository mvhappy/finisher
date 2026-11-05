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

test("the envelope types above fin-core are generated too", () => {
  // fin-probe's Probe (asset.probe), with Rational times.
  expect(block("Probe")).toMatch(/duration: Rational;/);
  expect(block("Probe")).toMatch(/size: number;/);
  expect(block("Probe")).toMatch(/video\?: VideoInfo \| null;|video: VideoInfo \| null;/);
  // fin-store's Generation, tx.get's set-aside answer, and the engine events.
  expect(block("Generation")).toMatch(/incarnation: string;/);
  expect(block("Generation")).toMatch(/n: number;/);
  expect(block("TxSetAside")).toMatch(/setAside: SetAside;/);
  expect(block("SetAside")).toMatch(/txId: string;/);
  expect(ts).toMatch(/export type EngineEvent =/);
  for (const type of ["applied", "status", "rewound", "rebuilt", "error", "lagged"]) {
    expect(ts).toContain(`type: "${type}";`);
  }
});

test("Tx carries the optional argsHash", () => {
  expect(block("Tx")).toMatch(/argsHash\?: string \| null;|argsHash\?: string;/);
});
