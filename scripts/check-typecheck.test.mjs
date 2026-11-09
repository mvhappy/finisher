import { test } from "node:test";
import assert from "node:assert/strict";
import { fixtureRoot, runGate } from "./gate-helpers.mjs";

const PROJECTS = ["apps/finisherd", "packages/contracts", "packages/core-types", "packages/fin-node", "packages/plugin-sdk"];
const WORKSPACE = 'packages:\n  - "packages/*"\n  - "apps/*"\n';
const tsconfig = JSON.stringify({ compilerOptions: { strict: true, noEmit: true, module: "NodeNext", moduleResolution: "NodeNext" }, include: ["src"] });
const files = (broken, projects = PROJECTS) => ({
  "pnpm-workspace.yaml": WORKSPACE,
  ...Object.fromEntries(
    projects.flatMap((p) => [
      [`${p}/package.json`, "{}"],
      [`${p}/tsconfig.json`, tsconfig],
      [`${p}/src/a.ts`, p === broken ? 'export const a: number = "not a number";\n' : "export const a: number = 1;\n"],
    ]),
  ),
});
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

test("a package the workspace gained is checked without editing the gate", () => {
  const r = gate(fixtureRoot(files("packages/brand-new", [...PROJECTS, "packages/brand-new"])));
  assert.equal(r.code, 1);
  assert.match(r.out, /TYPECHECK_FAILED: packages\/brand-new/);
});

test("a project without tsconfig.json fails", () => {
  const f = files();
  delete f["packages/fin-node/tsconfig.json"];
  const r = gate(fixtureRoot(f));
  assert.equal(r.code, 1);
  assert.match(r.out, /TYPECHECK_FAILED: packages\/fin-node\/tsconfig\.json is missing/);
});

test("a workspace that lists no package fails", () => {
  const r = gate(fixtureRoot({ "pnpm-workspace.yaml": WORKSPACE }));
  assert.equal(r.code, 1);
  assert.match(r.out, /TYPECHECK_FAILED: pnpm-workspace\.yaml lists no package/);
});
