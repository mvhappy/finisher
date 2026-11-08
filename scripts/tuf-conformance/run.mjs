#!/usr/bin/env node
// Runs the upstream tuf-conformance suite (pinned in suite.json) against the real client, the way
// the CI job does, on a machine that has Python >= 3.10 (CI itself uses the suite's own action):
//   1. builds `tuf_conformance_client` (fin-registry, feature `test-http`; never a shipped build);
//   2. checks the suite out at the pinned commit under `archive/tuf-conformance/` (git-ignored)
//      and installs it into a virtual environment there;
//   3. runs it with `client.sh` as the entrypoint (`client.sh.xfails` lists the expected
//      failures), using `scripts/tuf-conformance/faketime` where libfaketime is not installed;
//   4. prints summary.mjs's table (a job summary on CI) and exits 1 on any unexpected result.
// Python is a dependency of this run only, never of the build, the tests or the app.
//
// Usage: node scripts/tuf-conformance/run.mjs [--no-build]   (PYTHON=/path/to/python3.x to choose one)
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { delimiter, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = fileURLToPath(new URL(".", import.meta.url));
const root = join(here, "..", "..");
const work = join(root, "archive", "tuf-conformance");
const pin = JSON.parse(readFileSync(join(here, "suite.json"), "utf8"));
const WIN = process.platform === "win32";

function run(cmd, args, opts = {}) {
  process.stdout.write(`$ ${cmd} ${args.join(" ")}\n`);
  const r = spawnSync(cmd, args, { stdio: "inherit", cwd: root, ...opts });
  if (r.status !== 0) {
    process.stderr.write(`${cmd} ${args[0] ?? ""} failed (${r.status ?? r.signal})\n`);
    process.exit(r.status || 1);
  }
}

/** The first interpreter that is Python 3.10 or newer. */
function python() {
  const candidates = [process.env.PYTHON, "python3.14", "python3.13", "python3.12", "python3.11", "python3.10", "python3", "python"].filter(Boolean);
  for (const c of candidates) {
    const r = spawnSync(c, ["-c", "import sys; print(sys.version_info >= (3, 10))"], { encoding: "utf8" });
    if (r.status === 0 && r.stdout.trim() === "True") return c;
  }
  process.stderr.write("no Python 3.10 or newer found: set PYTHON=/path/to/python3.x\n");
  process.exit(2);
}

if (!process.argv.includes("--no-build")) {
  run("cargo", ["build", "--release", "-p", "fin-registry", "--features", "test-http", "--example", "tuf_conformance_client"]);
}
mkdirSync(work, { recursive: true });
const suite = join(work, "suite");
if (!existsSync(join(suite, ".git"))) run("git", ["clone", "--quiet", pin.repo, suite]);
if (spawnSync("git", ["-C", suite, "cat-file", "-e", `${pin.sha}^{commit}`]).status !== 0) run("git", ["-C", suite, "fetch", "--quiet", "origin"]);
run("git", ["-C", suite, "checkout", "--quiet", "--detach", pin.sha]);

const venv = join(work, "venv");
const bin = join(venv, WIN ? "Scripts" : "bin");
if (!existsSync(bin)) run(python(), ["-m", "venv", venv]);
run(join(bin, "python"), ["-m", "pip", "install", "--quiet", "--disable-pip-version-check", "-e", suite]);

// libfaketime where it exists; otherwise the stand-in next to this file.
const haveFaketime = spawnSync(WIN ? "where" : "which", ["faketime"], { stdio: "ignore" }).status === 0;
const env = { ...process.env, ...(haveFaketime ? {} : { PATH: `${here}${delimiter}${process.env.PATH}` }) };
const report = join(work, "report.json");
const entry = join(here, "client.sh");
const tests = spawnSync(join(bin, "pytest"), ["-q", "-p", "no:cacheprovider", join(suite, "tuf_conformance"), "--entrypoint", entry, "--json-report", `--json-report-file=${report}`], { stdio: "inherit", cwd: root, env });
if (!existsSync(report)) {
  process.stderr.write("the suite wrote no report\n");
  process.exit(tests.status || 1);
}
const summary = spawnSync(process.execPath, [join(here, "summary.mjs"), report, `${entry}.xfails`], { encoding: "utf8", env: { ...process.env, TUF_CONFORMANCE_SHA: pin.sha } });
process.stdout.write(summary.stdout);
writeFileSync(join(work, "summary.md"), summary.stdout);
process.exit(summary.status ?? 1);
