import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parseXfails, render, summarise } from "./summary.mjs";

const here = (f) => fileURLToPath(new URL(f, import.meta.url));
const t = (nodeid, outcome) => ({ nodeid, outcome });

test("xfails_groups_follow_their_comments", () => {
  const groups = parseXfails("# one\n# two\na\nb[x y]\n\n# three\nc\n");
  assert.deepEqual(groups, [{ reason: "one two", names: ["a", "b[x y]"] }, { reason: "three", names: ["c"] }]);
});

test("the_checked_in_xfails_parse_and_have_a_reason_each", () => {
  const groups = parseXfails(readFileSync(here("client.sh.xfails"), "utf8"));
  assert.ok(groups.length >= 4);
  for (const g of groups) assert.ok(g.reason.length > 20 && g.names.length > 0, JSON.stringify(g));
});

test("a_run_is_ok_only_when_nothing_failed_and_every_listed_failure_failed", () => {
  const xfails = [{ reason: "by design", names: ["test_b"] }];
  const good = { tests: [t("x/test_a.py::test_a", "passed"), t("x/test_a.py::test_b", "xfailed")] };
  assert.equal(render(good, xfails).ok, true);
  assert.match(render(good, xfails).text, /\| test_a\.py \| 1 \| 1 \| 0 \| 0 \|/);
  const failed = { tests: [t("x/test_a.py::test_a", "failed"), t("x/test_a.py::test_b", "xfailed")] };
  assert.equal(render(failed, xfails).ok, false);
  assert.match(render(failed, xfails).text, /FAILED.*test_a \(failed\)/);
  // A listed failure that passes is news: the list is strict.
  const better = { tests: [t("x/test_a.py::test_b", "xpassed")] };
  assert.equal(summarise(better, xfails).bad.length, 1);
  assert.equal(render(better, xfails).ok, false);
});

test("the_pinned_suite_is_a_full_commit_sha", () => {
  const pin = JSON.parse(readFileSync(here("suite.json"), "utf8"));
  assert.match(pin.sha, /^[0-9a-f]{40}$/);
});

test("faketime_sets_the_clock_the_client_reads", () => {
  const run = (time) =>
    spawnSync(process.execPath, [here("faketime.mjs"), time, process.execPath, "-e", "process.stdout.write(process.env.FIN_TUF_FAKE_NOW_MS)"], { encoding: "utf8" });
  assert.equal(run("2026-11-03 12:00:00").stdout, String(Date.UTC(2026, 10, 3, 12)));
  assert.equal(run("2026-11-03 12:00:00+01:00").stdout, String(Date.UTC(2026, 10, 3, 11)));
  assert.equal(run("not a time").status, 2);
});
