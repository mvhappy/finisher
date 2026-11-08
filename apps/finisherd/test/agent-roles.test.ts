// Role schema v1 (plan 7B Task 3, Decision 14): the seven bundled roles are complete data, a
// broken role file fails the start naming file and field, and a role only ever narrows.
import { cpSync, mkdtempSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { Schema } from "effect";
import { Rule } from "@finisher/contracts";
import { describe, expect, it } from "vitest";
import { MODE_SCOPES, NEVER_SESSION_SCOPES } from "../src/agent/credentials.ts";
import { BUNDLED_ROLES, loadRoles, MODE_RANK, roleModel } from "../src/agent/roles.ts";

const ROLES_DIR = new URL("../src/agent/roles/", import.meta.url);
const AGENT_NAMES_RS = new URL("../../../crates/fin-core/src/events/agent_names.rs", import.meta.url);
const NAMES = ["caption-stylist", "clip-finder", "colourist", "critic", "editor", "fixer", "mixer"];

/** A copy of the bundled roles in a temporary directory, with `edit` applied to one file. */
function broken(file: string, edit: (role: Record<string, unknown>) => void): URL {
  const dir = mkdtempSync(join(tmpdir(), "fin-roles-"));
  cpSync(new URL(".", ROLES_DIR), dir, { recursive: true });
  const path = join(dir, file);
  const role = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
  edit(role);
  writeFileSync(path, JSON.stringify(role));
  return pathToFileURL(`${dir}/`);
}

function invalid(dir: URL): { code?: string; details?: { file?: string; field?: string; reason?: string } } {
  try {
    loadRoles(dir);
  } catch (e) {
    return e as { code?: string; details?: { file?: string; field?: string; reason?: string } };
  }
  return {};
}

describe("bundled roles", () => {
  it("every bundled role decodes", () => {
    expect([...BUNDLED_ROLES.keys()].sort()).toEqual(NAMES);
    expect(readdirSync(ROLES_DIR).filter((f) => f.endsWith(".json")).length).toBe(NAMES.length);
    for (const role of BUNDLED_ROLES.values()) {
      expect(role.title.length).toBeGreaterThan(0);
      expect(role.prompt.text.split("\n\n").length).toBeGreaterThanOrEqual(3);
      expect(role.prompt.skills.length).toBeGreaterThan(0);
      expect(role.tools.allow).toContain("fin");
      expect(MODE_RANK[role.defaultMode]).toBeLessThanOrEqual(MODE_RANK[role.modeCeiling]);
      expect(role.models.builtin).toBeDefined();
    }
  });

  it("bundled_roles_carry_default_rules", () => {
    for (const role of BUNDLED_ROLES.values()) {
      const by = (cmd: string) => role.rules.find((r) => r.command === cmd);
      expect(by("plugin.install")?.effect).toBe("deny");
      expect(by("render")).toMatchObject({ effect: "ask", where: { minutesOver: 10, mode: "draft" } });
      expect(by("asset.add")).toMatchObject({ effect: "ask", where: { unlicensedDownload: true } });
      expect(by("workspace.gc")?.effect).toBe("ask");
      for (const r of role.rules) {
        expect(r).toMatchObject({ layer: "role", source: `role:${role.name}` });
        expect(Schema.decodeUnknownSync(Rule)(r)).toEqual(r);
      }
      expect(new Set(role.rules.map((r) => r.id)).size).toBe(role.rules.length);
    }
  });

  it("spawn_roles_are_known_names", () => {
    for (const role of BUNDLED_ROLES.values()) for (const n of role.spawn?.roles ?? []) expect(BUNDLED_ROLES.has(n)).toBe(true);
    expect(BUNDLED_ROLES.get("editor")?.spawn?.roles.slice().sort()).toEqual(NAMES.filter((n) => n !== "editor"));
    expect(BUNDLED_ROLES.get("clip-finder")?.spawn?.roles).toEqual(["clip-finder"]);
    expect(BUNDLED_ROLES.get("critic")?.spawn?.roles).toEqual(["fixer"]);
    for (const n of ["fixer", "caption-stylist", "colourist", "mixer"]) expect(BUNDLED_ROLES.get(n)?.spawn).toBeUndefined();
  });

  it("AGENT_ROLES holds every bundled name", () => {
    const rs = readFileSync(AGENT_NAMES_RS, "utf8");
    const table = /AGENT_ROLES: NameTable = NameTable::new\(\s*"AGENT_ROLES",\s*&\[([^\]]*)\]/.exec(rs)?.[1] ?? "";
    const names = [...table.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
    for (const n of BUNDLED_ROLES.keys()) expect(names).toContain(n);
  });

  it("role_narrows_never_widens", () => {
    for (const role of BUNDLED_ROLES.values()) {
      // The ceiling picks the scope set; a role adds none, and no mode holds a human-only scope.
      for (const s of MODE_SCOPES[role.modeCeiling]) expect(NEVER_SESSION_SCOPES.has(s)).toBe(false);
      // The tools are an allow-set of known names, never a scope.
      for (const t of role.tools.allow) expect(t).toMatch(/^[A-Za-z_]+$/);
    }
    expect(invalid(broken("critic.json", (r) => { r.defaultMode = "auto"; })).details).toMatchObject({ file: "critic.json", field: "defaultMode" });
    expect(invalid(broken("critic.json", (r) => { r.tools = { allow: ["fin", "agent.approve"] }; })).details).toMatchObject({ file: "critic.json", field: "tools.allow[1]" });
    expect(invalid(broken("critic.json", (r) => { (r.rules as unknown[]).push({ id: "x", effect: "allow", command: "agent.approve" }); })).details).toMatchObject({ file: "critic.json", field: "rules[4].command" });
    // A role cannot claim another layer or source for its rules.
    expect(invalid(broken("critic.json", (r) => { (r.rules as Record<string, unknown>[])[0]!.layer = "managed"; })).details).toMatchObject({ file: "critic.json", field: "rules[0].layer" });
  });
});

describe("role_invalid_fails_start", () => {
  it("names the file and the field", () => {
    const cases: Array<[string, (r: Record<string, unknown>) => void, string]> = [
      ["editor.json", (r) => { r.name = "Editor"; }, "name"],
      ["editor.json", (r) => { r.name = "mixer"; }, "name"],
      ["editor.json", (r) => { delete r.title; }, "title"],
      ["editor.json", (r) => { r.prompt = { text: "", skills: [] }; }, "prompt.text"],
      ["editor.json", (r) => { r.prompt = { text: "x", skills: ["Not A Topic"] }; }, "prompt.skills[0]"],
      ["editor.json", (r) => { r.modeCeiling = "root"; }, "modeCeiling"],
      ["editor.json", (r) => { r.models = { builtin: 4 }; }, "models.builtin"],
      ["editor.json", (r) => { r.budget = { tokens: 0 }; }, "budget.tokens"],
      ["editor.json", (r) => { r.budget = { gold: 1 }; }, "budget.gold"],
      ["editor.json", (r) => { r.spawn = { roles: ["nobody"] }; }, "spawn.roles[0]"],
      ["editor.json", (r) => { r.spawn = { roles: [], maxChildren: 0 }; }, "spawn.maxChildren"],
      ["editor.json", (r) => { r.rules = [{ id: "a", effect: "maybe", command: "render" }]; }, "rules[0].effect"],
      ["editor.json", (r) => { r.rules = [{ id: "a", effect: "ask", command: "render" }, { id: "a", effect: "ask", command: "asset.add" }]; }, "rules[1].id"],
      ["editor.json", (r) => { r.tools = { allow: [] }; }, "tools.allow"],
      ["editor.json", (r) => { r.extra = 1; }, "extra"],
    ];
    for (const [file, edit, field] of cases) {
      const e = invalid(broken(file, edit));
      expect(e.code, field).toBe("ROLE_INVALID");
      expect(e.details, field).toMatchObject({ file, field });
      expect(e.details?.reason?.length, field).toBeGreaterThan(0);
    }
  });

  it("a file that is not JSON fails too", () => {
    const dir = mkdtempSync(join(tmpdir(), "fin-roles-"));
    writeFileSync(join(dir, "editor.json"), "{");
    const e = invalid(pathToFileURL(`${dir}/`));
    expect(e.code).toBe("ROLE_INVALID");
    expect(e.details).toMatchObject({ file: "editor.json", field: "" });
  });
});

describe("roleModel", () => {
  it("picks the role's model for the harness", () => {
    const editor = BUNDLED_ROLES.get("editor")!;
    expect(roleModel({ ...editor, models: { builtin: "a/b", claudeCode: "c", codex: "d", local: "l" } }, "builtin", "openrouter")).toBe("a/b");
    expect(roleModel({ ...editor, models: { builtin: "a/b", claudeCode: "c", codex: "d", local: "l" } }, "builtin", "local")).toBe("l");
    expect(roleModel({ ...editor, models: { builtin: "a/b", claudeCode: "c", codex: "d" } }, "claudeCode")).toBe("c");
    expect(roleModel({ ...editor, models: { builtin: "a/b", claudeCode: "c", codex: "d" } }, "codex")).toBe("d");
    expect(roleModel({ ...editor, models: { builtin: "a/b" } }, "codex")).toBeUndefined();
  });
});
