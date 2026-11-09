// 5B Task 11: allowed roots for devices (Decision 32, spec §5.7). A device has `projects: "*"`
// but names server paths only inside `server.projectRoots`, the library roots and the folders
// of projects open here, all canonical; every path resolver is checked (`PATH_NOT_ALLOWED`).
// `local` and `console` keep full reach; agents keep their own roots.
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Principal } from "@finisher/contracts";
import { Effect, Fiber } from "effect";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { type ChainProjects, rootsOf } from "../src/chain/resolve.ts";
import { serve, StderrLogger } from "../src/main.ts";
import { defaultProjectRoots, readServerSettings } from "../src/net/server-settings.ts";
import { pairDevice, type TestDevice } from "./support/device.ts";
import { connectProved, type NdjsonClient, ok, type ServerInfo } from "./support/ndjson-client.ts";
import type { WsClient } from "./support/ws-client.ts";

let tmp: string;
let roots: string;
let fiber: Fiber.Fiber<unknown, unknown> | undefined;
let server: { info: ServerInfo; serverData: string };
const clients: Array<{ close(): unknown }> = [];

const writeRoots = (list: ReadonlyArray<string>) =>
  writeFileSync(join(server.serverData, "settings.json"), JSON.stringify({ server: { projectRoots: list } }));

beforeEach(async () => {
  tmp = realpathSync(mkdtempSync(join(tmpdir(), "finisherd-device-roots-")));
  roots = join(tmp, "roots");
  mkdirSync(roots);
  const serverData = join(tmp, "sd");
  mkdirSync(serverData, { mode: 0o700 });
  writeFileSync(join(serverData, "settings.json"), JSON.stringify({ server: { projectRoots: [roots] } }));
  let ready!: (i: ServerInfo) => void;
  const info = new Promise<ServerInfo>((r) => (ready = r));
  fiber = Effect.runFork(serve({ serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
  server = { info: await info, serverData };
});

afterEach(async () => {
  for (const c of clients.splice(0)) await c.close();
  if (fiber !== undefined) await Effect.runPromise(Fiber.interrupt(fiber));
  fiber = undefined;
  rmSync(tmp, { recursive: true, force: true });
});

async function asLocal(): Promise<NdjsonClient> {
  const c = await connectProved(server);
  clients.push(c);
  return c;
}

async function deviceWs(): Promise<{ device: TestDevice; ws: WsClient }> {
  const { device } = await pairDevice(server);
  const ws = await device.ws();
  clients.push(ws);
  return { device, ws };
}

type Exit = { _tag: "Success"; value: Record<string, unknown> } | { _tag: "Failure"; cause: unknown };
const exitOf = async (ws: WsClient, tag: string, payload: unknown) => (await ws.call(tag, payload)).exit as Exit;
const codeOf = (e: Exit) => (e._tag === "Success" ? "ok" : (/"code":"([A-Z_]+)"/.exec(JSON.stringify(e.cause))?.[1] ?? "?"));

describe("a device's roots over a server", () => {
  test("device_create_outside_roots_refused; inside works", async () => {
    const { ws } = await deviceWs();
    expect(codeOf(await exitOf(ws, "project.create", { dir: join(tmp, "Outside"), name: "O", rate: "30" }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "project.create", { dir: join(roots, "Inside"), name: "I", rate: "30" }))).toBe("ok");
  });

  test("device_open_outside_roots_refused", async () => {
    const { ws } = await deviceWs();
    mkdirSync(join(tmp, "NotAProject"));
    expect(codeOf(await exitOf(ws, "project.open", { dir: join(tmp, "NotAProject") }))).toBe("PATH_NOT_ALLOWED");
  });

  test("open_project_folder_is_a_root; device_probe_and_import_and_out_dir_checked; library roots", async () => {
    const local = await asLocal();
    const dir = join(tmp, "Open");
    const projectId = ok<{ projectId: string }>(await local.call("project.create", { dir, name: "P", rate: "30" })).projectId;
    writeFileSync(join(dir, "note.txt"), "x");
    writeFileSync(join(tmp, "outside.txt"), "x");
    const { ws } = await deviceWs();
    // The open project's folder is a root: opening it, and probing inside it.
    expect(codeOf(await exitOf(ws, "project.open", { dir }))).toBe("ok");
    // Read and refused for what it is, not where it is.
    expect(codeOf(await exitOf(ws, "asset.probe", { projectId, path: join(dir, "note.txt") }))).toBe("UNSUPPORTED_CONTAINER");
    // A symlink inside the open folder to a file outside every root.
    symlinkSync(join(tmp, "outside.txt"), join(dir, "alias.txt"));
    expect(codeOf(await exitOf(ws, "asset.probe", { projectId, path: join(dir, "alias.txt") }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "asset.probe", { projectId, path: join(tmp, "outside.txt") }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "project.recover", { projectId, importDir: join(tmp, "Elsewhere") }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "media.look", { projectId, at: ["0"], out: join(tmp, "Frames") }))).toBe("PATH_NOT_ALLOWED");
    // `library.add`'s root too (spec §5.7), absolute or not.
    mkdirSync(join(tmp, "Media"));
    expect(codeOf(await exitOf(ws, "library.add", { root: join(tmp, "Media") }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "library.add", { root: "Media" }))).toBe("PATH_NOT_ALLOWED");
    mkdirSync(join(roots, "Media"));
    expect(codeOf(await exitOf(ws, "library.add", { root: join(roots, "Media") }))).toBe("ok");
    // `registry.add`'s folder and root file (review of Task 11).
    expect(codeOf(await exitOf(ws, "registry.add", { cfg: { id: "r", path: join(tmp, "Reg") }, root: { kind: "file", path: join(tmp, "outside.txt") } }))).toBe("PATH_NOT_ALLOWED");
  });

  test("a device cannot widen its roots or name TLS files (review of Task 11)", async () => {
    const { ws } = await deviceWs();
    expect(codeOf(await exitOf(ws, "settings.write", { path: "server.projectRoots", value: ["/"] }))).toBe("PATH_NOT_ALLOWED");
    const files = { enabled: true, listen: "127.0.0.1:0", tls: { mode: "files", certFile: join(tmp, "c.pem"), keyFile: join(tmp, "k.pem") } };
    expect(codeOf(await exitOf(ws, "settings.write", { path: "server.remote", value: files }))).toBe("PATH_NOT_ALLOWED");
    expect(readServerSettings(server.serverData).projectRoots).toEqual([roots]);
  });

  test("a device's reopen by id is checked against its roots (review of Task 11)", async () => {
    const local = await asLocal();
    const outside = ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "Closed"), name: "C", rate: "30" })).projectId;
    const inside = ok<{ projectId: string }>(await local.call("project.create", { dir: join(roots, "Kept"), name: "K", rate: "30" })).projectId;
    for (const c of clients.splice(0)) await c.close();
    // A restart closes every project; the index still knows their folders.
    await Effect.runPromise(Fiber.interrupt(fiber!));
    let ready!: (i: ServerInfo) => void;
    const info = new Promise<ServerInfo>((r) => (ready = r));
    fiber = Effect.runFork(serve({ serverData: server.serverData, onReady: (i) => ready(i as ServerInfo) }).pipe(Effect.provide(StderrLogger)));
    server = { info: await info, serverData: server.serverData };
    const { ws } = await deviceWs();
    expect(codeOf(await exitOf(ws, "project.status", { projectId: outside }))).toBe("PATH_NOT_ALLOWED");
    expect(codeOf(await exitOf(ws, "project.status", { projectId: inside }))).toBe("ok");
  });

  test("symlink_out_of_root_refused (canonical paths)", async () => {
    const { ws } = await deviceWs();
    mkdirSync(join(tmp, "Target"));
    symlinkSync(join(tmp, "Target"), join(roots, "link"));
    expect(codeOf(await exitOf(ws, "project.create", { dir: join(roots, "link", "P"), name: "P", rate: "30" }))).toBe("PATH_NOT_ALLOWED");
  });

  test("missing_root_skipped_not_created; roots are read live", async () => {
    const missing = join(tmp, "Missing");
    writeRoots([missing]);
    const { ws } = await deviceWs();
    expect(codeOf(await exitOf(ws, "project.create", { dir: join(missing, "P"), name: "P", rate: "30" }))).toBe("PATH_NOT_ALLOWED");
    expect(() => realpathSync(missing)).toThrow();
    writeRoots([roots]);
    expect(codeOf(await exitOf(ws, "project.create", { dir: join(roots, "Later"), name: "L", rate: "30" }))).toBe("ok");
  });

  test("local_and_console_keep_full_reach", async () => {
    const local = await asLocal();
    expect(ok<{ projectId: string }>(await local.call("project.create", { dir: join(tmp, "L"), name: "L", rate: "30" })).projectId).toBeTruthy();
    const c = await connectProved(server, "console");
    clients.push(c);
    const created = await c.call("project.create", { dir: join(tmp, "C"), name: "C", rate: "30" });
    expect(created.ok).toBe(true);
  });
});

describe("rootsOf (units)", () => {
  const principal = (kind: Principal["kind"], projects: Principal["projects"]): Principal => ({ kind, id: "p", scopes: [], projects, branches: "*", human: kind !== "agent" });
  const deps = (over: Partial<ChainProjects> = {}): ChainProjects => ({
    isOpen: () => false,
    dirOf: () => undefined,
    resolveBranch: () => Effect.succeed("b"),
    branches: () => Effect.succeed([]),
    ...over,
  });

  test("default_project_root_is_home_finisher", () => {
    expect(defaultProjectRoots("/home/someone")).toEqual([join("/home/someone", "Finisher")]);
    const empty = realpathSync(mkdtempSync(join(tmpdir(), "finisherd-settings-")));
    try {
      expect(readServerSettings(empty).projectRoots).toEqual(defaultProjectRoots());
    } finally {
      rmSync(empty, { recursive: true, force: true });
    }
  });

  test("a device's roots: canonical, missing ones skipped; none when not wired", async () => {
    const dir = realpathSync(mkdtempSync(join(tmpdir(), "finisherd-roots-")));
    try {
      mkdirSync(join(dir, "real"));
      symlinkSync(join(dir, "real"), join(dir, "alias"));
      const got = await Effect.runPromise(rootsOf(principal("device", "*"), deps({ allowedRoots: () => Effect.succeed([join(dir, "alias"), join(dir, "gone")]) })));
      expect(got).toEqual([join(dir, "real")]);
      expect(await Effect.runPromise(rootsOf(principal("device", "*"), deps()))).toEqual([]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("agent_reach_unchanged; local and console have none", async () => {
    const dir = realpathSync(mkdtempSync(join(tmpdir(), "finisherd-roots-")));
    try {
      const d = deps({ dirOf: (id) => (id === "A" ? dir : undefined), allowedRoots: () => Effect.succeed(["/never"]) });
      expect(await Effect.runPromise(rootsOf(principal("agent", ["A"]), d))).toEqual([dir]);
      expect(await Effect.runPromise(rootsOf(principal("local", "*"), d))).toBeUndefined();
      expect(await Effect.runPromise(rootsOf(principal("console", "*"), d))).toBeUndefined();
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
