// 7A Task 7: an agent session's root branch and credential (Decisions 15, 18; spec §5.1, §5.2,
// §6.1): opened as the starter, minted with the session's id and its mode's scopes, written
// 0600, revoked at the end and refreshed on resume, both through the outbox, at least once.
import { mkdirSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { HUMAN_ONLY, type Principal, type Session } from "@finisher/contracts";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { MODE_SCOPES, NEVER_SESSION_SCOPES, nextBranchName, SESSION_IDLE_S } from "../src/agent/credentials.ts";
import { actorFor } from "../src/diagnostics.ts";
import { AgentServer, code, open, readCredential } from "./support/agent-server.ts";
import { addAsset, commit, connectProved, ok, tx } from "./support/ndjson-client.ts";
import type { NdjsonClient } from "./support/ndjson-client.ts";

const DAY = 86_400_000;
let tmp: string;
let server: AgentServer;

beforeEach(() => {
  tmp = mkdtempSync(join(tmpdir(), "finisherd-agent-creds-"));
  const serverData = join(tmp, "server-data");
  mkdirSync(serverData, { recursive: true, mode: 0o700 });
  server = new AgentServer(serverData);
});

afterEach(async () => {
  await server.stop();
  rmSync(tmp, { recursive: true, force: true });
});

describe("MODE_SCOPES (units)", () => {
  test("no_human_scope_in_any_mode, and none of the scopes no mode may hold", () => {
    for (const [mode, scopes] of Object.entries(MODE_SCOPES)) {
      for (const s of scopes) {
        expect(HUMAN_ONLY.has(s), `${mode}: ${s}`).toBe(false);
        expect(NEVER_SESSION_SCOPES.has(s), `${mode}: ${s}`).toBe(false);
      }
      expect(scopes, mode).not.toContain("project.create");
    }
    expect(MODE_SCOPES.plan).not.toContain("project.write");
    // 7B Task 6: a draft or auto session starts and drives its own children; Task 11: it asks for reviews.
    expect(MODE_SCOPES.draft).toEqual([...MODE_SCOPES.plan, "project.write", "agent.run", "review.write"]);
    expect(MODE_SCOPES.auto).toEqual(MODE_SCOPES.draft);
    expect(MODE_SCOPES.plan).not.toContain("agent.run");
  });

  test("nextBranchName counts the role's own", () => {
    expect(nextBranchName([], "editor")).toBe("agent.editor-1");
    expect(nextBranchName([{ name: "agent.editor-1" }, { name: "agent.editor-7" }, { name: "agent.critic-9" }, { name: "agent.editor-x" }], "editor")).toBe(
      "agent.editor-8",
    );
  });
});

async function project(local: NdjsonClient, name = "P"): Promise<string> {
  const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, name), name, rate: "30" })).projectId;
  ok(await commit(local, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa0", 0, [addAsset()])));
  return projectId;
}

type Branch = { id: string; name: string; parent?: string | null; forkSeq: number; createdBy: string };
const branchesOf = async (c: NdjsonClient, projectId: string) =>
  ok<{ branches: Branch[] }>(await c.call("branch.list", { projectId, all: true })).branches;

describe("sessions on a server", () => {
  test("start_creates_root_branch_and_credential", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const { session, credentialPath } = await open(server.core, local, projectId);
    expect(session).toMatchObject({ status: "idle", mode: "draft", credentialId: session.id, base: { branch: "main", seq: 1 } });
    const root = (await branchesOf(local, projectId)).find((b) => b.id === session.rootBranch);
    expect(root).toMatchObject({ name: "agent.editor-1", parent: "main", forkSeq: 1, createdBy: `agent:editor:${session.id}` });
    // The 5A credential file, 0600, named for the session.
    expect(credentialPath).toBe(join(server.core.store.root, session.id, "credential.json"));
    if (process.platform !== "win32") expect(statSync(credentialPath).mode & 0o777).toBe(0o600);
    const file = readCredential(credentialPath);
    expect(file).toEqual({ v: 1, kind: "agent", id: session.id, key: file.key, env: server.info.envId });
    const a = await server.agent(credentialPath);
    const who = ok<{ principal: Principal; defaultBranch?: string }>(await a.call("auth.whoami", {}));
    expect(who.principal).toEqual({ kind: "agent", id: session.id, scopes: [...MODE_SCOPES.draft], projects: [projectId], branches: [session.rootBranch], human: false });
    // A second session gets the next name.
    const second = await open(server.core, local, projectId);
    expect((await branchesOf(local, projectId)).find((b) => b.id === second.session.rootBranch)?.name).toBe("agent.editor-2");
  });

  test("principal_id_is_session_id: whoami, the default branch and the journal actor", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const { session, credentialPath } = await open(server.core, local, projectId);
    const a = await server.agent(credentialPath);
    const who = ok<{ principal: Principal; defaultBranch?: string }>(await a.call("auth.whoami", {}));
    expect([who.principal.id, who.defaultBranch]).toEqual([session.id, session.rootBranch]);
    expect(actorFor(who.principal, [["fin-actor-session", "forged"]]).actor).toEqual({ kind: "agent", session: session.id });
    // Only a session's credential has a default branch.
    expect(ok<{ defaultBranch?: string }>(await local.call("auth.whoami", {})).defaultBranch).toBeUndefined();
  });

  test("plan_mode_has_no_branch_and_no_write_scope", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const before = (await branchesOf(local, projectId)).length;
    const { session, credentialPath } = await open(server.core, local, projectId, { mode: "plan" });
    expect(session.rootBranch).toBeUndefined();
    expect((await branchesOf(local, projectId)).length).toBe(before);
    const a = await server.agent(credentialPath);
    const who = ok<{ principal: Principal; defaultBranch?: string }>(await a.call("auth.whoami", {}));
    expect(who.principal.scopes).toEqual([...MODE_SCOPES.plan]);
    expect(who.defaultBranch).toBe("main");
    expect(code(await commit(a, projectId, tx("01aaaaaaaaaaaaaaaaaaaaaaa1", 1, [addAsset("$b")])))).toBe("SCOPE_DENIED");
    // It reads.
    expect(code(await a.call("snapshot.get", { projectId, branch: "main" }))).toBeUndefined();
  });

  test("no_project_open_or_create_in_any_mode", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    for (const mode of ["plan", "draft", "auto"] as const) {
      const { credentialPath } = await open(server.core, local, projectId, { mode });
      const a = await server.agent(credentialPath);
      expect(code(await a.call("project.open", { dir: join(tmp, "P") })), mode).toBe("SCOPE_DENIED");
      expect(code(await a.call("project.create", { dir: join(tmp, `Q-${mode}`), name: "Q", rate: "30" })), mode).toBe("SCOPE_DENIED");
    }
  });

  test("branch_in_use: a second live session on the same root", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const { session } = await open(server.core, local, projectId);
    await expect(open(server.core, local, projectId, { root: session.rootBranch! })).rejects.toMatchObject({ code: "BRANCH_IN_USE" });
    server.core.store.apply(session.id, { kind: "ended", reason: "done" });
    const again = await open(server.core, local, projectId, { root: session.rootBranch! });
    expect(again.session.rootBranch).toBe(session.rootBranch);
  });

  test("end_revokes_credential: through the outbox, once", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const { session, credentialPath } = await open(server.core, local, projectId);
    const a = await server.agent(credentialPath);
    server.core.store.apply(session.id, { kind: "ended", reason: "done" });
    expect(server.core.credentials.drain()).toBe(1);
    expect(server.core.credentials.drain()).toBe(0);
    // Its live connection was ended by the revoke path.
    expect(code(await a.call("auth.whoami", {}))).not.toBeUndefined();
    const again = await server.proveAs(readCredential(credentialPath));
    expect([again.proved, code(again.exit)]).toEqual([true, "CREDENTIAL_REVOKED"]);
    // Handling the same item again (a crash before `outboxDone`) changes nothing.
    expect(server.core.credentials.handle({ id: 0, sessionId: session.id, effect: { kind: "revokeCredential" }, at: 0 })).toBe(true);
    const console = await connectProved({ info: server.info, serverData: server.serverData }, "console");
    server.clients.push(console);
    const audit = ok<{ entries: Array<{ kind: string; details: { credentialId?: string } }> }>(await console.call("auth.audit.list", {}));
    expect(audit.entries.filter((e) => e.kind === "credential.revoke" && e.details.credentialId === session.id).length).toBe(1);
  });

  test("resume_keeps_credential across a restart and refreshes its expiry", async () => {
    await server.start();
    const local = await server.local();
    const projectId = await project(local);
    const { session, credentialPath } = await open(server.core, local, projectId);
    server.core.store.startRun(session.id);
    server.core.store.apply(session.id, { kind: "interrupted", reason: "user" });
    await server.stop();
    server.time.advance(SESSION_IDLE_S * 1000 - 2 * DAY);
    await server.start();
    const resumed: Session = server.core.store.apply(session.id, { kind: "resumed" }).session;
    expect(resumed.status).not.toBe("ended");
    expect(server.core.credentials.drain()).toBe(1);
    // A month and more since the mint, but not since the resume: still live.
    server.time.advance(10 * DAY);
    const a = await server.agent(credentialPath);
    expect(ok<{ principal: Principal }>(await a.call("auth.whoami", {})).principal.id).toBe(session.id);
  });
});
