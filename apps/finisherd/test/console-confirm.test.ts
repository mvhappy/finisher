// The console's confirmations (owner decision of 2026-11-08; `src/console/confirm.ts`): `y/N` or
// the name typed back, locally, and then the server's `console.challenge` code fetched and sent
// by the console itself (never shown), replaced once when the server turns it down as wrong or
// expired. A scripted connection stands in for the server.
import { describe, expect, test } from "vitest";
import type { ConsoleConnection } from "../src/console/client.ts";
import { ask, confirmed, proceed, typeName, withChallenge } from "../src/console/confirm.ts";
import type { Exit } from "./support/ndjson-client.ts";
import { person } from "./support/person.ts";

const refused = (reason: string): Exit => ({ ok: false, error: { code: "CONFIRMATION_REQUIRED", message: "no", details: { reason } } }) as Exit;
const okExit = (value: unknown): Exit => ({ ok: true, value }) as Exit;

/** A connection that issues codes `C1`, `C2`, … and answers the confirmed call with `replies` in order. */
function scripted(...replies: Exit[]): ConsoleConnection & { calls: Array<{ tag: string; payload: Record<string, unknown> }> } {
  const calls: Array<{ tag: string; payload: Record<string, unknown> }> = [];
  let issued = 0;
  return {
    calls,
    call: async (tag, payload) => {
      calls.push({ tag, payload: payload as Record<string, unknown> });
      if (tag === "console.challenge") return okExit({ code: `C${++issued}`, expiresAt: Date.now() + 60_000 });
      return replies.shift() ?? okExit({ done: true });
    },
    stream: () => {
      throw new Error("not used");
    },
    close: async () => undefined,
  };
}

describe("the challenge is the console's own", () => {
  test("fetched_and_sent_after_the_person_confirms", async () => {
    const c = scripted(okExit({ removed: 3 }));
    const p = person();
    expect(await confirmed(c, p, "This removes things.", proceed, "x.purge", { all: true })).toEqual({ removed: 3 });
    expect(p.asked).toEqual(["Proceed? [y/N] "]);
    expect(c.calls).toEqual([
      { tag: "console.challenge", payload: {} },
      { tag: "x.purge", payload: { all: true, challenge: "C1" } },
    ]);
    // Never shown.
    expect(p.shown.join("\n")).not.toContain("C1");
  });

  test("nothing_is_sent_when_the_person_says_no", async () => {
    for (const [how, answers] of [
      [proceed, ["n"]],
      [proceed, [""]],
      [typeName("word", "audit"), [""]],
      [typeName("word", "audit"), ["a", "b", "c"]],
    ] as const) {
      const c = scripted();
      await expect(confirmed(c, person(...answers), "What.", how, "x.purge", {})).rejects.toMatchObject({ error: { code: "CONFIRMATION_REQUIRED", details: { reason: "cancelled" } } });
      expect(c.calls).toEqual([]);
    }
  });

  test("wrong_or_expired_code_is_replaced_once", async () => {
    for (const reason of ["wrong", "expired"]) {
      const c = scripted(refused(reason), okExit({ ok: 1 }));
      expect(await withChallenge(c, "x.purge", {})).toEqual({ ok: 1 });
      expect(c.calls.filter((x) => x.tag === "x.purge").map((x) => x.payload.challenge)).toEqual(["C1", "C2"]);
    }
    // Twice: given up, with the server's error.
    const twice = scripted(refused("wrong"), refused("expired"));
    await expect(withChallenge(twice, "x.purge", {})).rejects.toMatchObject({ error: { code: "CONFIRMATION_REQUIRED", details: { reason: "expired" } } });
    expect(twice.calls.filter((x) => x.tag === "x.purge")).toHaveLength(2);
    // `missing` and other errors are not retried.
    for (const e of [refused("missing"), { ok: false, error: { code: "NOT_FOUND", message: "gone" } } as Exit]) {
      const once = scripted(e);
      await expect(withChallenge(once, "x.purge", {})).rejects.toBeDefined();
      expect(once.calls.filter((x) => x.tag === "x.purge")).toHaveLength(1);
    }
  });
});

describe("what the person is asked", () => {
  test("y_and_yes_in_any_case_proceed", async () => {
    for (const a of ["y", "Y", " yes ", "YES"]) await expect(ask(person(a), "What.", proceed)).resolves.toBeUndefined();
  });

  test("the_name_is_shown_plain_and_must_match_exactly", async () => {
    const how = typeName("library's name", "R\u0007aw\u202e");
    const p = person("raw", "Raw");
    await ask(p, "This removes it.", how);
    expect(p.shown).toEqual(["This removes it.", "To confirm, type the library's name: Raw", "That is not Raw. Type it exactly as shown, or press Enter to cancel."]);
    expect(p.asked).toEqual(["Type the library's name to confirm: ", "Type the library's name to confirm: "]);
  });
});
