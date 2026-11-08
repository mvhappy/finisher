// The 3b/6b/7 review of 7A-2, against a real server: a revocation lost before it was written is
// done at the next start (finding 1), sessions cannot change state outside their branches
// (finding 10), and the persistent unsandboxed warning is shown (finding 13, owner decision F2).
import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { AgentServer, code, open, readCredential } from "./support/agent-server.ts";
import { addAsset, commit, ok, tx } from "./support/ndjson-client.ts";
import type { NdjsonClient } from "./support/ndjson-client.ts";

let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-review-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

async function project(local: NdjsonClient, name = "P"): Promise<string> {
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, name), name, rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
  return projectId;
}

describe("agent review fixes (server)", () => {
  test("a revocation lost before it was written is done at the next start", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const ended = await open(server.core, local, projectId);
    const gone = await open(server.core, local, projectId);
    const live = await open(server.core, local, projectId);
    const files = { ended: readCredential(ended.credentialPath), gone: readCredential(gone.credentialPath), live: readCredential(live.credentialPath) };
    // The ends are recorded, but their revocations never ran (a crash before the outbox).
    server.core.store.apply(ended.session.id, { kind: "ended", reason: "userEnded" });
    await server.core.store.delete(gone.session.id);
    for (const item of server.core.store.outbox()) server.core.store.outboxDone(item.id);
    const proves = async (k: keyof typeof files) => (await server.proveAs(files[k])).exit;
    expect((await proves("ended")).ok).toBe(true);
    expect((await proves("gone")).ok).toBe(true);
    await server.stop();
    await server.start();
    expect(code(await proves("ended"))).toMatch(/^CREDENTIAL_(REVOKED|EXPIRED)$|^UNAUTHENTICATED$/);
    expect(code(await proves("gone"))).toMatch(/^CREDENTIAL_(REVOKED|EXPIRED)$|^UNAUTHENTICATED$/);
    // A session that has not ended keeps its credential.
    expect((await proves("live")).ok).toBe(true);
  });

  test("sessions cannot acknowledge a salvage or change diagnostics settings", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    for (const mode of ["plan", "draft"] as const) {
      const a = await server.agent((await open(server.core, local, projectId, { mode })).credentialPath);
      const refused = async (tag: string, payload: unknown) => {
        const exit = await a.call(tag, payload);
        expect(code(exit), `${mode} ${tag}`).toBe("SCOPE_DENIED");
        // A plan session lacks `project.write` already; a draft one holds it and is still refused.
        if (mode === "draft" || tag.startsWith("diagnostics.")) {
          expect(exit.ok ? undefined : exit.error.details, `${mode} ${tag}`).toMatchObject({ reason: "agentSession" });
        }
      };
      await refused("project.acknowledgeSalvage", { projectId, copy: "x.db" });
      await refused("diagnostics.project", { projectId, collect: "never" });
      // Reads stay open.
      expect(code(await a.call("diagnostics.status", {})), mode).toBeUndefined();
    }
    // A person (`local`) is not affected.
    expect(code(await local.call("diagnostics.project", { projectId, collect: "never" }))).toBeUndefined();
  });

  test("server.status carries the unsandboxed agent tools warning", async () => {
    await server.start();
    const local = await server.local();
    const status = ok<{ warnings: Array<{ code: string; details: Record<string, unknown> }> }>(await local.call("server.status", {}));
    expect(status.warnings).toContainEqual(expect.objectContaining({ code: "SANDBOX_DISABLED", details: expect.objectContaining({ profile: "agent" }) }));
  });
});
