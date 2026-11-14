// A library file's `Expect` (`{size, mtimeNs, fileId}`) as fin-library's catalogue records it,
// for tests that hand a stage a file without scanning its library. The file id is the
// platform's form: the inode as 8 big-endian bytes on Unix (`source/unix.rs`), the 128-bit
// `FileIdInfo` on Windows, which NTFS fills with the 64-bit file index (Node's `ino`) as
// little-endian bytes and 8 zero bytes (`source/win.rs` `id64_bytes`).
import { statSync } from "node:fs";

export interface LibraryExpect {
  readonly size: number;
  readonly mtimeNs: string;
  readonly fileId: string;
}

export function libraryFileId(ino: bigint, platform: NodeJS.Platform = process.platform): string {
  const b = Buffer.alloc(platform === "win32" ? 16 : 8);
  if (platform === "win32") b.writeBigUInt64LE(ino, 0);
  else b.writeBigUInt64BE(ino, 0);
  return b.toString("base64");
}

export function libraryExpect(path: string): LibraryExpect {
  const s = statSync(path, { bigint: true });
  return { size: Number(s.size), mtimeNs: s.mtimeNs.toString(), fileId: libraryFileId(s.ino) };
}
