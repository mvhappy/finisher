import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const PROJECTS = ["apps/finisherd", "packages/contracts", "packages/core-types", "packages/fin-node", "packages/plugin-sdk"];
const tsconfig = JSON.stringify({ compilerOptions: { strict: true, noEmit: true, module: "NodeNext", moduleResolution: "NodeNext" }, include: ["src"] });
const files = (broken) =>
  Object.fromEntries(
    PROJECTS.flatMap((p) => [
      [`${p}/tsconfig.json`, tsconfig],
      [`${p}/src/a.ts`, p === broken ? 'export const a: number = "not a number";\n' : "export const a: number = 1;\n"],
    ]),
  );
const gate = (root) => runGate("./check-typecheck.mjs", ["--root", root]);

test("projects that typecheck pass", () => {
  const r = gate(fixtureRoot(files()));
  assert.equal(r.code, 0, r.out);
});

test("a seeded type error fails, naming the project", () => {
  const r = gate(fixtureRoot(files("packages/contracts")));
  assert.equal(r.code, 1);
  assert.match(r.out, /TYPECHECK_FAILED: packages\/contracts/);
  assert.match(r.out, /TS2322/);
  assert.doesNotMatch(r.out, /TYPECHECK_FAILED: apps\/finisherd/);
});

test("a project without tsconfig.json fails", () => {
  const f = files();
  delete f["packages/fin-node/tsconfig.json"];
  const r = gate(fixtureRoot(f));
  assert.equal(r.code, 1);
  assert.match(r.out, /TYPECHECK_FAILED: packages\/fin-node\/tsconfig\.json is missing/);
});
