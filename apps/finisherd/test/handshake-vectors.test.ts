// The handshake MACs against the shared vector file (plan 5A Task 2; Task 4 adds the Rust twin).
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { b64u, canonicalEndpoint, credentialKey, helloProof, LABEL, macInput, nonce, proveMac, sameMac } from "../src/auth/crypto.ts";

interface Vectors {
  labels: typeof LABEL;
  keys: Record<string, string>;
  handshakes: Array<{
    name: string;
    key: string;
    envId: string;
    endpoint: string;
    clientNonce: string;
    serverNonce: string;
    helloInput: string;
    proof: string;
    proveInput: string;
    mac: string;
  }>;
  credentialKeys: Array<{ envId: string; kind: "agent" | "mcp"; id: string; input: string; key: string }>;
}

const vectors = JSON.parse(
  readFileSync(new URL("../../../tests/fixtures/auth/handshake.json", import.meta.url), "utf8"),
) as Vectors;
const hex = (s: string) => Buffer.from(s, "hex");
const key = (name: string) => b64u.decode(vectors.keys[name]!)!;

describe("handshake crypto", () => {
  test("handshake_vectors_match", () => {
    expect(vectors.labels).toEqual(LABEL);
    expect(vectors.handshakes.length).toBeGreaterThanOrEqual(5);
    for (const v of vectors.handshakes) {
      const k = key(v.key);
      const [cn, sn] = [hex(v.clientNonce), hex(v.serverNonce)];
      expect(macInput(LABEL.hello, [v.envId, v.endpoint, cn, sn]).toString("hex"), v.name).toBe(v.helloInput);
      expect(macInput(LABEL.prove, [v.envId, v.endpoint, sn, cn]).toString("hex"), v.name).toBe(v.proveInput);
      expect(helloProof(k, v.envId, v.endpoint, cn, sn).toString("hex"), v.name).toBe(v.proof);
      expect(proveMac(k, v.envId, v.endpoint, sn, cn).toString("hex"), v.name).toBe(v.mac);
    }
    const master = key("credentialMaster");
    for (const c of vectors.credentialKeys) {
      expect(macInput(LABEL.credentialKey, [c.envId, c.kind, c.id]).toString("hex")).toBe(c.input);
      expect(credentialKey(master, c.envId, c.kind, c.id).toString("hex")).toBe(c.key);
    }
    // Review of Tasks 8–9, M3: the environment id is bound in, so another server data's copy
    // of the same master derives a different key for the same credential.
    const [a, , other] = vectors.credentialKeys;
    expect(other!.id).toBe(a!.id);
    expect(other!.envId).not.toBe(a!.envId);
    expect(other!.key).not.toBe(a!.key);
  });

  test("length_prefixes_make_fields_unambiguous", () => {
    const [a, b] = vectors.handshakes.filter((v) => v.name.startsWith("byte moved"));
    // The same concatenated bytes, split differently between envId and endpoint.
    expect(a!.envId + a!.endpoint).toBe(b!.envId + b!.endpoint);
    expect(a!.proof).not.toBe(b!.proof);
    expect(a!.mac).not.toBe(b!.mac);
    // Proof and MAC differ (label and nonce order), and either key changes everything.
    const v = vectors.handshakes[0]!;
    expect(v.proof).not.toBe(v.mac);
    const other = proveMac(key("console"), v.envId, v.endpoint, hex(v.serverNonce), hex(v.clientNonce));
    expect(other.toString("hex")).not.toBe(v.mac);
  });

  test("inputs are checked", () => {
    const k = key("local");
    expect(() => helloProof(k, "e", "x", Buffer.alloc(31), nonce())).toThrow(RangeError);
    expect(() => helloProof(Buffer.alloc(16), "e", "x", nonce(), nonce())).toThrow(RangeError);
    expect(() => proveMac(k, "e".repeat(70_000), "x", nonce(), nonce())).toThrow(RangeError);
    expect(nonce()).toHaveLength(32);
    expect(nonce().equals(nonce())).toBe(false);
  });

  test("sameMac and b64u", () => {
    const m = hex(vectors.handshakes[0]!.mac);
    expect(sameMac(m, Buffer.from(m))).toBe(true);
    expect(sameMac(m, m.subarray(0, 31))).toBe(false);
    const flipped = Buffer.from(m);
    flipped[0] = (flipped[0] ?? 0) ^ 1;
    expect(sameMac(m, flipped)).toBe(false);
    expect(b64u.decode(b64u.encode(m))?.equals(m)).toBe(true);
    for (const bad of ["a+b/", "abc=", "a", "ab c", "AB"]) expect(b64u.decode(bad), bad).toBeNull();
  });

  test("canonical_endpoint_resolves_symlinks_and_lowercases_pipes", () => {
    expect(canonicalEndpoint("\\\\.\\pipe\\Finisher-Ann-ABC", "win32")).toBe("\\\\.\\pipe\\finisher-ann-abc");
    expect(canonicalEndpoint("\\\\.\\PIPE\\X", "linux")).toBe("\\\\.\\pipe\\x");
    if (process.platform === "win32") return;
    const tmp = mkdtempSync(join(tmpdir(), "endpoint-"));
    try {
      const real = join(tmp, "real", "run");
      mkdirSync(real, { recursive: true });
      writeFileSync(join(real, "fin.sock"), "");
      symlinkSync(join(tmp, "real"), join(tmp, "link"));
      const canonical = canonicalEndpoint(join(real, "fin.sock"));
      expect(canonicalEndpoint(join(tmp, "link", "run", "fin.sock"))).toBe(canonical);
      expect(canonicalEndpoint(join(tmp, "link", "run", "..", "run", "fin.sock"))).toBe(canonical);
      // Not created yet: the directory's real path.
      expect(canonicalEndpoint(join(tmp, "link", "run", "other.sock"))).toBe(join(canonical, "..", "other.sock"));
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
  });
});
