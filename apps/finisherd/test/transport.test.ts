import { Cause, Effect, Exit } from "effect";
import { describe, expect, test } from "vitest";
import { claimSocketPath, isPeerClose } from "../src/transport.ts";

const socketError = (code: string) => ({ _tag: "SocketError", reason: { _tag: "SocketReadError", cause: { code } } });

describe("isPeerClose", () => {
  test("a peer that went away is a normal close (Unix and Windows codes)", () => {
    for (const code of ["ECONNRESET", "EPIPE", "ERROR_NO_DATA", "ERROR_BROKEN_PIPE"]) {
      expect(isPeerClose(Cause.fail(socketError(code))), code).toBe(true);
    }
    expect(isPeerClose(Cause.fail({ code: "EPIPE" }))).toBe(true);
    expect(isPeerClose(Cause.die(socketError("ECONNRESET")))).toBe(true);
    expect(isPeerClose(Cause.interrupt())).toBe(true);
  });

  test("anything else is still reported", () => {
    expect(isPeerClose(Cause.fail(socketError("EACCES")))).toBe(false);
    expect(isPeerClose(Cause.fail(new Error("boom")))).toBe(false);
    expect(isPeerClose(Cause.die(new Error("boom")))).toBe(false);
    expect(isPeerClose(Cause.fail("ECONNRESET"))).toBe(false);
    expect(isPeerClose(Cause.combine(Cause.fail(socketError("EPIPE")), Cause.die("x")))).toBe(false);
  });
});

describe("claimSocketPath", () => {
  // A socket path that does not fit `sun_path` made `listen` fail with a bare `EINVAL` (macOS)
  // after the rest of the start; it is refused first, by name.
  const at = (bytes: number) => `/${"d".repeat(bytes - "/run/fin.sock".length - 1)}/run/fin.sock`;

  test("a path longer than the OS's socket path is SOCKET_PATH_TOO_LONG, before anything else", async () => {
    for (const [platform, max] of [["darwin", 103], ["linux", 107], ["freebsd", 103]] as const) {
      const exit = await Effect.runPromiseExit(claimSocketPath(at(max + 1), platform));
      expect(Exit.isFailure(exit), platform).toBe(true);
      const error = Exit.isFailure(exit) ? Cause.squash(exit.cause) : undefined;
      expect(error, platform).toMatchObject({ code: "SOCKET_PATH_TOO_LONG", details: { path: at(max + 1), bytes: max + 1, max } });
      // At the limit it is claimed (nothing there to remove).
      expect(Exit.isSuccess(await Effect.runPromiseExit(claimSocketPath(at(max), platform))), platform).toBe(true);
    }
    // A pipe name has no such limit here.
    expect(Exit.isSuccess(await Effect.runPromiseExit(claimSocketPath(at(300), "win32")))).toBe(true);
  });
});
