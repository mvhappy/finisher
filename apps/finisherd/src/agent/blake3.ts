// BLAKE3 in hash mode, 32-byte output: the `bodyHash` of a model request (plan 7A Decision 7).
// Node has no BLAKE3 and no new dependency is taken for it; this follows the specification's
// reference implementation (one chunk state, a stack of chaining values) and is checked against
// the official test vectors (`agent-blake3.test.ts`). Not for secrets: no constant-time claims.

const IV = new Uint32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
const PERM = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const BLOCK = 64;
const CHUNK = 1024;

const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));

function g(s: Uint32Array, a: number, b: number, c: number, d: number, x: number, y: number): void {
  s[a] = (s[a]! + s[b]! + x) | 0;
  s[d] = rotr(s[d]! ^ s[a]!, 16);
  s[c] = (s[c]! + s[d]!) | 0;
  s[b] = rotr(s[b]! ^ s[c]!, 12);
  s[a] = (s[a]! + s[b]! + y) | 0;
  s[d] = rotr(s[d]! ^ s[a]!, 8);
  s[c] = (s[c]! + s[d]!) | 0;
  s[b] = rotr(s[b]! ^ s[c]!, 7);
}

/** The full 16-word compression output. */
function compress(cv: Uint32Array, block: Uint32Array, counter: number, len: number, flags: number): Uint32Array {
  const s = new Uint32Array(16);
  s.set(cv, 0);
  s.set(IV.subarray(0, 4), 8);
  s[12] = counter >>> 0;
  s[13] = Math.floor(counter / 0x1_0000_0000) >>> 0;
  s[14] = len;
  s[15] = flags;
  let m = Uint32Array.from(block);
  for (let r = 0; r < 7; r++) {
    g(s, 0, 4, 8, 12, m[0]!, m[1]!);
    g(s, 1, 5, 9, 13, m[2]!, m[3]!);
    g(s, 2, 6, 10, 14, m[4]!, m[5]!);
    g(s, 3, 7, 11, 15, m[6]!, m[7]!);
    g(s, 0, 5, 10, 15, m[8]!, m[9]!);
    g(s, 1, 6, 11, 12, m[10]!, m[11]!);
    g(s, 2, 7, 8, 13, m[12]!, m[13]!);
    g(s, 3, 4, 9, 14, m[14]!, m[15]!);
    const next = new Uint32Array(16);
    for (let i = 0; i < 16; i++) next[i] = m[PERM[i]!]!;
    m = next;
  }
  for (let i = 0; i < 8; i++) {
    s[i] = s[i]! ^ s[i + 8]!;
    s[i + 8] = s[i + 8]! ^ cv[i]!;
  }
  return s;
}

function words(bytes: Uint8Array): Uint32Array {
  const w = new Uint32Array(16);
  for (let i = 0; i < 16; i++) {
    w[i] = bytes[4 * i]! | (bytes[4 * i + 1]! << 8) | (bytes[4 * i + 2]! << 16) | (bytes[4 * i + 3]! << 24);
  }
  return w;
}

interface Output {
  cv: Uint32Array;
  block: Uint32Array;
  counter: number;
  len: number;
  flags: number;
}

const chainingValue = (o: Output) => compress(o.cv, o.block, o.counter, o.len, o.flags).slice(0, 8);

function parent(left: Uint32Array, right: Uint32Array): Output {
  const block = new Uint32Array(16);
  block.set(left, 0);
  block.set(right, 8);
  return { cv: IV, block, counter: 0, len: BLOCK, flags: PARENT };
}

/** The 32-byte BLAKE3 hash of `input`. */
export function blake3(input: Uint8Array): Buffer {
  const stack: Uint32Array[] = [];
  let cv = IV;
  let counter = 0;
  let compressed = 0;
  let block = new Uint8Array(BLOCK);
  let blockLen = 0;
  let pos = 0;
  const startFlag = () => (compressed === 0 ? CHUNK_START : 0);
  while (pos < input.length) {
    if (compressed * BLOCK + blockLen === CHUNK) {
      // A full chunk: its chaining value joins the tree; merge completed subtrees.
      let chunkCv = chainingValue({ cv, block: words(block), counter, len: blockLen, flags: startFlag() | CHUNK_END });
      let total = counter + 1;
      while (total % 2 === 0) {
        chunkCv = chainingValue(parent(stack.pop()!, chunkCv));
        total /= 2;
      }
      stack.push(chunkCv);
      counter += 1;
      cv = IV;
      compressed = 0;
      block = new Uint8Array(BLOCK);
      blockLen = 0;
    }
    if (blockLen === BLOCK) {
      cv = compress(cv, words(block), counter, BLOCK, startFlag()).slice(0, 8);
      compressed += 1;
      block = new Uint8Array(BLOCK);
      blockLen = 0;
    }
    const take = Math.min(BLOCK - blockLen, input.length - pos);
    block.set(input.subarray(pos, pos + take), blockLen);
    blockLen += take;
    pos += take;
  }
  let out: Output = { cv, block: words(block), counter, len: blockLen, flags: startFlag() | CHUNK_END };
  for (let i = stack.length - 1; i >= 0; i--) out = parent(stack[i]!, chainingValue(out));
  const root = compress(out.cv, out.block, 0, out.len, out.flags | ROOT);
  const bytes = Buffer.alloc(32);
  for (let i = 0; i < 8; i++) bytes.writeUInt32LE(root[i]!, 4 * i);
  return bytes;
}

/** `blake3:<64 hex>`, as `fin`'s `argsHash` writes an unkeyed BLAKE3. */
export function blake3Tagged(input: Uint8Array): string {
  return `blake3:${blake3(input).toString("hex")}`;
}
