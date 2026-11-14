import { describe, expect, test } from "vitest";
import { linuxLibc, loadNative, nativeFileNames } from "../loader.js";

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

describe("loadNative", () => {
  const base = { dir: "/bin/", platform: "linux", arch: "x64", getReport: glibc };

  test("no binary present: the error names the base, directory and file names", () => {
    const load = () => {
      throw new Error("must not load");
    };
    expect(() => loadNative({ ...base, exists: () => false, load })).toThrow(
      /no native binary for linux-x64 in \/bin\/ \(looked for fin-node\.linux-x64\.node, fin-node\.linux-x64-gnu\.node, fin-node\.linux-x64-musl\.node\)/,
    );
  });

  test("the best present binary loads", () => {
    const tried: string[] = [];
    const got = loadNative({
      ...base,
      exists: (p: string) => p.endsWith("-gnu.node") || p.endsWith("-musl.node"),
      load: (p: string) => (tried.push(p), { from: p }),
    });
    expect(got).toEqual({ from: "/bin/fin-node.linux-x64-gnu.node" });
    expect(tried).toEqual(["/bin/fin-node.linux-x64-gnu.node"]);
  });

  test("a present binary that does not load is skipped for the next", () => {
    const got = loadNative({
      ...base,
      exists: () => true,
      load: (p: string) => {
        if (!p.endsWith("-musl.node")) throw new Error(`cannot load ${p}`);
        return { from: p };
      },
    });
    expect(got).toEqual({ from: "/bin/fin-node.linux-x64-musl.node" });
  });

  test("when every present binary fails, the first failure is thrown", () => {
    expect(() =>
      loadNative({
        ...base,
        exists: (p: string) => !p.endsWith("fin-node.linux-x64.node"),
        load: (p: string) => {
          throw new Error(`cannot load ${p}`);
        },
      }),
    ).toThrow("cannot load /bin/fin-node.linux-x64-gnu.node");
  });
});
