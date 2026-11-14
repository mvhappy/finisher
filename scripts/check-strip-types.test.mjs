import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const gate = (root) => runGate("./check-strip-types.mjs", ["--root", root]);

test("erasable TypeScript passes", () => {
  const r = gate(
    fixtureRoot({
      "apps/x/src/a.ts": "export interface I { a: number }\nexport class C { readonly #s: number; constructor(s: number) { this.#s = s; } get s(): number { return this.#s; } }\nexport const f = (x: I): number => x.a as number;\n",
      "packages/y/src/types.d.ts": "export declare enum Fine { A }\n",
    }),
  );
  assert.equal(r.code, 0, r.out);
});

test("a parameter property fails, naming the file", () => {
  const r = gate(fixtureRoot({ "apps/x/src/plan.ts": "export class Plan { constructor(private readonly state: number) {} }\n", "apps/x/src/ok.ts": "export const a = 1;\n" }));
  assert.equal(r.code, 1);
  assert.match(r.out, /STRIP_TYPES_FAILED: apps\/x\/src\/plan\.ts/);
  assert.doesNotMatch(r.out, /ok\.ts/);
});

test("an enum and a value namespace fail", () => {
  const r = gate(fixtureRoot({ "a.mts": "export enum E { A }\n", "b.cts": "namespace N { export const x = 1; }\n" }));
  assert.equal(r.code, 1);
  assert.match(r.out, /STRIP_TYPES_FAILED: a\.mts/);
  assert.match(r.out, /STRIP_TYPES_FAILED: b\.cts/);
});
