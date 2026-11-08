// A server-data folder, a control store and the plugin service over fin-node's `Registry`, for
// the plugin tests that build real registries (8A Task 9b review, Task 10). Call `plugins()`
// at the top of a `describe`: it registers the hooks that make and remove the folders.
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { FinError, Scope } from "@finisher/contracts";
import { Registry, testHooks } from "@finisher/fin-node";
import { Effect, Exit } from "effect";
import { afterEach, beforeEach } from "vitest";
import { Audit } from "../../src/control/audit.ts";
import { type ControlStore, openControlStore } from "../../src/control/db.ts";
import { makePlugins, type MakePluginsOptions } from "../../src/plugins/registries.ts";
import { pluginHandlers } from "../../src/plugins/rpc.ts";
import { PluginTables } from "../../src/plugins/tables.ts";

export const hooks = testHooks!;
export type Who = { readonly kind: string; readonly id: string; readonly human: boolean };
export const CONSOLE: Who = { kind: "console", id: "console", human: true };
export const DEVICE: Who = { kind: "device", id: "01device00000000000000000a", human: true };
export const LOCAL: Who = { kind: "local", id: "local", human: false };
export const AGENT: Who = { kind: "agent", id: "01agent000000000000000000a", human: false };
export const MCP: Who = { kind: "mcp", id: "01mcp00000000000000000000a", human: false };
export const MINUTE = 60_000;

export const run = <A>(e: Effect.Effect<A, FinError>) => Effect.runPromise(e);
export async function failure(e: Effect.Effect<unknown, FinError>): Promise<FinError | undefined> {
  const exit = await Effect.runPromiseExit(e);
  return Exit.isSuccess(exit) ? undefined : (Exit.findErrorOption(exit) as { value?: FinError }).value;
}
export const codeOf = async (e: Effect.Effect<unknown, FinError>) => (await failure(e))?.code;
export const as = (p: Who, scopes: ReadonlyArray<Scope> = []) => ({ headers: {}, guard: { denyBranches: [] }, principal: { ...p, scopes, projects: "*", branches: "*" } });

export interface Built {
  readonly dir: string;
  readonly keys: string;
  readonly rootSha256: string;
}

export const code = (id: string, version: string, permissions: object[] = [{ perm: "project:read" }]) => ({ id, version, kind: "code", permissions });
export const NET = { perm: "net", origins: ["https://api.example.org"] };
export const advisory = (plugin: string, versions: string, action: "warn" | "disable" = "disable") => ({ id: `ADV-${plugin.length}-${versions.length}`, plugin, versions, severity: "high", action, summary: "a test advisory" });

export function plugins() {
  const f = {
    tmp: "",
    serverData: "",
    control: undefined as unknown as ControlStore,
    now: 0,
    built: 0,
    registry(spec: object): Built {
      const dir = join(f.tmp, `registry-${++f.built}`);
      const keys = join(f.tmp, `keys-${f.built}`);
      const r = JSON.parse(hooks.buildRegistry(dir, keys, JSON.stringify(spec))) as { rootSha256: string };
      return { dir, keys, rootSha256: r.rootSha256 };
    },
    publish: (r: Built, spec: object) => hooks.publishToRegistry(r.dir, r.keys, JSON.stringify(spec)),
    service(options: MakePluginsOptions = {}) {
      const native = Registry.open(f.serverData);
      const tables = new PluginTables(f.control);
      const audit = new Audit(f.control, () => f.now);
      const plugins = makePlugins(native, tables, () => f.now, { audit, ...options });
      const h = pluginHandlers({ plugins, audit, guard: (_tag, self) => self });
      const rows = () =>
        (f.control.db.prepare("SELECT kind, principalKind, outcome, code, details FROM audit WHERE kind LIKE 'plugin.%' OR kind LIKE 'registry.%' ORDER BY id").all() as Array<{ kind: string; principalKind: string; outcome: string; code: string | null; details: string }>).map((r) => ({
          ...r,
          details: JSON.parse(r.details) as Record<string, unknown>,
        }));
      return { native, tables, plugins, life: plugins.lifecycle, audit, h, rows };
    },
    /** A service with registry `team` built from `spec`, added by the owner and refreshed. */
    async setup(spec: object, options: MakePluginsOptions = {}) {
      const reg = f.registry(spec);
      const s = f.service(options);
      await run(s.plugins.add({ cfg: { id: "team", path: reg.dir }, root: { kind: "sha256", sha256: reg.rootSha256 } }, CONSOLE));
      await run(s.plugins.refresh("team", CONSOLE));
      return { ...s, reg };
    },
    storeDirs(id: string): string[] {
      const [pub, name] = id.slice(1).split("/");
      const d = join(f.serverData, "plugins", "store", `@${pub}`, name!);
      return existsSync(d) ? readdirSync(d).filter((n) => !n.startsWith(".")).sort() : [];
    },
    /** `control.db`'s plugin rows and grants, for "nothing changed" checks. */
    snapshot: () => ({
      plugins: f.control.db.prepare("SELECT * FROM plugins ORDER BY id").all(),
      grants: f.control.db.prepare("SELECT pluginId, version, permission FROM plugin_grants ORDER BY rowid").all(),
    }),
  };
  beforeEach(() => {
    f.tmp = mkdtempSync(join(tmpdir(), "finisherd-plugins-"));
    f.serverData = join(f.tmp, "server-data");
    mkdirSync(f.serverData, { recursive: true });
    f.control = openControlStore(f.serverData, { serverId: "sid-1", checkpoint: false });
    f.now = Date.now();
  });
  afterEach(async () => {
    await f.control.close();
    rmSync(f.tmp, { recursive: true, force: true });
  });
  return f;
}
export type Setup = Awaited<ReturnType<ReturnType<typeof plugins>["setup"]>>;
