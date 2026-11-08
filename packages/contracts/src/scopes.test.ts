import { expect, test } from "vitest";
import { defaultScopes } from "./auth.ts";
import { FinRpcs } from "./rpc.ts";
import { HUMAN_ONLY, RPC_SCOPES, requiredScopes, type ResolverName, ruleOf, SCOPES, WRITE_RPCS } from "./scopes.ts";

/** The resolver a payload field needs (spec §4.4 stage 3). */
const NEEDS: Record<string, ResolverName> = {
  projectId: "project",
  pinned: "project",
  branch: "branch",
  from: "branch",
  into: "branch",
  tx: "branch",
  path: "probePath",
  importDir: "importDir",
};

test("rpc_scopes_cover_every_rpc", () => {
  const tags = [...FinRpcs.requests.keys()].sort();
  expect(Object.keys(RPC_SCOPES).sort()).toEqual(tags);
  for (const tag of tags) {
    const rule = ruleOf(tag as keyof typeof RPC_SCOPES);
    expect(rule.scopes.length, tag).toBeGreaterThan(0);
    for (const s of rule.scopes) expect(SCOPES, tag).toContain(s);
    const rpc = FinRpcs.requests.get(tag) as unknown as { payloadSchema: { fields?: Record<string, unknown> } };
    const fields = Object.keys(rpc.payloadSchema.fields ?? {});
    for (const f of fields) {
      const need = f === "dir" ? (tag === "project.create" ? "createDir" : "openDir") : NEEDS[f];
      if (need !== undefined) expect(rule.resolve ?? [], `${tag}.${f}`).toContain(need);
    }
    // Every write target is a payload field the branch resolver covers.
    for (const w of rule.writes ?? []) expect(fields, tag).toContain(w.split(".")[0]);
    if (rule.author !== undefined) expect(fields, tag).toContain(rule.author);
  }
  // Only the handshake is public.
  expect(tags.filter((t) => RPC_SCOPES[t as keyof typeof RPC_SCOPES].scopes.includes("public"))).toEqual(["auth.prove", "server.hello"]);
});

test("escalations_add_the_human_only_scopes", () => {
  expect(requiredScopes("project.takeover", { projectId: "p", confirm: false })).toEqual(["project.admin"]);
  expect(requiredScopes("project.takeover", { projectId: "p", confirm: true })).toEqual(["project.admin", "project.confirm"]);
  expect(requiredScopes("project.open", { dir: "/d" }, { attached: true })).toEqual(["project.read"]);
  expect(requiredScopes("project.open", { dir: "/d" }, { attached: false })).toEqual(["project.read", "project.write"]);
  expect(requiredScopes("project.open", { dir: "/d", relocate: true }, { attached: true })).toEqual(["project.read", "project.confirm"]);
});

test("diagnostics_admin_guards_consent_purge_project_default_and_content (5A Task 7)", () => {
  expect(requiredScopes("diagnostics.consent", { purpose: "diagnostics", value: "off" })).toEqual(["diagnostics.admin"]);
  expect(requiredScopes("diagnostics.purge", { selector: "all" })).toEqual(["diagnostics.admin"]);
  expect(requiredScopes("diagnostics.project", { projectId: "p", collect: "never" })).toEqual(["diagnostics.read"]);
  expect(requiredScopes("diagnostics.project", { projectId: "p", collect: "default" })).toEqual(["diagnostics.read", "diagnostics.admin"]);
  expect(requiredScopes("diagnostics.export", {})).toEqual(["diagnostics.read"]);
  expect(requiredScopes("diagnostics.export", { content: true })).toEqual(["diagnostics.read", "diagnostics.admin"]);
  // Training on is a device's alone; nothing else is device-only.
  const deviceOnly = (tag: keyof typeof RPC_SCOPES, p: unknown) => ruleOf(tag).deviceOnly?.(p as never) === true;
  expect(deviceOnly("diagnostics.consent", { purpose: "training", value: "on" })).toBe(true);
  expect(deviceOnly("diagnostics.consent", { purpose: "training", value: "off" })).toBe(false);
  expect(deviceOnly("diagnostics.consent", { purpose: "diagnostics", value: "on" })).toBe(false);
  expect(Object.keys(RPC_SCOPES).filter((t) => ruleOf(t as keyof typeof RPC_SCOPES).deviceOnly !== undefined)).toEqual(["diagnostics.consent"]);
});

test("confirm_marks_the_mutating_human_only_rpcs (Decision 30)", () => {
  const confirmed = Object.keys(RPC_SCOPES).filter((t) => ruleOf(t as keyof typeof RPC_SCOPES).confirm === true);
  expect(confirmed.sort()).toEqual(["diagnostics.consent", "diagnostics.project", "diagnostics.purge", "project.open", "project.takeover"]);
  // Each takes the typed code; reading content and asking for a code need none.
  for (const tag of confirmed) {
    const rpc = FinRpcs.requests.get(tag) as unknown as { payloadSchema: { fields?: Record<string, unknown> } };
    expect(Object.keys(rpc.payloadSchema.fields ?? {}), tag).toContain("challenge");
  }
  expect(ruleOf("diagnostics.export").confirm).toBeUndefined();
  expect(ruleOf("console.challenge").confirm).toBeUndefined();
});

test("defaults_follow_owner_decision_5", () => {
  for (const s of HUMAN_ONLY) expect(SCOPES).toContain(s);
  const local = defaultScopes("local");
  for (const s of HUMAN_ONLY) expect(local).not.toContain(s);
  expect(local).toContain("branch.protected");
  expect(local).toContain("review.decide");
  expect(defaultScopes("console")).toEqual(SCOPES);
  expect(defaultScopes("anonymous")).toEqual(["public"]);
  expect(defaultScopes("agent")).toEqual([]);
});

test("writes_required_on_writing_rules: exactly the RPCs whose handlers write through the guard", () => {
  // The handlers that pass a write guard to the store (`meta(options)` in finisherd's handlers).
  expect([...WRITE_RPCS].sort()).toEqual(
    [
      "branch.create",
      "branch.set",
      "history.checkpoint",
      "history.cherryPick",
      "history.merge",
      "history.redo",
      "history.revert",
      "history.undo",
      "project.recover",
      "tx.commit",
    ].sort(),
  );
  // A new branch's author is stamped by the server for branch-limited principals.
  expect(ruleOf("branch.create").author).toBe("author");
  expect(ruleOf("project.recover").author).toBe("author");
});

test("reopening_a_closed_project_needs_project_write", () => {
  expect(requiredScopes("project.status", { projectId: "p" }, { reopens: false })).toEqual(["project.read"]);
  expect(requiredScopes("project.status", { projectId: "p" }, { reopens: true })).toEqual(["project.read", "project.write"]);
  expect(requiredScopes("tx.commit", { projectId: "p" }, { reopens: true })).toEqual(["project.write"]);
});

test("human_only_needs_follow_the_payload_alone (never whether a project is open)", () => {
  const facts = [{}, { attached: true }, { attached: false }, { reopens: true }, { attached: true, reopens: true }];
  const payloads: Array<[keyof typeof RPC_SCOPES, unknown]> = [
    ["project.open", { dir: "/d" }],
    ["project.open", { dir: "/d", relocate: true }],
    ["project.takeover", { projectId: "p", confirm: true }],
    ["project.takeover", { projectId: "p", confirm: false }],
    ["diagnostics.project", { projectId: "p", collect: "default" }],
    ["diagnostics.project", { projectId: "p", collect: "never" }],
  ];
  for (const [tag, payload] of payloads) {
    const human = facts.map((f) => requiredScopes(tag, payload, f).filter((s) => HUMAN_ONLY.has(s)).join(","));
    expect(new Set(human).size, `${tag} ${JSON.stringify(payload)}`).toBe(1);
  }
});
