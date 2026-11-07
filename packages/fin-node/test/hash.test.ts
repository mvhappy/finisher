// BLAKE3 through napi (review M3): the agent runtime's bodyHash, systemHash and toolsHash. Checked
// against the official test vectors (BLAKE3-team/BLAKE3, test_vectors/test_vectors.json, CC0):
// input byte i is i % 251; the key is "whats the Elvish word for friend".
import { describe, expect, it } from "vitest";
import { blake3Hex, blake3KeyedHex } from "../index.js";

const input = (n: number) => Buffer.from(Array.from({ length: n }, (_, i) => i % 251));
const KEY = Buffer.from("whats the Elvish word for friend");

describe("blake3", () => {
  it("hashes as the official vectors say", () => {
    expect(blake3Hex(input(0))).toBe("af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    expect(blake3Hex(input(1))).toBe("2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213");
    expect(blake3Hex(input(1025))).toBe("d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444");
    expect(blake3Hex(input(102400))).toBe("bc3e3d41a1146b069abffad3c0d44860cf664390afce4d9661f7902e7943e085");
  });

  it("keys with a 32-byte key and refuses another length", () => {
    expect(blake3KeyedHex(KEY, input(0))).toBe("92b2b75604ed3c761f9d6f62392c8a9227ad0ea3f09573e783f1498a4ed60d26");
    expect(blake3KeyedHex(KEY, input(1025))).toBe("357dc55de0c7e382c900fd6e320acc04146be01db6a8ce7210b7189bd664ea69");
    expect(() => blake3KeyedHex(Buffer.alloc(16), input(1))).toThrow(/32 bytes/);
  });
});
