// The checks a batch's names and keys get before anything is written (plan 5D Decision 26,
// spec §6.5: worker output is untrusted). The schema check (`batch-file.ts`) knows only types;
// these know what the strings mean:
// - every raw component (a chain's and an entry's name) obeys `fin_library::RawRel::validate`:
//   non-empty, not `.` or `..`, no `/` or NUL byte (nor `\` on Windows);
// - `nameLossy` says whether the raw name is invalid UTF-8;
// - `nameNfc` is canonically equivalent to the raw name decoded as Rust's `from_utf8_lossy`
//   does (WHATWG UTF-8 with U+FFFD; both sides normalised here, so a Unicode-version gap
//   between the walker's and V8's tables cannot refuse a good name);
// - `relKey` is the parent directory's key, `/`, then `nameNfc` or a case fold of it (the
//   indexer does not know whether the root folds case, so either is taken; the parent part
//   must match exactly, so a key can never name another directory).
// A failure is `LIBRARY_BATCH_INVALID {reason: rawName | nameLossy | nameNfc | relKey, row}`.
import type { ScanEntry } from "@finisher/core-types";
import { CatalogFailure } from "../protocol.ts";

const fatal = new TextDecoder("utf-8", { fatal: true });
const lossy = new TextDecoder("utf-8");
const BACKSLASH = process.platform === "win32";

const invalid = (reason: string, row: number): CatalogFailure =>
  new CatalogFailure("LIBRARY_BATCH_INVALID", "the scan batch file is not valid", { reason, row });

/** `RawRel::validate` for one component's bytes. */
export function validComponent(raw: Uint8Array): boolean {
  if (raw.length === 0) return false;
  if (raw.length <= 2 && raw.every((b) => b === 0x2e)) return false;
  for (const b of raw) if (b === 0x2f || b === 0 || (BACKSLASH && b === 0x5c)) return false;
  return true;
}

/** Checks every component of a chain (base64). */
export function checkChain(components: ReadonlyArray<string>, row: number): void {
  for (const c of components) if (!validComponent(Buffer.from(c, "base64"))) throw invalid("rawName", row);
}

function utf8(raw: Uint8Array): boolean {
  try {
    fatal.decode(raw);
    return true;
  } catch {
    return false;
  }
}

/** Checks an entry's name, display name and key against its parent's key. */
export function checkEntry(e: ScanEntry, parentKey: string, row: number): void {
  checkChain(e.dir.components, row);
  const raw = Buffer.from(e.name, "base64");
  if (!validComponent(raw)) throw invalid("rawName", row);
  if (e.nameLossy === utf8(raw)) throw invalid("nameLossy", row);
  if (e.nameNfc.normalize("NFC") !== lossy.decode(raw).normalize("NFC")) throw invalid("nameNfc", row);
  const prefix = parentKey === "" ? "" : `${parentKey}/`;
  if (!e.relKey.startsWith(prefix)) throw invalid("relKey", row);
  const last = e.relKey.slice(prefix.length);
  const same = last === e.nameNfc || last.normalize("NFC") === e.nameNfc.normalize("NFC");
  const folded = last.toLowerCase().normalize("NFC") === e.nameNfc.toLowerCase().normalize("NFC");
  if (last.length === 0 || last.includes("/") || !(same || folded)) throw invalid("relKey", row);
}
