import { expect, test } from "vitest";
import { defaultScopes } from "./auth.ts";
import { FinRpcs } from "./rpc.ts";
import { HUMAN_ONLY, RPC_SCOPES, requiredScopes, type ResolverName, SCOPES } from "./scopes.ts";

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
    const rule = RPC_SCOPES[tag as keyof typeof RPC_SCOPES];
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
