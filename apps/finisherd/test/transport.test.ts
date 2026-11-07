import { Cause } from "effect";
import { describe, expect, test } from "vitest";
import { isPeerClose } from "../src/transport.ts";

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
