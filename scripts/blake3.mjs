// BLAKE3 (32-byte output, unkeyed) for the licence-file hashes of `third_party/native.json`.
// Node's crypto has no BLAKE3; this follows the BLAKE3 specification's reference algorithm
// (chunks of 1,024 bytes, 64-byte blocks, 7 rounds, a binary tree of chaining values). It is
// small, scalar and checked against the published test vectors in `blake3.test.mjs`.

const IV = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const CHUNK_LEN = 1024;
const BLOCK_LEN = 64;

const rotr = (x, n) => ((x >>> n) | (x << (32 - n))) >>> 0;

function g(s, a, b, c, d, x, y) {
  s[a] = (s[a] + s[b] + x) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 16);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 12);
  s[a] = (s[a] + s[b] + y) >>> 0;
  s[d] = rotr(s[d] ^ s[a], 8);
  s[c] = (s[c] + s[d]) >>> 0;
  s[b] = rotr(s[b] ^ s[c], 7);
}

function round(s, m) {
  g(s, 0, 4, 8, 12, m[0], m[1]);
  g(s, 1, 5, 9, 13, m[2], m[3]);
  g(s, 2, 6, 10, 14, m[4], m[5]);
  g(s, 3, 7, 11, 15, m[6], m[7]);
  g(s, 0, 5, 10, 15, m[8], m[9]);
  g(s, 1, 6, 11, 12, m[10], m[11]);
  g(s, 2, 7, 8, 13, m[12], m[13]);
  g(s, 3, 4, 9, 14, m[14], m[15]);
}

/** The first 8 words of the compression function's output. */
function compress(cv, words, counter, blockLen, flags) {
  const s = [...cv, IV[0], IV[1], IV[2], IV[3], counter >>> 0, Math.floor(counter / 2 ** 32) >>> 0, blockLen, flags];
  let m = words;
  for (let r = 0; r < 7; r++) {
    round(s, m);
    if (r < 6) m = PERM.map((i) => m[i]);
  }
  return Array.from({ length: 8 }, (_, i) => (s[i] ^ s[i + 8]) >>> 0);
}

function blockWords(bytes, start, len) {
  const w = new Array(16).fill(0);
  for (let i = 0; i < len; i++) w[i >> 2] |= bytes[start + i] << (8 * (i & 3));
  return w.map((x) => x >>> 0);
}

function chunkCv(bytes, start, end, counter, root) {
  let cv = IV;
  const blocks = Math.max(1, Math.ceil((end - start) / BLOCK_LEN));
  for (let b = 0; b < blocks; b++) {
    const s = start + b * BLOCK_LEN;
    const len = Math.min(BLOCK_LEN, end - s);
    let flags = 0;
    if (b === 0) flags |= CHUNK_START;
    if (b === blocks - 1) flags |= CHUNK_END | (root ? ROOT : 0);
    cv = compress(cv, blockWords(bytes, s, Math.max(0, len)), counter, Math.max(0, len), flags);
  }
  return cv;
}

/** Chaining value of chunks [first, first + count) of `bytes`. */
function subtree(bytes, first, count, root) {
  if (count === 1) return chunkCv(bytes, first * CHUNK_LEN, Math.min(bytes.length, (first + 1) * CHUNK_LEN), first, root);
  let left = 1;
  while (left * 2 < count) left *= 2;
  const l = subtree(bytes, first, left, false);
  const r = subtree(bytes, first + left, count - left, false);
  return compress(IV, [...l, ...r], 0, BLOCK_LEN, PARENT | (root ? ROOT : 0));
}

/** BLAKE3 of `bytes` (a Uint8Array or Buffer) as 64 lowercase hex digits. */
export function blake3(bytes) {
  const chunks = Math.max(1, Math.ceil(bytes.length / CHUNK_LEN));
  const words = subtree(bytes, 0, chunks, true);
  return words.map((w) => [0, 8, 16, 24].map((s) => ((w >>> s) & 0xff).toString(16).padStart(2, "0")).join("")).join("");
}
