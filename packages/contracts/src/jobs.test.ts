import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { JOB_ERROR_CODES } from "./jobs.ts";

// `fin`'s exit classes, read from the Rust lists the way `exit_code` reads them.
const output = readFileSync(new URL("../../../crates/fin/src/output_codes.rs", import.meta.url), "utf8");
function body(name: string): string {
  const m = output.match(new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\];`));
  if (!m) throw new Error(`no ${name} in output_codes.rs`);
  return m[1]!;
}
function list(name: string): string[] {
  return [...body(name).replaceAll(/\/\/.*$/gm, "").matchAll(/"([A-Z0-9_]+)"/g)].map((x) => x[1]!);
}
/** The codes under a `// Jobs (5C-1 …)` comment in a list, up to the next comment. */
function jobsBlock(name: string): string[] {
  const out: string[] = [];
  let inJobs = false;
  let codes = false;
  for (const line of body(name).split("\n").map((l) => l.trim())) {
    if (line.startsWith("//")) {
      if (line.includes("Jobs (5C-1")) [inJobs, codes] = [true, false];
      else if (codes) inJobs = false;
      continue;
    }
    const code = line.match(/^"([A-Z0-9_]+)"/)?.[1];
    if (code === undefined) continue;
    codes = true;
    if (inJobs) out.push(code);
  }
  return out;
}
const LISTS = ["USAGE_CODES", "CONFLICT_CODES", "JOB_RESULT_CODES", "UNAVAILABLE_CODES", "VALIDATION_CODES"];
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

test("fin_job_codes_are_all_in_job_error_codes", () => {
  // The other direction: every job code `fin` classifies is in the contracts' list.
  const jobs = new Set([
    ...list("JOB_RESULT_CODES"),
    ...LISTS.flatMap(jobsBlock),
    ...LISTS.flatMap(list).filter((c) => /^(JOB|JOBHOST|WORKER)_/.test(c)),
  ]);
  expect(jobs.size).toBeGreaterThan(15);
  expect(jobs.has("WATCH_LAGGING")).toBe(true);
  expect(jobs.has("TOO_MANY_WATCHES")).toBe(true);
  for (const code of jobs) expect(Object.keys(JOB_ERROR_CODES), code).toContain(code);
});
