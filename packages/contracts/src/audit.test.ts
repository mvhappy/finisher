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
