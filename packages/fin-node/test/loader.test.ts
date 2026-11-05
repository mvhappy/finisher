import { describe, expect, test } from "vitest";
import { linuxLibc, nativeFileNames } from "../loader.js";

const glibc = () => ({ header: { glibcVersionRuntime: "2.39" } });
const musl = () => ({ header: {} });

describe("native binary names", () => {
  test("glibc report: gnu first, musl as the fallback", () => {
    expect(nativeFileNames("linux", "x64", glibc)).toEqual([
      "fin-node.linux-x64.node",
      "fin-node.linux-x64-gnu.node",
      "fin-node.linux-x64-musl.node",
    ]);
  });

  test("no glibc in the report: musl first, gnu as the fallback", () => {
    expect(nativeFileNames("linux", "arm64", musl)).toEqual([
      "fin-node.linux-arm64.node",
      "fin-node.linux-arm64-musl.node",
      "fin-node.linux-arm64-gnu.node",
    ]);
  });

  test("an unreadable report tries both, gnu first", () => {
    const broken = () => {
      throw new Error("no report");
    };
    expect(linuxLibc(broken)).toBeUndefined();
    expect(nativeFileNames("linux", "x64", broken).slice(1)).toEqual(["fin-node.linux-x64-gnu.node", "fin-node.linux-x64-musl.node"]);
    expect(linuxLibc(() => undefined)).toBe("musl");
  });

  test("other platforms never consult the report", () => {
    const never = () => {
      throw new Error("must not be called");
    };
    expect(nativeFileNames("win32", "x64", never)).toEqual(["fin-node.win32-x64.node", "fin-node.win32-x64-msvc.node"]);
    expect(nativeFileNames("darwin", "arm64", never)).toEqual(["fin-node.darwin-arm64.node"]);
  });
});
