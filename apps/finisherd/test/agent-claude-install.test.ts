// The CLI's install tree a session may read (plan 7D Decision 7, Task 14 review): only the native
// layout (`…/claude/versions/<v>`, Task 0) grants its directory; any other binary is granted as
// the one file it is, never its directory (a home, `/`, a temp root, a Downloads folder).
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { installTreeRead } from "../src/agent/claude/install.ts";

const WIN = process.platform === "win32";
const same = (p: string) => p;

let tmp: string;
let home: string;
beforeEach(() => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "claude-install-")));
  home = join(tmp, "home");
  mkdirSync(home, { recursive: true });
});
afterEach(() => rmSync(tmp, { recursive: true, force: true }));

const file = (path: string) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, "ELF");
  return path;
};

describe.skipIf(WIN)("installTreeRead on a Unix install", () => {
  test("a binary directly in the home is granted alone, never the home", () => {
    const bin = file(join(home, "claude"));
    expect(installTreeRead(bin, "linux", { home })).toEqual([bin]);
  });

  test("a binary in a broad directory is granted alone", () => {
    const h = "/home/someone";
    for (const bin of ["/claude", "/home/claude", "/tmp/claude", "/var/tmp/claude", `${h}/claude`, `${h}/Downloads/claude`, "/usr/bin/claude", "/usr/local/bin/claude", "/opt/claude"]) {
      expect(installTreeRead(bin, "linux", { home: h, realpath: same }), bin).toEqual([bin]);
    }
  });

  test("the native layout through its link grants the versions directory", () => {
    const target = file(join(home, ".local", "share", "claude", "versions", "2.1.321"));
    const link = join(home, ".local", "bin", "claude");
    mkdirSync(dirname(link), { recursive: true });
    symlinkSync(target, link);
    expect(installTreeRead(link, "linux", { home })).toEqual([dirname(target)]);
    // A prerelease's file is a version too; a file that is not a version is granted alone.
    expect(installTreeRead("/opt/claude/versions/2.1.322-beta.1", "linux", { home, realpath: same })).toEqual(["/opt/claude/versions"]);
    expect(installTreeRead("/opt/claude/versions/run", "linux", { home, realpath: same })).toEqual(["/opt/claude/versions/run"]);
    expect(installTreeRead("/opt/other/versions/2.1.321", "linux", { home, realpath: same })).toEqual(["/opt/other/versions/2.1.321"]);
  });

  test("a link pointing into the home grants the file it points at, not the home or the link's directory", () => {
    const real = file(join(home, "claude"));
    const link = join(tmp, "bin", "claude");
    mkdirSync(dirname(link), { recursive: true });
    symlinkSync(real, link);
    expect(installTreeRead(link, "linux", { home })).toEqual([real]);
  });

  test("the layout's directory is refused when it holds the home, a temp root or is a root", () => {
    expect(installTreeRead("/u/claude/versions/2.1.321", "linux", { home: "/u/claude/versions", realpath: same })).toEqual(["/u/claude/versions/2.1.321"]);
    expect(installTreeRead("/u/claude/versions/2.1.321", "linux", { home: "/u/claude/versions/me", realpath: same })).toEqual(["/u/claude/versions/2.1.321"]);
    expect(installTreeRead("/s/claude/versions/2.1.321", "linux", { home: "/h", tmp: ["/s/claude/versions/tmp"], realpath: same })).toEqual(["/s/claude/versions/2.1.321"]);
    // The default temp roots are the OS's.
    expect(installTreeRead("/tmp/claude", "linux", { home: "/h", realpath: same })).toEqual(["/tmp/claude"]);
  });

  test("a guarded root that is a link is compared resolved (review: TMPDIR alias)", () => {
    const links: Record<string, string> = { "/alias": "/opt/claude/versions", "/alias/2.1.321": "/opt/claude/versions/2.1.321" };
    const realpath = (q: string) => links[q] ?? q;
    expect(installTreeRead("/alias/2.1.321", "linux", { home: "/h", tmp: ["/alias"], realpath })).toEqual(["/opt/claude/versions/2.1.321"]);
    expect(installTreeRead("/alias/2.1.321", "linux", { home: "/alias", tmp: [], realpath })).toEqual(["/opt/claude/versions/2.1.321"]);
  });

  test("an unresolvable native-layout path is granted as the file, never its directory", () => {
    const gone = () => {
      throw new Error("ENOENT");
    };
    expect(installTreeRead("/opt/claude/versions/2.1.321", "linux", { home: "/h", tmp: [], realpath: gone })).toEqual(["/opt/claude/versions/2.1.321"]);
    expect(installTreeRead("claude/versions/2.1.321", "linux", { home: "/h", tmp: [], realpath: gone })).toEqual(["claude/versions/2.1.321"]);
  });

  test("macOS compares the guard without case", () => {
    expect(installTreeRead("/Users/Me/claude/versions/2.1.321", "darwin", { home: "/users/me/claude/versions", tmp: [], realpath: same })).toEqual(["/Users/Me/claude/versions/2.1.321"]);
  });

  test("a path that cannot be resolved is taken as given", () => {
    expect(installTreeRead(join(tmp, "missing", "claude"), "linux", { home })).toEqual([join(tmp, "missing", "claude")]);
  });
});

describe("installTreeRead on Windows paths", () => {
  test("the layout grants its directory, case-insensitively; a file in the profile alone", () => {
    const o = { home: "C:\\Users\\me", tmp: ["C:\\Users\\me\\AppData\\Local\\Temp"], realpath: same };
    expect(installTreeRead("C:\\Users\\me\\.local\\share\\claude\\versions\\2.1.321.exe", "win32", o)).toEqual(["C:\\Users\\me\\.local\\share\\claude\\versions"]);
    expect(installTreeRead("C:\\Users\\me\\.local\\share\\Claude\\Versions\\2.1.321.exe", "win32", o)).toEqual(["C:\\Users\\me\\.local\\share\\Claude\\Versions"]);
    expect(installTreeRead("C:\\Users\\me\\claude.exe", "win32", o)).toEqual(["C:\\Users\\me\\claude.exe"]);
    expect(installTreeRead("C:\\claude.exe", "win32", o)).toEqual(["C:\\claude.exe"]);
    expect(installTreeRead("c:\\users\\ME\\claude\\versions\\2.1.321.exe", "win32", { ...o, home: "C:\\Users\\me\\claude\\versions" })).toEqual(["c:\\users\\ME\\claude\\versions\\2.1.321.exe"]);
  });
});
