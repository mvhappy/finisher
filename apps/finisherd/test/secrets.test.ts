// The secret store, the seal/open seam and ModelGateway (plan 7A Task 5, Decisions 22–24).
import { chmodSync, mkdtempSync, readdirSync, readFileSync, statSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { lockExclusive } from "@finisher/fin-node";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { manualClock, ulidAt } from "../src/agent/clock.ts";
import { identityBox, type Sealed, type SecretBox, sealedBytes } from "../src/secrets/box.ts";
import { type GrantAudit, ModelGateway } from "../src/secrets/gateway.ts";
import { FileSecretStore } from "../src/secrets/store.ts";

const KEY = "sk-or-v1-0123456789abcdef0123456789abcdef";
const OTHER = "sk-or-v1-fedcba9876543210fedcba9876543210";
const P1 = "01jb0000000000000000000001";
const P2 = "01jb0000000000000000000002";
const unix = process.platform !== "win32";

function dir(): string {
  return mkdtempSync(join(tmpdir(), "fin-secrets-"));
}

function ids() {
  let n = 0;
  return { ulid: () => ulidAt(1_794_571_200_000 + n++, Buffer.alloc(10, n)) };
}

async function code(p: Promise<unknown> | (() => unknown)): Promise<string> {
  try {
    await (typeof p === "function" ? p() : p);
  } catch (e) {
    return (e as { code?: string }).code ?? "no code";
  }
  return "none";
}

/** A box that XORs and counts its calls, so a test sees every crossing. */
function xorBox(): SecretBox & { opened: Buffer[]; seals: number } {
  const x = (b: Buffer) => Buffer.from(b.map((c) => c ^ 0x5a));
  const box = {
    opened: [] as Buffer[],
    seals: 0,
    seal(p: Buffer): Sealed {
      box.seals++;
      return { v: 1, data: x(p).toString("base64url") };
    },
    open(s: Sealed): Buffer {
      const out = x(sealedBytes(s));
      box.opened.push(out);
      return out;
    },
  };
  return box;
}

describe("FileSecretStore", () => {
  it("creates secrets.json 0600 in a 0700 directory, holding the sealed form only", async () => {
    const sd = join(dir(), "server-data");
    const store = new FileSecretStore(sd, { box: identityBox, clock: manualClock(), ids: ids() });
    const id = await store.put({ provider: "openrouter", label: "default", value: Buffer.from(KEY) });
    if (unix) {
      expect(statSync(sd).mode & 0o777).toBe(0o700);
      expect(statSync(store.path).mode & 0o777).toBe(0o600);
    }
    const text = readFileSync(store.path, "utf8");
    expect(text).not.toContain(KEY);
    expect(text).toContain(Buffer.from(KEY).toString("base64url"));
    expect(store.list()).toEqual([{ id, provider: "openrouter", label: "default", createdAt: 1_794_571_200_000 }]);
    expect(JSON.stringify(store.list())).not.toMatch(/sealed|value|data/);
    expect(store.reveal(id).toString()).toBe(KEY);
  });

  it("crosses plaintext only through the box", async () => {
    const box = xorBox();
    const store = new FileSecretStore(dir(), { box, clock: manualClock(), ids: ids() });
    const id = await store.put({ provider: "openrouter", label: "k", value: Buffer.from(KEY) });
    expect(box.seals).toBe(1);
    const text = readFileSync(store.path, "utf8");
    expect(text).not.toContain(KEY);
    expect(text).not.toContain(Buffer.from(KEY).toString("base64url"));
    store.list();
    expect(box.opened).toHaveLength(0);
    expect(store.reveal(id).toString()).toBe(KEY);
    expect(box.opened).toHaveLength(1);
  });

  it("refuses a provider other than openrouter, a bad label, project or key", async () => {
    const store = new FileSecretStore(dir());
    const v = Buffer.from(KEY);
    expect(await code(store.put({ provider: "anthropic", label: "x", value: v }))).toBe("PROVIDER_UNAVAILABLE");
    expect(await code(store.put({ provider: "openrouter", label: "", value: v }))).toBe("SECRET_INVALID");
    expect(await code(store.put({ provider: "openrouter", label: "a\nb", value: v }))).toBe("SECRET_INVALID");
    expect(await code(store.put({ provider: "openrouter", label: "x", projectId: "../p", value: v }))).toBe("SECRET_INVALID");
    expect(await code(store.put({ provider: "openrouter", label: "x", value: Buffer.from(`${KEY}\n`) }))).toBe("SECRET_INVALID");
    expect(await code(store.put({ provider: "openrouter", label: "x", value: Buffer.from("short") }))).toBe("SECRET_INVALID");
    expect(store.list()).toEqual([]);
  });

  it("a corrupted file or entry is SECRET_CORRUPT", async () => {
    const store = new FileSecretStore(dir(), { box: identityBox, clock: manualClock(), ids: ids() });
    const id = await store.put({ provider: "openrouter", label: "k", value: Buffer.from(KEY) });
    const doc = JSON.parse(readFileSync(store.path, "utf8"));
    doc.entries[0].sealed.data = "not base64url!";
    writeFileSync(store.path, JSON.stringify(doc));
    expect(await code(() => store.reveal(id))).toBe("SECRET_CORRUPT");
    doc.entries[0].sealed = { v: 2, data: "" };
    writeFileSync(store.path, JSON.stringify(doc));
    expect(await code(() => store.list())).toBe("SECRET_CORRUPT");
    writeFileSync(store.path, "{ not json");
    expect(await code(() => store.list())).toBe("SECRET_CORRUPT");
  });

  it("replaces the key bound to the same project, keeps the others, and removes by id", async () => {
    const store = new FileSecretStore(dir(), { box: identityBox, clock: manualClock(), ids: ids() });
    const server = await store.put({ provider: "openrouter", label: "server", value: Buffer.from(KEY) });
    const p1 = await store.put({ provider: "openrouter", label: "p1", projectId: P1, value: Buffer.from(KEY) });
    const p1b = await store.put({ provider: "openrouter", label: "p1 again", projectId: P1, value: Buffer.from(OTHER) });
    expect(store.list().map((m) => m.id)).toEqual([server, p1b]);
    expect(p1).not.toBe(p1b);
    await store.remove(server);
    expect(store.list().map((m) => m.label)).toEqual(["p1 again"]);
    expect(await code(store.remove(server))).toBe("SECRET_NOT_FOUND");
  });

  it("serialises concurrent puts and writes temp-then-rename", async () => {
    const d = dir();
    const store = new FileSecretStore(d, { box: identityBox, clock: manualClock(), ids: ids() });
    const projects = Array.from({ length: 20 }, (_, i) => `01jb00000000000000000000${String(i).padStart(2, "0")}`);
    const puts = projects.map((projectId) => store.put({ provider: "openrouter", label: projectId, projectId, value: Buffer.from(KEY) }));
    const done = await Promise.all(puts);
    expect(new Set(done).size).toBe(20);
    expect(store.list().map((m) => m.projectId).sort()).toEqual(projects);
    expect(readdirSync(d).sort()).toEqual(["secrets.json", "secrets.lock"]);
    // A failing write leaves the old file whole and no temp file behind.
    const before = readFileSync(store.path, "utf8");
    const box = { ...identityBox, seal: () => ({ v: 1, data: "x" }) as Sealed };
    const broken = new FileSecretStore(d, { box, clock: manualClock(), ids: ids() });
    const spy = vi.spyOn(JSON, "stringify").mockImplementationOnce(() => {
      throw new Error("disk full");
    });
    await expect(broken.put({ provider: "openrouter", label: "x", value: Buffer.from(KEY) })).rejects.toThrow("disk full");
    spy.mockRestore();
    expect(readFileSync(store.path, "utf8")).toBe(before);
    expect(readdirSync(d).sort()).toEqual(["secrets.json", "secrets.lock"]);
  });
});

describe("FileSecretStore: privacy and locking (review M2)", () => {
  it.skipIf(!unix)("repairs a loosened directory or file mode at every read and write", async () => {
    const d = dir();
    const store = new FileSecretStore(d, { clock: manualClock(), ids: ids() });
    await store.put({ provider: "openrouter", label: "k", value: Buffer.from(KEY) });
    chmodSync(store.path, 0o644);
    chmodSync(d, 0o755);
    store.list();
    expect(statSync(store.path).mode & 0o777).toBe(0o600);
    expect(statSync(d).mode & 0o777).toBe(0o700);
    chmodSync(store.path, 0o640);
    await store.put({ provider: "openrouter", label: "k2", value: Buffer.from(OTHER) });
    expect(statSync(store.path).mode & 0o777).toBe(0o600);
  });

  it.skipIf(!unix)("refuses a secrets.json that is a link", async () => {
    const d = dir();
    const elsewhere = join(dir(), "planted.json");
    writeFileSync(elsewhere, JSON.stringify({ v: 1, entries: [] }));
    symlinkSync(elsewhere, join(d, "secrets.json"));
    expect(await code(() => new FileSecretStore(d))).toBe("SECRET_STORE_UNSAFE");
    unlinkSync(join(d, "secrets.json"));
    const store = new FileSecretStore(d);
    symlinkSync(elsewhere, store.path);
    expect(await code(() => store.list())).toBe("SECRET_STORE_UNSAFE");
  });

  it("refuses to run on Windows until the keychain backend or an ACL helper exists", async () => {
    expect(await code(() => new FileSecretStore(dir(), { platform: "win32" }))).toBe("SECRET_STORE_UNAVAILABLE");
  });

  it("waits for another holder of the cross-process lock, then writes", async () => {
    const d = dir();
    const store = new FileSecretStore(d, { clock: manualClock(), ids: ids() });
    const held = lockExclusive(join(d, "secrets.lock"));
    let done = false;
    const put = store.put({ provider: "openrouter", label: "k", value: Buffer.from(KEY) }).then(() => {
      done = true;
    });
    await new Promise((r) => setTimeout(r, 150));
    expect(done).toBe(false);
    expect(store.list()).toEqual([]);
    held.release();
    await put;
    expect(store.list().map((m) => m.label)).toEqual(["k"]);
  });
});

describe("ModelGateway", () => {
  afterEach(() => vi.restoreAllMocks());

  async function setup(opts: { server?: boolean; project?: boolean } = { server: true }) {
    const box = xorBox();
    const store = new FileSecretStore(dir(), { box, clock: manualClock(), ids: ids() });
    if (opts.server) await store.put({ provider: "openrouter", label: "server", value: Buffer.from(KEY) });
    if (opts.project) await store.put({ provider: "openrouter", label: "p1", projectId: P1, value: Buffer.from(OTHER) });
    const running = new Set(["s1", "s2"]);
    const audit: GrantAudit[] = [];
    const gw = new ModelGateway(store, { isRunning: (s) => running.has(s) }, { record: (e) => audit.push(e) }, manualClock());
    return { box, store, running, audit, gw };
  }

  it("uses the project's key when one is bound, else the server's default", async () => {
    const { gw } = await setup({ server: true, project: true });
    expect(await gw.withKey(P1, "s1", async (k) => k)).toBe(OTHER);
    expect(await gw.withKey(P2, "s1", async (k) => k)).toBe(KEY);
    expect(gw.hasKey(P2)).toBe(true);
  });

  it("is PROVIDER_AUTH when no key exists, and audits the refusal", async () => {
    const { gw, audit } = await setup({});
    expect(gw.hasKey(P1)).toBe(false);
    expect(await code(gw.withKey(P1, "s1", async () => 1))).toBe("PROVIDER_AUTH");
    expect(audit).toEqual([expect.objectContaining({ outcome: "denied", reason: "noKey", sessionId: "s1", projectId: P1 })]);
  });

  it("refuses a session that is not running and audits it; audits the first grant once", async () => {
    const { gw, audit, running } = await setup();
    running.delete("s2");
    let called = false;
    expect(
      await code(
        gw.withKey(P1, "s2", async () => {
          called = true;
        }),
      ),
    ).toBe("PROVIDER_AUTH");
    expect(called).toBe(false);
    await gw.withKey(P1, "s1", async () => 1);
    await gw.withKey(P1, "s1", async () => 2);
    expect(audit.map((a) => [a.sessionId, a.outcome, a.scope ?? a.reason])).toEqual([
      ["s2", "denied", "sessionNotRunning"],
      ["s1", "granted", "server"],
    ]);
  });

  it("zero-fills the plaintext buffer once f settles, success or failure", async () => {
    const { gw, box } = await setup();
    let during = "";
    await gw.withKey(P1, "s1", async (k) => {
      during = box.opened[0]!.toString();
      return k.length;
    });
    expect(during).toBe(KEY);
    expect(box.opened[0]!.every((b) => b === 0)).toBe(true);
    await expect(gw.withKey(P1, "s1", async () => Promise.reject(new Error("boom")))).rejects.toThrow("boom");
    expect(box.opened[1]!.every((b) => b === 0)).toBe(true);
  });

  it("never lets the key into an error, a log or anything it returns", async () => {
    const { gw, audit } = await setup();
    const logged: string[] = [];
    for (const m of ["log", "info", "warn", "error", "debug"] as const) {
      vi.spyOn(console, m).mockImplementation((...a: unknown[]) => {
        logged.push(a.map(String).join(" "));
      });
    }
    const echoed = new Error(`401 Unauthorized: bad key ${KEY}`) as Error & { headers?: unknown };
    echoed.headers = { authorization: `Bearer ${KEY}` };
    const err = await gw.withKey(P1, "s1", async () => Promise.reject(echoed)).catch((e: unknown) => e);
    expect(String(err)).not.toContain(KEY);
    expect((err as Error).message).toContain("[redacted]");
    expect(JSON.stringify(err)).not.toContain(KEY);
    expect((err as Error).stack ?? "").not.toContain(KEY);
    // A key quoted only on the `cause` chain is caught too.
    const wrapped = new Error("request failed", { cause: new Error(`upstream said ${KEY}`) });
    const viaCause = await gw.withKey(P1, "s1", async () => Promise.reject(wrapped)).catch((e: unknown) => e);
    expect(viaCause).not.toBe(wrapped);
    expect((viaCause as Error).cause).toBeUndefined();
    expect(String((viaCause as Error).stack)).not.toContain(KEY);
    const refused = await gw.withKey(P1, "s9", async () => 1).catch((e: unknown) => e);
    expect(JSON.stringify(refused)).not.toContain(KEY);
    const ok = await gw.withKey(P1, "s1", async () => ({ status: 200 }));
    expect(JSON.stringify(ok)).not.toContain(KEY);
    expect(JSON.stringify(audit)).not.toContain(KEY);
    expect(logged.join("\n")).not.toContain(KEY);
  });
});
