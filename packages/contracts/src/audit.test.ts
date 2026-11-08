import { describe, expect, test } from "vitest";
import { AUDIT_KINDS, auditDetailsValid } from "./audit.ts";

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
    for (const kind of ["pair.redeem", "device.revoke", "device.rename", "credential.revoke", "auth.fail"] as const) {
      expect(auditDetailsValid(kind, named), kind).toBe(true);
    }
    for (const kind of ["auth.denied", "credential.mint", "consent.change", "diagnostics.purge", "control.reset"] as const) {
      expect(auditDetailsValid(kind, named), kind).toBe(false);
      expect(auditDetailsValid(kind, { address: "remote-a" }), kind).toBe(false);
    }
    expect(auditDetailsValid("auth.fail", { address: "remote-a" })).toBe(true);
    expect(auditDetailsValid("pair.offer", { address: "remote-a" })).toBe(true);
    expect(auditDetailsValid("device.revoke", { address: "remote-a" })).toBe(false);
    expect(auditDetailsValid("pair.redeem", { deviceName: "x".repeat(65) })).toBe(false);
  });
});
