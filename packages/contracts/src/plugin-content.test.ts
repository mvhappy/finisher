// Review L3 of Tasks 10 and 12: `plugin.content.get` has a real success schema, and its result type
// is derived from it.
import { Schema } from "effect";
import { describe, expect, test } from "vitest";
import { PluginContentGetSuccess, type PluginContentResult } from "./plugin-content.ts";

const style = { name: "Default", color: "#ffffff" };
const ok: PluginContentResult = { id: "@acme/captions", version: "1.0.0", kind: "captionStyle", name: "punchy", title: "Punchy", style: style as PluginContentResult["style"] };

describe("PluginContentGetSuccess", () => {
  test("decodes_what_the_server_sends_and_refuses_what_it_does_not", () => {
    const decode = Schema.decodeUnknownSync(PluginContentGetSuccess);
    expect(decode(ok)).toEqual(ok);
    const { title: _title, ...untitled } = ok;
    expect(decode(untitled)).toEqual(untitled);
    for (const bad of [{ ...ok, kind: "fontCatalog" }, { ...ok, name: "Not A Name" }, { ...ok, style: "a string" }, { ...ok, id: undefined }, 7, null]) {
      expect(() => decode(bad)).toThrow();
    }
  });
});
