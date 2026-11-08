// The library RPCs (plan 5D Task 13): scopes and the human-only matrix through the chain, the
// handlers over a real engine and catalogs, reach by catalogued row for `asset.probe`, the
// watch stream, and what the audit and the journal may hold.
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { exitCodeOf } from "../../src/console/exit-codes.ts";
import {
  defaultScopes,
  type FinError,
  FinRpcs,
  HUMAN_ONLY,
  type Principal,
  RPC_SCOPES,
  requiredScopes,
  type Scope,
  SCOPES,
  STREAM_RPCS,
} from "@finisher/contracts";
import { Effect, Exit, Stream } from "effect";
import { afterEach, describe, expect, test } from "vitest";
import { issueChallenge } from "../../src/auth/challenges.ts";
import { chainOf, withChain } from "../../src/chain/chain.ts";
import { humanOnly, pathNotAllowed, scopeDenied } from "../../src/chain/fixes.ts";
import { Connections } from "../../src/connections.ts";
import type { Audit, AuditInput } from "../../src/control/audit.ts";
import { CONSOLE, LOCAL } from "../../src/principal.ts";
import { probeFor } from "../../src/probe.ts";
import { libraryHandlers } from "../../src/library/rpc.ts";
import { relComponents } from "../../src/library/paths.ts";
import { cataloguedAt, libraryOf } from "../../src/library/reach.ts";
import { failure } from "../support/jobs.ts";
import { SAMPLE_PAYLOADS } from "../support/payloads.ts";
import { overlay, tree } from "./support/kinds.ts";
import { closeLiveRigs, type LiveRig, liveRig, liveRoots } from "./support/live.ts";

afterEach(closeLiveRigs, 60_000);

const TAGS = Object.keys(RPC_SCOPES).filter((t) => t.startsWith("library.")) as Array<keyof typeof RPC_SCOPES>;
const READS = ["library.list", "library.status", "library.files", "library.file", "library.search", "library.lookup", "library.watch"] as const;
const OWNER = ["library.add", "library.remove"] as const;

const credential = (kind: "agent" | "mcp", scopes: Scope[], projects: Principal["projects"] = "*"): Principal => ({ kind, id: "01agent000000000000000000a", scopes, projects, branches: "*", human: false });

// ---- scopes through the chain ----------------------------------------------------------

function chainHarness(principal: Principal, tags: ReadonlyArray<string> = TAGS) {
  const seen: string[] = [];
  const audited: AuditInput[] = [];
  const connections = new Connections("socket", () => "/sd/run/fin.sock");
  connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
  connections.get(1)!.principal = principal;
  const streams = new Set<string>(STREAM_RPCS);
  const handlers = Object.fromEntries(
    tags.map((tag) => [tag, () => (streams.has(tag) ? Stream.make(1).pipe(Stream.tap(() => Effect.sync(() => void seen.push(tag)))) : Effect.sync(() => (seen.push(tag), {})))]),
  );
  const audit = { append: (e: AuditInput) => audited.push(e) } as unknown as Audit;
  const projects = { isOpen: () => false, dirOf: () => undefined, resolveBranch: () => Effect.succeed("main"), branches: () => Effect.succeed([]) };
  const chained = withChain(handlers, { connections, projects, audit }) as unknown as Record<string, (p: unknown, o: unknown) => unknown>;
  const call = async (tag: string, extra: object = {}, typed?: string): Promise<string | undefined> => {
    const payload = { ...(SAMPLE_PAYLOADS[tag as keyof typeof SAMPLE_PAYLOADS] as object), ...extra, ...(typed === undefined ? {} : { challenge: typed }) };
    const out = chained[tag]!(payload, { client: { id: 1 }, headers: {} });
    const exit = await Effect.runPromiseExit(Stream.isStream(out) ? Stream.runDrain(out as Stream.Stream<unknown, FinError>) : (out as Effect.Effect<unknown, FinError>));
    return Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value?.code;
  };
  const challenge = () => issueChallenge(connections.get(1)!).code;
  return { seen, audited, call, challenge };
}

describe("scopes", () => {
  test("rpc_scopes_cover_library_rpcs", () => {
    const tags = [...FinRpcs.requests.keys()].filter((t) => t.startsWith("library."));
    expect(tags.sort()).toEqual([...TAGS].sort());
    expect(TAGS).toHaveLength(11);
    for (const tag of READS) expect(RPC_SCOPES[tag].scopes, tag).toEqual(["library.read"]);
    for (const tag of OWNER) expect(RPC_SCOPES[tag].scopes, tag).toEqual(["library.admin"]);
    expect(RPC_SCOPES["library.scan"].scopes).toEqual(["library.scan", "jobs.submit"]);
    expect(RPC_SCOPES["library.set"].scopes).toEqual(["library.scan"]);
    // Nothing of the library writes a project branch, and only the watch streams.
    for (const tag of TAGS) expect(RPC_SCOPES[tag].writes, tag).toBeUndefined();
    expect(STREAM_RPCS).toContain("library.watch");
    expect(HUMAN_ONLY.has("library.admin")).toBe(true);
    // The default scopes: a local tool reads and scans; only the console administers.
    expect(defaultScopes("local")).toEqual(expect.arrayContaining(["library.read", "library.scan", "jobs.submit"]));
    expect(defaultScopes("local")).not.toContain("library.admin");
  });

  test("set_non_human_fields_with_library_scan", async () => {
    const plain = { id: "x", name: "Raw", include: ["a"], exclude: ["b"], kinds: "all", scan: { poll: 60 }, paused: true, stages: [] };
    expect(requiredScopes("library.set", plain)).toEqual(["library.scan"]);
    const a = credential("agent", ["library.scan"]);
    expect(await chainHarness(a).call("library.set", plain)).toBeUndefined();
    // Each field only a person decides adds `library.admin`, which is human-only.
    for (const field of [{ hydrate: true }, { bandwidthMiBs: 40 }, { bandwidthMiBs: null }, { acceptVolume: true }, { aliases: ["/mnt/x"] }, { aliases: [] }]) {
      expect(requiredScopes("library.set", { id: "x", ...field }), JSON.stringify(field)).toEqual(["library.scan", "library.admin"]);
      expect(await chainHarness(credential("agent", [...SCOPES])).call("library.set", field), JSON.stringify(field)).toBe("HUMAN_ONLY");
      expect(await chainHarness(LOCAL).call("library.set", field)).toBe("HUMAN_ONLY");
    }
  });

  test("scan_needs_jobs_submit_too", async () => {
    expect(await chainHarness(credential("agent", ["library.scan"])).call("library.scan")).toBe("SCOPE_DENIED");
    expect(await chainHarness(credential("agent", ["jobs.submit"])).call("library.scan")).toBe("SCOPE_DENIED");
    expect(await chainHarness(credential("agent", ["library.scan", "jobs.submit"])).call("library.scan")).toBeUndefined();
    expect(await chainHarness(LOCAL).call("library.scan")).toBeUndefined();
    // Hashing candidates submits jobs: `library.read` alone is not enough for it.
    const reader = credential("agent", ["library.read"]);
    expect(await chainHarness(reader).call("library.lookup")).toBeUndefined();
    expect(await chainHarness(reader).call("library.lookup", { hashCandidates: true })).toBe("SCOPE_DENIED");
    expect(await chainHarness(LOCAL).call("library.lookup", { hashCandidates: true })).toBeUndefined();
  });

  test("human_only_matrix_for_add_remove_aliases_and_admin_fields", async () => {
    const calls: Array<[string, object]> = [["library.add", {}], ["library.remove", {}], ["library.set", { aliases: ["/mnt/x"] }], ["library.set", { hydrate: true }], ["library.set", { bandwidthMiBs: 5 }], ["library.set", { acceptVolume: true }]];
    for (const [tag, extra] of calls) {
      for (const p of [LOCAL, credential("agent", [...SCOPES]), credential("mcp", [...SCOPES])]) {
        const h = chainHarness(p);
        expect(await h.call(tag, extra), `${tag} ${JSON.stringify(extra)} × ${p.kind}`).toBe("HUMAN_ONLY");
        expect(h.seen).toEqual([]);
        expect(h.audited.map((a) => [a.kind, a.code, a.details?.rpc])).toEqual([["auth.denied", "HUMAN_ONLY", tag]]);
      }
      // Only the console passes, with the connection's typed code, once.
      const owner = chainHarness(CONSOLE);
      expect(await owner.call(tag, extra), tag).toBe("CONFIRMATION_REQUIRED");
      expect(await owner.call(tag, extra, owner.challenge()), tag).toBeUndefined();
      expect(owner.seen).toEqual([tag]);
    }
    // The ordinary fields need no code from a person.
    expect(await chainHarness(CONSOLE).call("library.set", { paused: true })).toBeUndefined();
  });

  test("fix_texts_contain_no_command", async () => {
    // A refusal's fix is for a person: no backticks, no `fin …` or `finisherd …` command line.
    const fixes: string[] = [];
    for (const scope of ["library.admin", "library.scan"] as const) {
      fixes.push(humanOnly("library.add", scope, LOCAL).fix ?? "", scopeDenied("library.scan", [scope], LOCAL).fix ?? "");
    }
    for (const bad of ["../x", "/abs", "C:\\x", "a/../b"]) {
      try {
        relComponents({ root: "/r" }, bad);
      } catch (e) {
        fixes.push((e as FinError).fix ?? "");
      }
    }
    fixes.push((await failure(libraryOf({ registry: { resolve: () => undefined } } as never, LOCAL, "x"))).fix ?? "");
    fixes.push(pathNotAllowed("asset.probe", "/x").fix ?? "");
    for (const fix of fixes) {
      expect(fix.length).toBeGreaterThan(0);
      expect(fix).not.toMatch(/`/);
      expect(fix).not.toMatch(/\bfin(isherd)?\s+[a-z-]+/);
    }
  });

  test("exit_codes_mapped", () => {
    const want: Record<string, number> = {
      LIBRARY_ROOT_INVALID: 3, LIBRARY_OVERLAP: 3, LIBRARY_NOT_FOUND: 3, LIBRARY_NAME_TAKEN: 3, FILE_NOT_FOUND: 3, INVALID_PATH: 3,
      PATH_AMBIGUOUS: 3, CURSOR_EXPIRED: 3, CURSOR_INVALID: 3, SEARCH_QUERY_INVALID: 3, LIBRARY_METADATA_ONLY: 3, NAME_COLLISION: 3,
      FILE_CLOUD_ONLY: 4, FILE_CHANGED: 4,
      LIBRARY_OFFLINE: 5, LIBRARY_PAUSED: 5, LIBRARY_ROOT_UNREADABLE: 5, CATALOG_TOO_NEW: 5, ROOT_EMPTY_SUSPECT: 5, DIR_EMPTY_SUSPECT: 5,
      ROOT_UNMOUNTED: 5, WATCH_DEGRADED: 5, ENUMERATION_FALLBACK: 5, RESOURCE_STUCK: 5,
      HUMAN_ONLY: 7, PATH_NOT_ALLOWED: 7,
    };
    for (const [code, exit] of Object.entries(want)) expect(exitCodeOf(code), code).toBe(exit);
  });
});

// ---- the handlers over a real engine ------------------------------------------------------

const ACTOR = CONSOLE;
const guard = <A>(_tag: string, self: Effect.Effect<A, FinError>) => self;
const optionsOf = (principal: Principal) => ({ client: { id: 1 }, headers: {}, guard: { denyBranches: [] }, principal });

function handlers(rig: LiveRig) {
  const h = libraryHandlers({ service: rig.svc, engine: rig.t.engine, guard }) as unknown as Record<string, (req: unknown, o: unknown) => unknown>;
  const call = <A>(tag: string, req: object, principal: Principal = ACTOR): Promise<A> => Effect.runPromise(h[tag]!(req, optionsOf(principal)) as Effect.Effect<A, FinError>);
  const fail = (tag: string, req: object, principal: Principal = ACTOR): Promise<FinError> => failure(h[tag]!(req, optionsOf(principal)) as Effect.Effect<unknown, FinError>);
  const stream = (tag: string, req: object, principal: Principal = ACTOR) => h[tag]!(req, optionsOf(principal)) as Stream.Stream<Record<string, unknown>, FinError>;
  return { call, fail, stream };
}

const MEDIA = { "": ["a.png", "b.mov"], clips: ["c.png", "interview_take3.final.mov"] };

async function addAndScan(rig: LiveRig, root: string, extra: object = {}) {
  const h = handlers(rig);
  const added = await h.call<{ library: { id: string; name: string }; job?: { id: string } }>("library.add", { root, name: "Raw", ...extra });
  expect((await rig.t.wait(added.job!.id, 30_000)).state).toBe("succeeded");
  return { h, id: added.library.id, added };
}

describe("handlers", () => {
  test("library_rpcs_round_trip", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, MEDIA);
    const rig = await liveRig();
    const { h, id, added } = await addAndScan(rig, root);
    // A scan job as a client sees it carries no params.
    expect(added.job).toMatchObject({ kind: "library.scan", params: {} });
    expect((await h.call<{ libraries: unknown[] }>("library.list", {})).libraries).toHaveLength(1);
    const status = await h.call<{ library: { counts: Record<string, number> }; stages: unknown[] }>("library.status", { id: "raw" });
    expect(Object.values(status.library.counts).reduce((a, b) => a + b, 0)).toBe(4);
    const listing = await h.call<{ dir: { display: string }; dirs: Array<{ name: string; display: string }>; files: Array<{ name: string; display: string }> }>("library.files", { id });
    expect(listing.dir.display).toBe("");
    expect(listing.dirs.map((d) => [d.name, d.display])).toEqual([["clips", "clips"]]);
    expect(listing.files.map((f) => f.display)).toEqual(["a.png", "b.mov"]);
    const sub = await h.call<{ dir: { display: string }; files: Array<{ display: string }>; next?: string }>("library.files", { id, dir: "clips/", limit: 1 });
    expect(sub.dir.display).toBe("clips");
    expect(sub.files.map((f) => f.display)).toEqual(["clips/c.png"]);
    expect(sub.next).toBeDefined();
    const page2 = await h.call<{ files: Array<{ display: string }> }>("library.files", { id, dir: "clips", limit: 1, cursor: sub.next });
    expect(page2.files.map((f) => f.display)).toEqual(["clips/interview_take3.final.mov"]);
    expect((await h.fail("library.files", { id, cursor: "garbage" })).code).toBe("CURSOR_INVALID");
    const file = await h.call<{ file: { display: string; state: string } }>("library.file", { id, path: "clips/c.png" });
    expect(file.file).toMatchObject({ display: "clips/c.png", state: "present" });
    expect((await h.fail("library.file", { id, path: "nope.png" })).code).toBe("FILE_NOT_FOUND");
    const hits = await h.call<{ hits: Array<{ display: string }>; stale: boolean }>("library.search", { query: "interview" });
    expect(hits.hits.map((x) => x.display)).toEqual(["clips/interview_take3.final.mov"]);
    expect(hits.stale).toBe(false);
    expect((await h.call<{ hits: unknown[] }>("library.search", { query: "interview", libraries: ["RAW"] })).hits).toHaveLength(1);
    expect((await h.fail("library.search", { query: "x", libraries: ["nope"] })).code).toBe("LIBRARY_NOT_FOUND");
    const scan = await h.call<{ job: { id: string; params: unknown } }>("library.scan", { id: "raw", dirs: ["clips"] });
    expect(scan.job.params).toEqual({});
    expect((await h.fail("library.scan", { id: "raw", dirs: ["nope"] })).code).toBe("FILE_NOT_FOUND");
    const set = await h.call<{ library: { name: string; state: string } }>("library.set", { id: "raw", name: "Footage", paused: true });
    expect(set.library).toMatchObject({ name: "Footage", state: "paused" });
    expect((await h.fail("library.scan", { id: "Footage" })).code).toBe("LIBRARY_PAUSED");
    await h.call("library.remove", { id });
    expect((await h.fail("library.status", { id })).code).toBe("LIBRARY_NOT_FOUND");
  });

  test("files_refuses_escaping_paths", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, MEDIA);
    const rig = await liveRig();
    const { h, id } = await addAndScan(rig, root);
    for (const dir of ["../x", "clips/../../x", "/etc", "C:\\Windows", "a/./b"]) expect((await h.fail("library.files", { id, dir })).code, dir).toBe("INVALID_PATH");
    for (const path of ["../x.png", "/etc/passwd", "clips/../../a.png", "a\0b"]) expect((await h.fail("library.file", { id, path })).code, path).toBe("INVALID_PATH");
    expect((await h.fail("library.scan", { id, dirs: ["../.."] })).code).toBe("INVALID_PATH");
    expect((await h.fail("library.file", { id })).code).toBe("INVALID_REQUEST");
  });

  test("a_library_out_of_reach_is_not_found", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, MEDIA);
    const rig = await liveRig();
    const { h, id } = await addAndScan(rig, root);
    const blind = credential("agent", ["server.read"]);
    expect((await h.fail("library.status", { id }, blind)).code).toBe("LIBRARY_NOT_FOUND");
    expect((await h.call<{ libraries: unknown[] }>("library.list", {}, blind)).libraries).toEqual([]);
    expect((await h.call<{ hits: unknown[] }>("library.search", { query: "interview" }, blind)).hits).toEqual([]);
    expect((await h.call<{ matches: unknown[] }>("library.lookup", { fingerprints: ["fp1:00"] }, blind)).matches).toEqual([]);
  });

  test("status_warns_about_flagged_directories", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, { d: ["a.mov", "b.mov"], keep: ["c.mov"] });
    const rig = await liveRig();
    const { h, id } = await addAndScan(rig, root);
    for (const f of ["a.mov", "b.mov"]) rmSync(join(root, "d", f));
    const scan = await h.call<{ job: { id: string } }>("library.scan", { id, dirs: ["d"] });
    expect((await rig.t.wait(scan.job.id, 30_000)).state).toBe("succeeded");
    // A directory that suddenly lists nothing is only suspect until a later listing says so (Decision 25).
    const status = await h.call<{ library: { warnings: Array<{ code: string; dir?: string }> } }>("library.status", { id });
    expect(status.library.warnings).toContainEqual({ code: "DIR_EMPTY_SUSPECT", dir: "d" });
    const listed = await h.call<{ libraries: Array<{ warnings: unknown[] }> }>("library.list", {});
    expect(listed.libraries[0]!.warnings).toContainEqual({ code: "DIR_EMPTY_SUSPECT", dir: "d" });
    // Its files stay present meanwhile.
    expect((await h.call<{ library: { counts: Record<string, number> } }>("library.status", { id })).library.counts.missing).toBeUndefined();
  });

  test("watch_starts_with_snapshot_and_marks_in_use", { timeout: 90_000 }, async () => {
    const root = join(liveRoots(), "raw");
    tree(root, MEDIA);
    const rig = await liveRig();
    const { h, id } = await addAndScan(rig, root);
    const holders = () => rig.svc.usage.holders(id);
    const before = holders();
    let held = -1;
    const first = await Effect.runPromise(
      Stream.runCollect(
        h.stream("library.watch", {}).pipe(
          Stream.tap(() => Effect.sync(() => (held = holders()))),
          Stream.take(1),
        ),
      ),
    );
    expect(first[0]).toMatchObject({ type: "snapshot", libraries: [{ id, name: "Raw" }] });
    expect(held).toBe(before + 1);
    expect(holders()).toBe(before);
    // Then what changes: a scan's states and a file event.
    const seen: string[] = [];
    const run = Effect.runPromise(
      Stream.runDrain(
        h.stream("library.watch", { ids: ["Raw"] }).pipe(
          Stream.tap((e) => Effect.sync(() => void seen.push(String(e.type)))),
          Stream.takeUntil((e) => e.type === "scan" && (e.job as { state: string }).state === "succeeded"),
        ),
      ),
    );
    await new Promise((r) => setTimeout(r, 300));
    writeFileSync(join(root, "new.mov"), "x");
    await h.call("library.scan", { id, full: true });
    await run;
    expect(seen[0]).toBe("snapshot");
    expect(seen).toContain("scan");
    expect(holders()).toBe(before);
    expect((await Effect.runPromise(Stream.runCollect(h.stream("library.watch", { ids: ["nope"] }).pipe(Stream.take(1))).pipe(Effect.flip)) as FinError).code).toBe("LIBRARY_NOT_FOUND");
  });

  test("journal_and_audit_carry_no_names", { timeout: 90_000 }, async () => {
    const audited: AuditInput[] = [];
    const root = join(liveRoots(), "Secret Footage");
    tree(root, MEDIA);
    const alias = "/mnt/secret-nas/Secret Footage";
    const rig = await liveRig({ audit: { append: (e: AuditInput) => void audited.push(e) } as unknown as Audit });
    const { h, id } = await addAndScan(rig, root, { name: "Secret Name", aliases: [alias], hydrate: true });
    await h.call("library.set", { id, hydrate: false, paused: true, name: "Other Secret" });
    await h.call("library.remove", { id });
    expect(audited.map((a) => a.kind)).toEqual(["library.add", "library.set", "library.remove"]);
    const everything = JSON.stringify([audited, rig.journal]);
    for (const word of ["Secret", "secret", root, alias, "Footage", "Other"]) expect(everything, word).not.toContain(word);
    expect(audited[1]!.details).toMatchObject({ libraryId: id, fields: ["hydrate"] });
  });
});

// ---- asset reach by catalogued row ----------------------------------------------------------

describe("asset.probe of library files", () => {
  const projects = (rig: LiveRig, folder: string) => ({
    isOpen: () => true,
    dirOf: (id: string) => (id === "P" ? folder : undefined),
    resolveBranch: () => Effect.succeed("main"),
    branches: () => Effect.succeed([]),
    libraryFile: (path: string) => cataloguedAt(rig.svc, path),
  });
  /** `asset.probe` through the chain for `principal`: the code, or the probe's kind. */
  function prober(rig: LiveRig, principal: Principal, folder: string) {
    const connections = new Connections("socket", () => "/sd/run/fin.sock");
    connections.attach(1, { close() {}, closeAfterReply() {}, terminate() {} });
    connections.get(1)!.principal = principal;
    const deps = projects(rig, folder);
    const handlers = {
      "asset.probe": ({ projectId, path }: { projectId: string; path: string }, options: object) => probeFor(chainOf(options)?.principal, deps, projectId, path),
    };
    const chained = withChain(handlers, { connections, projects: deps }) as unknown as Record<string, (p: unknown, o: unknown) => Effect.Effect<{ kind: string }, FinError>>;
    return async (path: string): Promise<string> => {
      const exit = await Effect.runPromiseExit(chained["asset.probe"]!({ projectId: "P", path }, { client: { id: 1 }, headers: {} }));
      return Exit.isSuccess(exit) ? `ok:${exit.value.kind}` : ((Exit.findErrorOption(exit) as { value?: FinError }).value?.code ?? "?");
    };
  }

  test("asset_probe_of_library_file_uses_catalogued_row", { timeout: 90_000 }, async () => {
    const data = realpathSync(mkdtempSync(join(tmpdir(), "fin-assetreach-")));
    const base = liveRoots();
    const root = join(base, "raw");
    const alias = join(base, "mirror");
    const project = join(data, "project");
    mkdirSync(project, { recursive: true });
    tree(root, { "": ["a.png", "tw\u00e9in.png"], clips: ["c.png"] });
    // The twin: the same name in NFD (Linux keeps both; the catalog folds them to one key).
    writeFileSync(join(root, "twe\u0301in.png"), Buffer.from(readPng()));
    tree(alias, { "": ["a.png"], clips: ["c.png"] });
    const rig = await liveRig();
    try {
      await addAndScan(rig, root, { aliases: [alias] });
      const reader = credential("agent", ["project.write", "library.read"], ["P"]);
      const probe = prober(rig, reader, project);
      expect(await probe(join(root, "a.png"))).toBe("ok:image");
      expect(await probe(join(root, "clips", "c.png"))).toBe("ok:image");
      // A file under the root that the catalog does not hold grants nothing by itself.
      writeFileSync(join(root, "uncatalogued.png"), Buffer.from(readPng()));
      expect(await probe(join(root, "uncatalogued.png"))).toBe("PATH_NOT_ALLOWED");
      // Neither does an alias: another machine's path of the same root.
      expect(await probe(join(alias, "a.png"))).toBe("PATH_NOT_ALLOWED");
      // Twins (two rows under one key) are ambiguous.
      if (process.platform === "linux") expect(await probe(join(root, "tw\u00e9in.png"))).toBe("PATH_AMBIGUOUS");
      // A principal that does not read libraries has no reach to them.
      expect(await prober(rig, credential("agent", ["project.write"], ["P"]), project)(join(root, "a.png"))).toBe("PATH_NOT_ALLOWED");
      // The owner's local tools are unrestricted, but a library file is still read through its row:
      // a file that changed since it was catalogued is `FILE_CHANGED`, not probed by name.
      const local = prober(rig, LOCAL, project);
      expect(await local(join(root, "a.png"))).toBe("ok:image");
      writeFileSync(join(root, "a.png"), Buffer.concat([Buffer.from(readPng()), Buffer.alloc(9)]));
      expect(await local(join(root, "a.png"))).toBe("FILE_CHANGED");
      expect(await probe(join(root, "a.png"))).toBe("FILE_CHANGED");
      // Outside every library, a plain path still probes (the local principal reads anywhere).
      tree(join(base, "elsewhere"), { "": ["z.png"] });
      expect(await local(join(base, "elsewhere", "z.png"))).toBe("ok:image");
    } finally {
      rmSync(data, { recursive: true, force: true });
    }
  });

  test("asset_probe_of_cloud_only_is_file_cloud_only_without_reading", { timeout: 90_000 }, async () => {
    const base = liveRoots();
    const root = join(base, "raw");
    tree(root, { "": ["cloud.png", "a.png"] });
    overlay(root, { paths: { "cloud.png": { placeholder: "dataless" } } });
    const rig = await liveRig();
    const { id } = await addAndScan(rig, root);
    expect(rig.svc.registry.get(id)!.hydrate).toBe(false);
    const project = join(base, "p");
    mkdirSync(project);
    const reader = credential("agent", ["project.write", "library.read"], ["P"]);
    // The row says `cloudOnly`: refused before anything is opened, for the reaching agent and the owner alike.
    expect(await prober(rig, reader, project)(join(root, "cloud.png"))).toBe("FILE_CLOUD_ONLY");
    expect(await prober(rig, LOCAL, project)(join(root, "cloud.png"))).toBe("FILE_CLOUD_ONLY");
    expect(await prober(rig, LOCAL, project)(join(root, "a.png"))).toBe("ok:image");
  });
});

/** A 1×1 PNG (the one `tree` writes for `.png` names). */
function readPng(): Uint8Array {
  const dir = mkdtempSync(join(tmpdir(), "fin-png-"));
  try {
    tree(dir, { "": ["x.png"] });
    return readFileSync(join(dir, "x.png"));
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}
