// What a built-in session is opened with (plan 7A Decision 13; spec §4.4, §8.5): the system
// prompt (the role's prompt and the `fin` map only, so it is byte-identical across a role's
// sessions and `systemHash` deduplicates it), the working directory `workspace/<sessionId>/` in
// the project folder (made, private), and the session environment from the allow-list with
// `FIN_SERVER`, `FIN_SESSION` and `FIN_PROJECT_DIR` (the project folder `fin` trusts under the
// agent credential, never one the model names). The runtime adds `FIN_CREDENTIAL`; the `fin` tool adds
// `FIN_CALL`. Task 12 wires it with the roles and the server's endpoint.
import { execFile } from "node:child_process";
import { mkdir, stat } from "node:fs/promises";
import { join } from "node:path";
import type { Session } from "@finisher/contracts";
import { childEnv } from "../../env.ts";
import { finError } from "../../errors.ts";
import type { OpenEnvironment, SessionEnvironment } from "../runtime-deps.ts";

export interface BuiltinEnvironmentDeps {
  readonly finPath: string;
  /**
   * The endpoint `fin` connects to (`FIN_SERVER`): the local socket. The built-in `fin` tool runs
   * outside the OS sandbox (spec §8.9), so it does not use the Linux agent listener, which
   * sandboxed sessions get through the session wiring (plan 7C Tasks 6 and 11).
   */
  readonly server: string;
  readonly projectDirOf: (projectId: string) => string | undefined;
  /** The role's prompt (its skills included). */
  readonly rolePrompt: (role: string) => string;
}

const maps = new Map<string, Promise<string>>();

/** `fin` with no arguments (the command map), once per `fin` binary. */
export async function finMap(finPath: string): Promise<string> {
  const st = await stat(finPath);
  const key = `${finPath}\0${st.mtimeMs}\0${st.size}`;
  let p = maps.get(key);
  if (p === undefined) {
    p = new Promise<string>((ok, fail) =>
      execFile(finPath, [], { env: { PATH: process.env.PATH ?? "" }, timeout: 30_000 }, (err, stdout) => (err ? fail(err) : ok(stdout.trim()))),
    );
    maps.set(key, p);
    p.catch(() => maps.delete(key));
  }
  return p;
}

/** The system prompt of a role: its prompt, then the `fin` map. Never budget facts. */
export const systemPrompt = (rolePrompt: string, map: string) => `${rolePrompt}\n\n# fin\n\nYour one tool for the project is \`fin\`. Its command map:\n\n${map}\n`;

export function builtinEnvironment(deps: BuiltinEnvironmentDeps): SessionEnvironment {
  return {
    async prepare(s: Session): Promise<OpenEnvironment> {
      const dir = deps.projectDirOf(s.projectId);
      if (dir === undefined) throw finError("PROJECT_NOT_FOUND", "the session's project is not open on this server", { projectId: s.projectId });
      const cwd = join(dir, "workspace", s.id);
      await mkdir(cwd, { recursive: true, mode: 0o700 });
      const map = await finMap(deps.finPath).catch((e: unknown) => {
        throw finError("PROVIDER_UNAVAILABLE", `cannot run fin for its map: ${e instanceof Error ? e.message : String(e)}`, { finPath: deps.finPath }, "check agents.finPath");
      });
      return { system: systemPrompt(deps.rolePrompt(s.role), map), cwd, env: childEnv({ FIN_SERVER: deps.server, FIN_SESSION: s.id, FIN_PROJECT_DIR: dir }) };
    },
  };
}
