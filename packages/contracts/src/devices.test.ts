import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { DEVICE_ERROR_CODES, nameProblem } from "./devices.ts";

// `fin`'s exit classes, read from the Rust lists the way `exit_code` reads them.
const output = readFileSync(new URL("../../../crates/fin/src/output_codes.rs", import.meta.url), "utf8");
function list(name: string): string[] {
  const m = output.match(new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error(`no ${name} in output_codes.rs`);
  return [...m[1]!.replaceAll(/\/\/.*$/gm, "").matchAll(/"([A-Z0-9_]+)"/g)].map((x) => x[1]!);
}
function exitCode(code: string): number {
  if (list("DENIED_CODES").includes(code)) return 7;
  if (list("USAGE_CODES").includes(code)) return 2;
  if ([...list("CONFLICT_CODES"), ...list("JOB_RESULT_CODES")].includes(code)) return 4;
  if (list("UNAVAILABLE_CODES").includes(code)) return 5;
  if (list("VALIDATION_CODES").includes(code)) return 3;
  return 6;
}

test("codes_have_exit_classes (5B Decision 33: the Rust and TS tables agree)", () => {
  for (const [code, { exit }] of Object.entries(DEVICE_ERROR_CODES)) expect(exitCode(code), code).toBe(exit);
  expect(DEVICE_ERROR_CODES.DEVICE_LIMIT.exit).toBe(4);
  expect(DEVICE_ERROR_CODES.DEVICE_REVOKED.exit).toBe(7);
  expect(DEVICE_ERROR_CODES.ENV_MOVED.exit).toBe(7);
  expect(DEVICE_ERROR_CODES.PATH_NOT_ALLOWED.exit).toBe(7);
  // The HTTP statuses of Decision 33.
  const status = Object.fromEntries(Object.entries(DEVICE_ERROR_CODES).flatMap(([c, v]) => ("status" in v ? [[c, v.status]] : [])));
  expect(status).toMatchObject({
    PAIRING_EXPIRED: 410,
    PAIRING_USED: 409,
    PAIRING_REJECTED: 403,
    PAIRING_RATE_LIMITED: 429,
    TOKEN_RATE_LIMITED: 429,
    DPOP_INVALID: 401,
    DPOP_NONCE: 401,
    TOKEN_EXPIRED: 401,
    TICKET_INVALID: 401,
    ORIGIN_REFUSED: 403,
    HOST_REFUSED: 403,
    DEVICE_REVOKED: 401,
    ENV_MOVED: 403,
    DEVICE_LIMIT: 409,
  });
});

test("name_rules (NFC, length, control characters)", () => {
  expect(nameProblem("Studio tablet")).toBeUndefined();
  expect(nameProblem("Café")).toBeUndefined();
  expect(nameProblem("Café")).toBe("notNfc");
  expect(nameProblem("")).toBe("empty");
  expect(nameProblem("   ")).toBe("empty");
  expect(nameProblem("x".repeat(64))).toBeUndefined();
  expect(nameProblem("x".repeat(65))).toBe("tooLong");
  // UTF-16 units, as the audit's `Name` counts them.
  expect(nameProblem("📱".repeat(32))).toBeUndefined();
  expect(nameProblem("📱".repeat(33))).toBe("tooLong");
  // Format characters (a bidi override, a zero-width space) are refused too.
  expect(nameProblem("evil\u202Eenots")).toBe("control");
  expect(nameProblem("zero\u200Bwidth")).toBe("control");
  expect(nameProblem("tab\tlet")).toBe("control");
  expect(nameProblem("line\nbreak")).toBe("control");
});
