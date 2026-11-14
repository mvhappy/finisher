import { describe, expect, test } from "vitest";
import { Schema } from "effect";
import { AUDIT_KINDS, AuditOutcomeSchema, auditDetailsValid } from "./audit.ts";

describe("audit details", () => {
  test("every kind takes ids, words and counts", () => {
    for (const kind of AUDIT_KINDS) {
      expect(auditDetailsValid(kind, {}), kind).toBe(true);
      expect(
        auditDetailsValid(kind, { rpc: "history.cherryPick", scope: "project.write", code: "SCOPE_DENIED", count: 3, credentialId: "01agent000000000000000000a" }),
        kind,
      ).toBe(true);
    }
  });

  test("plugin_and_registry_kinds_take_ids_a_version_and_a_risk_only", () => {
    const kinds = AUDIT_KINDS.filter((k) => k.startsWith("plugin.") || k.startsWith("registry."));
    expect(kinds).toEqual(expect.arrayContaining(["plugin.install", "plugin.update", "plugin.remove", "plugin.enable", "plugin.disable", "plugin.revoke", "plugin.grant", "registry.add", "registry.remove", "registry.trust", "registry.forget", "registry.verifyFail"]));
    const ok = { pluginId: "@acme/pack", version: "1.2.0-beta.1", registryId: "team", permissionsAdded: 2, risk: "high", code: "TUF_EXPIRED", thirdPartyCode: { accepted: true, confined: false } };
    const sha = { rootSha256: "a".repeat(64) };
    for (const kind of kinds) {
      expect(auditDetailsValid(kind, ok), kind).toBe(true);
      expect(auditDetailsValid(kind, sha), kind).toBe(true);
      for (const bad of [{ pluginId: "acme/pack" }, { registryId: "Team Registry" }, { version: "v 1" }, { risk: "severe" }, { rootSha256: "xyz" }, { path: "/home/someone/plugin" }, { thirdPartyCode: { accepted: true } }, { publisher: "Acme Studio" }]) {
        expect(auditDetailsValid(kind, bad), `${kind} ${JSON.stringify(bad)}`).toBe(false);
      }
    }
    // The fields are the plugin kinds' own.
    expect(auditDetailsValid("auth.denied", { pluginId: "@acme/pack" })).toBe(false);
    expect(auditDetailsValid("consent.change", { registryId: "team" })).toBe(false);
  });

  test("rule_kinds_take_a_rule_id_a_layer_and_a_rules_hash_never_the_pattern", () => {
    const id = "01m6yfjx51brn5wdjt6gsrsq7g";
    for (const kind of ["agent.rule.add", "agent.rule.remove", "agent.project.trust"] as const) {
      expect(auditDetailsValid(kind, { rpc: "agent.rules.add", ruleId: id, layer: "local", value: "deny", projectId: id, sessionId: id, from: id }), kind).toBe(true);
      expect(auditDetailsValid(kind, { rulesHash: `sha256:${"a".repeat(64)}` }), kind).toBe(true);
      expect(auditDetailsValid(kind, { rulesHash: "none" }), kind).toBe(true);
      for (const bad of [{ command: "render" }, { layer: "team" }, { rulesHash: "abc" }, { ruleId: "not an id" }]) expect(auditDetailsValid(kind, bad), `${kind} ${JSON.stringify(bad)}`).toBe(false);
    }
    expect(auditDetailsValid("settings.write", { ruleId: id })).toBe(false);
  });

  test("hook_block_details_are_closed", () => {
    const id = "01m6yfjx51brn5wdjt6gsrsq7g";
    const row = { event: "PreCommand", hookId: "no-friday-trims", layer: "project", session: id, projectId: id, rpc: "item.trim" };
    expect(auditDetailsValid("hook.block", row)).toBe(true);
    expect(auditDetailsValid("hook.block", { ...row, event: "PostCommand", after: true })).toBe(true);
    expect(auditDetailsValid("hook.block", { event: "PreMerge", hookId: "managed.unreadable", layer: "managed" })).toBe(true);
    expect(auditDetailsValid("hook.block", { event: "JobDone", hookId: "x", layer: "plugin" })).toBe(true);
    // A plugin handler's row names its plugin (7F Task 10); never a version or a free word there.
    expect(auditDetailsValid("hook.block", { event: "PreCommand", hookId: "guard", layer: "plugin", pluginId: "@test/probe", projectId: id })).toBe(true);
    for (const bad of ["@test/probe@1.0.0", "no plugin", "../x"]) expect(auditDetailsValid("hook.block", { event: "PreCommand", hookId: "guard", layer: "plugin", pluginId: bad }), bad).toBe(false);
    // Never the reason text (the session's notice holds it), a path, or a free word.
    for (const bad of [{ reason: "no trims on Fridays" }, { reason: "timeout" }, { text: "context" }, { event: "PreOp" }, { layer: "role" }, { hookId: "Has Spaces" }, { hookId: "../x" }, { after: false }, { session: "not an id" }, { path: "/home/someone/hook.mjs" }]) {
      expect(auditDetailsValid("hook.block", { ...row, ...bad }), JSON.stringify(bad)).toBe(false);
    }
    // The fields are hook.block's own.
    expect(auditDetailsValid("auth.denied", { hookId: "x" })).toBe(false);
    expect(auditDetailsValid("hook.modify", { hookId: "x" })).toBe(false);
  });

  test("an auth.denied row may name the card an ask opened (APPROVAL_PENDING, outcome pending); no other kind", () => {
    const id = "01m6yfjx51brn5wdjt6gsrsq7g";
    expect(auditDetailsValid("auth.denied", { rpc: "history.checkpoint", rule: "ask", approval: id })).toBe(true);
    expect(auditDetailsValid("auth.denied", { approval: "not an id" })).toBe(false);
    expect(auditDetailsValid("settings.write", { approval: id })).toBe(false);
    expect(Schema.decodeUnknownSync(AuditOutcomeSchema)("pending")).toBe("pending");
  });

  test("free text, paths and unknown fields are refused", () => {
    for (const details of [
      { path: "/home/someone/film" },
      { reason: "a sentence with spaces" },
      { rpc: "/x" },
      { credentialId: "not-an-id" },
      { count: 1.5 },
      { code: "lower" },
      { scope: "a".repeat(60) },
    ]) {
      expect(auditDetailsValid("auth.denied", details), JSON.stringify(details)).toBe(false);
    }
  });

  test("names and addresses only for pairing, revocation and auth failures", () => {
    const named = { deviceName: "Studio laptop" };
    for (const kind of ["pair.redeem", "device.revoke", "credential.revoke", "auth.fail"] as const) {
      expect(auditDetailsValid(kind, named), kind).toBe(true);
    }
    for (const kind of ["auth.denied", "credential.mint", "consent.change", "diagnostics.purge", "control.reset", "device.rename"] as const) {
      expect(auditDetailsValid(kind, named), kind).toBe(false);
      expect(auditDetailsValid(kind, { address: "remote-a" }), kind).toBe(false);
    }
    expect(auditDetailsValid("auth.fail", { address: "remote-a" })).toBe(true);
    expect(auditDetailsValid("pair.offer", { address: "remote-a" })).toBe(true);
    // Owner decision 4 of 5B: revocations keep the remote address too.
    expect(auditDetailsValid("device.revoke", { address: "remote-a" })).toBe(true);
    expect(auditDetailsValid("device.rename", { address: "remote-a" })).toBe(false);
    expect(auditDetailsValid("device.rename", { deviceId: "01device00000000000000000a" })).toBe(true);
    expect(auditDetailsValid("pair.redeem", { deviceName: "x".repeat(65) })).toBe(false);
  });

  test("a pipe client's peer only for auth failures, as a pid and a SID", () => {
    const peer = { pid: 4242, sid: "S-1-5-21-1000-2000-3000-1001" };
    expect(auditDetailsValid("auth.fail", { reason: "badMac", transport: "pipe", peer })).toBe(true);
    expect(auditDetailsValid("auth.fail", { reason: "peer", transport: "pipe", peer: { pid: 7 } })).toBe(true);
    expect(auditDetailsValid("auth.denied", { peer })).toBe(false);
    expect(auditDetailsValid("auth.fail", { peer: { pid: 1, sid: "DOMAIN\\someone" } })).toBe(false);
    expect(auditDetailsValid("auth.fail", { peer: { pid: 1, user: "someone" } })).toBe(false);
  });
});

describe("5B kinds (Task 1)", () => {
  test("audit_details_closed_for_new_kinds", () => {
    const spki = "A".repeat(43);
    expect(auditDetailsValid("listener.start", { spki, mode: "selfSigned", source: "settings" })).toBe(true);
    expect(auditDetailsValid("listener.start", { spki: "short" })).toBe(false);
    expect(auditDetailsValid("listener.start", { mode: "acme" })).toBe(false);
    expect(auditDetailsValid("listener.stop", {})).toBe(true);
    expect(auditDetailsValid("env.reset", { devices: 2, credentials: 3 })).toBe(true);
    expect(auditDetailsValid("env.moved", { reason: "renewed" })).toBe(true);
    expect(auditDetailsValid("identity.rotate", { reason: "lost", count: 4 })).toBe(true);
    expect(auditDetailsValid("settings.write", { path: "server.remote" })).toBe(true);
    // Never a setting's value, never a path outside the closed list.
    expect(auditDetailsValid("settings.write", { path: "privacy" })).toBe(false);
    expect(auditDetailsValid("settings.write", { value: "0.0.0.0:7311" })).toBe(false);
    // 7D Task 11: Claude Code's keyed account hashes, never a label.
    expect(auditDetailsValid("settings.write", { rpc: "agent.account.confirm", value: "claudeCode.taken", accountHash: "0a1b2c3d", previousHash: "4e5f6a7b" })).toBe(true);
    expect(auditDetailsValid("settings.write", { accountHash: "person@example.invalid" })).toBe(false);
    expect(auditDetailsValid("pair.redeem", { deviceName: "Studio tablet", platform: "ios", address: "127.0.0.1" })).toBe(true);
    expect(auditDetailsValid("pair.redeem", { platform: "amiga" })).toBe(false);
    expect(auditDetailsValid("pair.offer", { label: "for the tablet" })).toBe(true);
    // Names and addresses: only pairing, device revocation and failures (owner decision 4).
    for (const kind of ["listener.start", "env.reset", "identity.rotate", "settings.write", "env.moved"] as const) {
      expect(auditDetailsValid(kind, { deviceName: "x" }), kind).toBe(false);
      expect(auditDetailsValid(kind, { address: "127.0.0.1" }), kind).toBe(false);
      expect(auditDetailsValid(kind, { label: "x" }), kind).toBe(false);
    }
    expect(auditDetailsValid("device.revoke", { deviceName: "x", address: "127.0.0.1", deviceId: "01device00000000000000000a" })).toBe(true);
    expect(AUDIT_KINDS).toContain("identity.rotate");
  });
});

