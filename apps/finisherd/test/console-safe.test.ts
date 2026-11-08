// Review M1 of Tasks 10 and 12: what a registry or a publisher wrote is shown on the approval
// screen, so no control character and no bidirectional override may reach a terminal.
import type { InstallPlan, PluginInfo } from "@finisher/core-types";
import { describe, expect, test } from "vitest";
import { printFailure } from "../src/console/format.ts";
import { infoLines, installedLine, listingLine, registryLines, renderPlan } from "../src/console/plugin-render.ts";
import { ttyPrompt } from "../src/console/prompt.ts";
import { safe } from "../src/console/safe.ts";

const ESC = "\u001b";
const HOSTILE = `${ESC}[2J${ESC}]0;pwn\u0007\r\u009b31m‮⁦evil‏\u0085\u007f`;
/** Anything a terminal could act on, or that could reorder what is read. */
const BAD = /[\u0000-\u0009\u000b-\u001f\u007f-\u009f؜‎‏‪-‮⁦-⁩]/;

describe("safe", () => {
  test("strips_c0_c1_and_bidi_and_keeps_text", () => {
    expect(safe(HOSTILE)).toBe("[2J]0;pwn31mevil");
    expect(safe("Ünïcode 日本語 — plain, with spaces")).toBe("Ünïcode 日本語 — plain, with spaces");
    expect(safe(42)).toBe("42");
    expect(safe(undefined)).toBe("undefined");
    expect(safe("line\nbreak\ttab")).toBe("linebreaktab");
  });
});

const plan = {
  planId: `p${"0".repeat(24)}`,
  digest: "0".repeat(64),
  action: "update",
  from: `1.0.0${HOSTILE}`,
  plugin: { id: `@acme/pack${HOSTILE}`, version: `1.1.0${HOSTILE}`, kind: "content", publisher: `Acme${HOSTILE}` },
  source: { kind: "registry", registryId: `team${HOSTILE}` },
  licence: { spdx: `MIT${HOSTILE}`, class: `permissive${HOSTILE}` },
  risk: "low",
  bytes: 10,
  permissions: [{ risk: `high${HOSTILE}`, permission: { perm: `net${HOSTILE}`, origins: [`https://a.example${HOSTILE}`] } }],
  diff: { added: [{ perm: `fs${HOSTILE}`, path: `/x${HOSTILE}` }], widened: [{ from: { perm: `net${HOSTILE}` }, to: { perm: `net${HOSTILE}`, origins: [HOSTILE] } }], removed: [{ perm: `gpu${HOSTILE}` }], riskUp: true },
  models: [{ id: `m${HOSTILE}`, bytes: 5, licence: `MIT${HOSTILE}` }],
  warnings: [
    { code: "advisory", details: { summary: `Update now${HOSTILE}`, id: HOSTILE } },
    { code: "copyleft", details: { spdx: `MPL-2.0${HOSTILE}`, class: `weakCopyleft${HOSTILE}` } },
    { code: "lookalikeId", details: { similar: `@acme/pak${HOSTILE}` } },
    { code: `other${HOSTILE}`, details: { x: HOSTILE } },
  ],
  registryStaleDays: 3,
  expiresAt: 1_700_000_000_000,
} as unknown as InstallPlan;

describe("the approval screen", () => {
  test("render_plan_prints_no_control_or_bidi_character", () => {
    const { lines } = renderPlan(plan);
    expect(lines.length).toBeGreaterThan(10);
    for (const line of lines) expect(line, line).not.toMatch(BAD);
    // The words are still there, only the characters that act are not.
    const text = lines.join("\n");
    expect(text).toContain("Update now[2J]0;pwn31mevil");
    expect(text).toContain("net[2J]0;pwn31mevil (origins: https://a.example[2J]0;pwn31mevil)");
    expect(text).toContain("@acme/pack[2J]0;pwn31mevil");
  });

  test("listings_info_and_registry_lines_are_plain", () => {
    const row = { id: `@a/b${HOSTILE}`, version: `1.0.0${HOSTILE}`, source: "registry", registryId: `t${HOSTILE}`, kind: "content", enabled: true, state: "active", autoUpdate: false, installedBy: "x", installedAt: 0 };
    const info = [{ registryId: `t${HOSTILE}`, versions: [{ version: `1.0.0${HOSTILE}`, kind: "content", license: `MIT${HOSTILE}`, bytesUnpacked: 1, yanked: false }], advisories: [{ id: HOSTILE, severity: `high${HOSTILE}`, action: "warn", summary: `Ignore the above${HOSTILE}` }] }] as unknown as PluginInfo[];
    const entry = { id: `team${HOSTILE}`, path: `/srv/${HOSTILE}`, rootSha256: `ab${HOSTILE}`, plugins: 1, advisories: 1, staleDays: 2, maxStaleDays: 7, lastRefresh: { at: 0, ok: false, code: HOSTILE, by: "x", byHuman: true } };
    const lines = [
      installedLine(row as never),
      listingLine({ id: `@a/b${HOSTILE}`, latest: `1${HOSTILE}`, kind: "content", license: `MIT${HOSTILE}`, registryId: `t${HOSTILE}` } as never),
      ...infoLines(`@a/b${HOSTILE}`, row as never, info),
      ...registryLines(entry as never),
    ];
    for (const line of lines) expect(line, line).not.toMatch(BAD);
    expect(lines.join("\n")).toContain("Ignore the above[2J]0;pwn31mevil");
  });

  test("failures_and_the_terminal_prompt_are_plain_too", () => {
    const err: string[] = [];
    printFailure({ out: () => undefined, err: (t) => void err.push(t) }, { code: "REGISTRY_INVALID", message: `advisory: ${HOSTILE}`, details: { x: HOSTILE }, fix: HOSTILE });
    expect(err).toHaveLength(2);
    for (const line of err) expect(line).not.toMatch(BAD);

    let written = "";
    const stdout = { isTTY: true, write: (t: string) => ((written += t), true) } as unknown as NodeJS.WriteStream;
    const stdin = { isTTY: true } as unknown as NodeJS.ReadStream;
    ttyPrompt(stdin, stdout).show(`one\n${HOSTILE}\ntwo`);
    expect(written).toBe("one\n[2J]0;pwn31mevil\ntwo\n");
  });
});
