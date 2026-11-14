// Project reach for the OpenRouter key calls (Task 12 review, L3). `agent.admin` is a person's
// scope, so in 7A only the console reaches these; the handlers check reach anyway, so the day
// another principal holds the scope (a project-limited one, a device) it cannot see, replace or
// remove a key of a project it does not reach, or touch the server's default key. A key out of
// reach answers as a missing one.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Principal } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AgentAdmin } from "../src/agent/admin.ts";
import { manualClock, systemIds } from "../src/agent/clock.ts";
import { identityBox } from "../src/secrets/box.ts";
import { FileSecretStore } from "../src/secrets/store.ts";

const [A, B] = ["01jb0000000000000000000001", "01jb0000000000000000000002"];
const KEY = "sk-or-v1-0123456789abcdef";
const person = (projects: Principal["projects"]): Principal => ({ kind: "console", id: "console", scopes: ["agent.admin"], projects, branches: "*", human: true }) as Principal;
const OWNER = person("*");
const ONLY_A = person([A]);
const ONLY_B = person([B]);

let dir: string;
let store: FileSecretStore;
let admin: AgentAdmin;
const audits: string[] = [];

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "finisherd-keys-reach-"));
  const clock = manualClock();
  store = new FileSecretStore(join(dir, "server-data"), { box: identityBox, clock, ids: systemIds(clock) });
  admin = new AgentAdmin({ serverData: join(dir, "server-data"), secrets: store, localPresets: {}, audit: (_by, d) => void audits.push(d.rpc) });
  audits.length = 0;
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

describe("key calls and project reach", () => {
  test("keyList shows a caller only the keys of projects in its reach, the server's default only to `*`", async () => {
    const def = await admin.keySet(OWNER, { provider: "openrouter", key: KEY });
    const ka = await admin.keySet(OWNER, { provider: "openrouter", key: KEY, projectId: A });
    const kb = await admin.keySet(OWNER, { provider: "openrouter", key: KEY, projectId: B });
    expect(admin.keyList(OWNER).map((k) => k.id).sort()).toEqual([def.id, ka.id, kb.id].sort());
    expect(admin.keyList(ONLY_A).map((k) => k.id)).toEqual([ka.id]);
    expect(admin.keyList(ONLY_B).map((k) => k.id)).toEqual([kb.id]);
    expect(admin.keyList(person([]))).toEqual([]);
  });

  test("keyRemove of a key out of reach (or the server's default) removes nothing and says what a missing key says", async () => {
    const def = await admin.keySet(OWNER, { provider: "openrouter", key: KEY });
    const ka = await admin.keySet(OWNER, { provider: "openrouter", key: KEY, projectId: A });
    audits.length = 0;
    expect(await admin.keyRemove(ONLY_B, ka.id)).toBe(false);
    expect(await admin.keyRemove(ONLY_A, def.id)).toBe(false);
    expect(await admin.keyRemove(ONLY_A, "no-such-key")).toBe(false);
    expect(audits).toEqual([]);
    expect(admin.keyList(OWNER)).toHaveLength(2);
    expect(await admin.keyRemove(ONLY_A, ka.id)).toBe(true);
    expect(await admin.keyRemove(OWNER, def.id)).toBe(true);
    expect(admin.keyList(OWNER)).toEqual([]);
    expect(audits).toEqual(["agent.key.remove", "agent.key.remove"]);
  });

  test("setting the server's default key needs every project; a project's own key needs only that project", async () => {
    await expect(admin.keySet(ONLY_A, { provider: "openrouter", key: KEY })).rejects.toMatchObject({ code: "SCOPE_DENIED", details: { reason: "serverDefaultKey" } });
    expect(admin.keyList(OWNER)).toEqual([]);
    expect((await admin.keySet(ONLY_A, { provider: "openrouter", key: KEY, projectId: A })).projectId).toBe(A);
    expect((await admin.keySet(OWNER, { provider: "openrouter", key: KEY })).projectId).toBeUndefined();
  });
});
