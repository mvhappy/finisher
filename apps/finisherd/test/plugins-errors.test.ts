// A plugin's own error answer as the host accepts it (`plugin-errors.ts`, sp8b review of Task 18).
import { describe, expect, test } from "vitest";
import { mapPluginError } from "../src/plugins/host/family.ts";
import { PLUGIN_ANSWER_CODES, pluginAnswerError } from "../src/plugins/host/plugin-errors.ts";

describe("plugin answer errors", () => {
  test("job_cancelled_is_the_hosts_words", () => {
    expect(pluginAnswerError({ code: "JOB_CANCELLED", message: "acme stopped /home/x", details: { why: "acme" }, retry: "never" })).toEqual({ code: "JOB_CANCELLED", message: "the job was cancelled", details: {}, retry: "never" });
  });

  test("no_host_control_code_is_answerable", () => {
    for (const c of ["CONFLICT", "APPROVAL_PENDING", "CREDENTIAL_REVOKED", "PLUGIN_AUTHORITY_RETIRED", "PROJECT_NOT_FOUND", "BRANCH_NOT_FOUND", "LIBRARY_NOT_FOUND", "SCOPE_DENIED", "PLUGIN_TX_INVALID", "PLUGIN_CRASH_LOOP", "PLUGIN_DISABLED"]) {
      expect(PLUGIN_ANSWER_CODES.has(c)).toBe(false);
    }
  });

  test("kept_details_are_bounded_and_survive_the_family_mapping", () => {
    const deep = { a: { b: { c: { d: { e: "deep" } } } }, list: Array.from({ length: 100 }, (_, i) => i) };
    const kept = pluginAnswerError({ code: "PLUGIN_PROTOCOL", message: "m", details: deep });
    expect(kept.details).toEqual({ a: { b: { c: { d: null } } }, list: Array.from({ length: 64 }, (_, i) => i), source: "plugin" });
    // The family's mapping at the job's end keeps a kept code and its mark.
    expect(mapPluginError({ code: kept.code, message: kept.message, details: kept.details ?? {} })).toMatchObject({ code: "PLUGIN_PROTOCOL", details: { source: "plugin" } });
  });
});
