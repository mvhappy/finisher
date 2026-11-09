// `pnpm sandbox:suite` (plan 7C Task 13): runs this OS's escape suites of `fin-sandbox` and
// prints a per-profile, per-OS table.
//
// - `escape_common` runs the case files of `crates/fin-sandbox/tests/cases/` for every profile
//   and writes one record per case and profile (`FIN_ESCAPE_REPORT`); the table counts them.
// - The OS's own suites (Linux: Landlock, the launcher, the namespace, the reaper; macOS:
//   Seatbelt; Windows: the container configuration) run after it, one `cargo test` each, and
//   get a row with libtest's counts.
// - Linux runs with `FIN_REQUIRE_LANDLOCK=1`: a host without Landlock fails instead of
//   skipping. `--require-namespaces` sets `FIN_REQUIRE_NAMESPACES=1` (a host with unprivileged
//   user namespaces for the launcher: the `sandbox-namespaces` CI job); without it, the
//   namespace cases skip where the host has none, and the reasons are listed.
//
// Exit 1 when a suite fails or a case is not observed as expected.

import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const SUITES = {
  linux: [
    "escape_common",
    "escape_agent_linux",
    "escape_worker_linux",
    "linux_landlock",
    "namespace_linux",
    "launcher",
    "reaper_linux",
    "spawner_linux",
  ],
  macos: ["escape_common", "escape_macos", "macos_seatbelt", "sbpl_macos", "spawner_macos"],
  windows: ["escape_common", "container_windows", "node_windows", "spawner_windows"],
};

export function osName(platform = process.platform) {
  if (platform === "linux") return "linux";
  if (platform === "darwin") return "macos";
  if (platform === "win32") return "windows";
  return null;
}

/// The environment the suites run with on `os`.
export function suiteEnv(os, { requireNamespaces = false, reportDir } = {}, base = process.env) {
  const env = { ...base, FIN_ESCAPE_REPORT: reportDir };
  if (os === "linux") env.FIN_REQUIRE_LANDLOCK = "1";
  if (requireNamespaces) env.FIN_REQUIRE_NAMESPACES = "1";
  return env;
}

/// libtest's summary line, or null.
export function parseResult(output) {
  const m = output.match(/test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored/);
  if (!m) return null;
  return { ok: m[1] === "ok", passed: +m[2], failed: +m[3], ignored: +m[4] };
}

/// The skip reasons the suites print (`<test>: skipped, …`, `… no socket namespace …`).
export function skipReasons(output) {
  const lines = output
    .split("\n")
    .map((l) => l.trim())
    .filter((l) => /: skipped, |no socket namespace on this host|: skipped$/.test(l));
  return [...new Set(lines)];
}

/// Per (OS, profile): how many cases were denied, allowed, skipped and failed.
export function tally(records) {
  const rows = new Map();
  for (const r of records) {
    const key = `${r.os}\u0000${r.profile}`;
    const row = rows.get(key) ?? { os: r.os, profile: r.profile, denied: 0, allowed: 0, skipped: 0, failed: 0 };
    if (!(r.result in row)) throw new Error(`unknown result ${r.result} (${r.case})`);
    row[r.result] += 1;
    rows.set(key, row);
  }
  const order = ["agent", "hook", "worker", "plugin"];
  return [...rows.values()].sort(
    (a, b) => a.os.localeCompare(b.os) || order.indexOf(a.profile) - order.indexOf(b.profile),
  );
}

export function table(header, rows) {
  const cells = [header, ...rows].map((r) => r.map(String));
  const width = header.map((_, i) => Math.max(...cells.map((r) => r[i].length)));
  const line = (r) => r.map((c, i) => c.padEnd(width[i])).join("  ").trimEnd();
  return [line(cells[0]), width.map((w) => "-".repeat(w)).join("  "), ...cells.slice(1).map(line)].join("\n");
}

export function format({ records, suites, skips }) {
  const out = [];
  out.push(
    table(
      ["os", "profile", "denied", "allowed", "skipped", "failed"],
      tally(records).map((r) => [r.os, r.profile, r.denied, r.allowed, r.skipped, r.failed]),
    ),
  );
  const failed = records.filter((r) => r.result === "failed");
  if (failed.length) {
    out.push("", "failed cases:", ...failed.map((r) => `  ${r.os} ${r.profile} ${r.file}: ${r.case}: ${r.detail}`));
  }
  out.push(
    "",
    table(
      ["suite", "passed", "failed", "ignored", "result"],
      suites.map((s) => [
        s.name,
        s.result?.passed ?? "-",
        s.result?.failed ?? "-",
        s.result?.ignored ?? "-",
        s.result?.ok && s.status === 0 ? "ok" : "FAILED",
      ]),
    ),
  );
  if (skips.length) out.push("", "skipped (reasons):", ...skips.map((s) => `  ${s}`));
  return out.join("\n");
}

export function readRecords(dir) {
  return readdirSync(dir)
    .filter((f) => f.endsWith(".json"))
    .sort()
    .flatMap((f) => JSON.parse(readFileSync(join(dir, f), "utf8")));
}

function main(argv) {
  const os = osName();
  if (!os) {
    console.error(`sandbox:suite: no sandbox on ${process.platform}`);
    return 1;
  }
  const requireNamespaces = argv.includes("--require-namespaces");
  const reportDir = mkdtempSync(join(tmpdir(), "fin-escape-"));
  try {
    const env = suiteEnv(os, { requireNamespaces, reportDir });
    const build = spawnSync("cargo", ["test", "-p", "fin-sandbox", "--tests", "--no-run"], {
      env,
      stdio: "inherit",
    });
    if (build.status !== 0) return 1;
    const suites = [];
    const skips = [];
    for (const name of SUITES[os]) {
      const r = spawnSync("cargo", ["test", "-q", "-p", "fin-sandbox", "--test", name, "--", "--nocapture"], {
        env,
        encoding: "utf8",
        maxBuffer: 64 << 20,
      });
      const output = `${r.stdout ?? ""}\n${r.stderr ?? ""}`;
      const result = parseResult(output);
      if (r.status !== 0 || !result?.ok) process.stdout.write(output);
      suites.push({ name, status: r.status, result });
      skips.push(...skipReasons(output).map((s) => `${name}: ${s}`));
    }
    const records = readRecords(reportDir);
    console.log(format({ records, suites, skips }));
    const bad = suites.some((s) => s.status !== 0 || !s.result?.ok) || records.some((r) => r.result === "failed");
    if (!records.length) {
      console.error("sandbox:suite: escape_common wrote no records");
      return 1;
    }
    return bad ? 1 : 0;
  } finally {
    rmSync(reportDir, { recursive: true, force: true });
  }
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
