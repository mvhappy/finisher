import { test } from "node:test";
import assert from "node:assert/strict";
import { blake3 } from "./blake3.mjs";

// The BLAKE3 project's test vectors (`test_vectors.json`): input byte i is i % 251.
const VECTORS = {
  0: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
  1: "2d3adedff11b61f14c886e35afa036736dcd87a74d27b5c1510225d0f592e213",
  64: "4eed7141ea4a5cd4b788606bd23f46e212af9cacebacdc7d1f4c6dc7f2511b98",
  1023: "10108970eeda3eb932baac1428c7a2163b0e924c9a9e25b35bba72b28f70bd11",
  1024: "42214739f095a406f3fc83deb889744ac00df831c10daa55189b5d121c855af7",
  1025: "d00278ae47eb27b34faecf67b4fe263f82d5412916c1ffd97c8cb7fb814b8444",
  3072: "b98cb0ff3623be03326b373de6b9095218513e64f1ee2edd2525c7ad1e5cffd2",
  31744: "62b6960e1a44bcc1eb1a611a8d6235b6b4b78f32e7abc4fb4c6cdcce94895c47",
};

test("blake3 matches the published test vectors", () => {
  for (const [n, want] of Object.entries(VECTORS)) {
    const input = Uint8Array.from({ length: Number(n) }, (_, i) => i % 251);
    assert.equal(blake3(input), want, `length ${n}`);
  }
});
