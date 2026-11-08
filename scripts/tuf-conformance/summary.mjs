#!/usr/bin/env node
// Summarises a tuf-conformance run: the pytest-json-report file the suite writes, the expected
// failures in `client.sh.xfails` (strict: a test that starts to pass fails the run until it is
// taken out of the list) and a verdict. Markdown on stdout, for the job summary.
//
// Usage: node scripts/tuf-conformance/summary.mjs <report.json> [xfails-file]   (exit 1 on any
// failure, including an xfail that passed)
import { readFileSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

/** `[{reason, names[]}]` from an xfails file: comment lines head the names that follow. */
export function parseXfails(text) {
  const groups = [];
  let reason = [];
  let names = [];
  const flush = () => {
    if (names.length > 0) groups.push({ reason: reason.join(" "), names });
    reason = [];
    names = [];
  };
  for (const raw of text.split("\n")) {
    const line = raw.trimEnd();
    if (line === "") flush();
    else if (line.startsWith("#")) {
      if (names.length > 0) flush();
      reason.push(line.replace(/^#\s?/, ""));
    } else names.push(line);
  }
  flush();
  return groups;
}

/** Counts per test file, and the tests that decide the verdict. */
export function summarise(report, xfails = []) {
  const files = new Map();
  const bad = [];
  for (const t of report.tests ?? []) {
    const file = basename(t.nodeid.split("::")[0]);
    const row = files.get(file) ?? { passed: 0, failed: 0, xfailed: 0, xpassed: 0, skipped: 0 };
    row[t.outcome] = (row[t.outcome] ?? 0) + 1;
    files.set(file, row);
    if (t.outcome === "failed" || t.outcome === "xpassed" || t.outcome === "error") bad.push(`${t.nodeid.split("::").slice(1).join("::")} (${t.outcome})`);
  }
  const total = { passed: 0, failed: 0, xfailed: 0, xpassed: 0, skipped: 0 };
  for (const row of files.values()) for (const k of Object.keys(total)) total[k] += row[k] ?? 0;
  const expected = xfails.reduce((n, g) => n + g.names.length, 0);
  return { files, total, bad, expected };
}

export function render(report, xfails, commit = "") {
  const { files, total, bad, expected } = summarise(report, xfails);
  const out = [`### tuf-conformance${commit ? ` @ ${commit.slice(0, 12)}` : ""}`, "", "| Test file | passed | expected failures | skipped | failed |", "|---|--:|--:|--:|--:|"];
  for (const [file, r] of [...files].sort()) out.push(`| ${file} | ${r.passed} | ${r.xfailed} | ${r.skipped} | ${r.failed + r.xpassed + (r.error ?? 0)} |`);
  out.push(`| **all** | **${total.passed}** | **${total.xfailed}** | **${total.skipped}** | **${total.failed + total.xpassed}** |`, "");
  if (total.skipped > 0) out.push(`${total.skipped} test(s) were skipped by the suite.`, "");
  if (xfails.length > 0) {
    out.push(`Expected failures (${expected} listed, ${total.xfailed} seen), by design:`, "");
    for (const g of xfails) out.push(`- **${g.names.length}**: ${g.reason}`);
    out.push("");
  }
  out.push(bad.length === 0 ? "Verdict: every test passed or failed as listed." : `Verdict: **FAILED**: ${bad.join("; ")}`);
  return { text: `${out.join("\n")}\n`, ok: bad.length === 0 && total.xfailed === expected };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const [reportFile, xfailFile] = process.argv.slice(2);
  if (!reportFile) {
    process.stderr.write("usage: summary.mjs <report.json> [xfails-file]\n");
    process.exit(2);
  }
  const report = JSON.parse(readFileSync(reportFile, "utf8"));
  const xfails = xfailFile ? parseXfails(readFileSync(xfailFile, "utf8")) : [];
  const { text, ok } = render(report, xfails, process.env.TUF_CONFORMANCE_SHA);
  process.stdout.write(text);
  process.exitCode = ok ? 0 : 1;
}
