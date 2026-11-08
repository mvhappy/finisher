import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { JOB_ERROR_CODES } from "./jobs.ts";

// `fin`'s exit classes, read from the Rust lists the way `exit_code` reads them.
const output = readFileSync(new URL("../../../crates/fin/src/output.rs", import.meta.url), "utf8");
function list(name: string): string[] {
  const m = output.match(new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error(`no ${name} in output.rs`);
  return [...m[1]!.replaceAll(/\/\/.*$/gm, "").matchAll(/"([A-Z0-9_]+)"/g)].map((x) => x[1]!);
}
const usage = list("USAGE_CODES");
const conflict = [...list("CONFLICT_CODES"), ...list("JOB_RESULT_CODES")];
const unavailable = list("UNAVAILABLE_CODES");
const validation = list("VALIDATION_CODES");
function exitCode(code: string): number {
  if (usage.includes(code)) return 2;
  if (conflict.includes(code)) return 4;
  if (unavailable.includes(code)) return 5;
  if (validation.includes(code)) return 3;
  return 6;
}

test("job_error_codes_match_fin_exit_classes", () => {
  for (const [code, exit] of Object.entries(JOB_ERROR_CODES)) expect(exitCode(code), code).toBe(exit);
});
