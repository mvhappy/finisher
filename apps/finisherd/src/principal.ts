// The principals this server binds in 5A (spec §3.2). `agent` and `mcp` come from credentials
// (Task 9); `device` (5B) and `plugin` (8) are reserved.
import { defaultScopes, type Principal } from "@finisher/contracts";

export type { Principal } from "@finisher/contracts";

/** A connection that has not proved anything: it may only say hello (spec §4.2). */
export const ANONYMOUS: Principal = Object.freeze({
  kind: "anonymous",
  id: "",
  scopes: defaultScopes("anonymous"),
  projects: "*",
  branches: "*",
  human: false,
});

/** `fin` and scripts the user runs: proved with `local.key`. */
export const LOCAL: Principal = Object.freeze({
  kind: "local",
  id: "local",
  scopes: defaultScopes("local"),
  projects: "*",
  branches: "*",
  human: false,
});

/** A person at the server (`finisherd <command>`): proved with `console.key`. */
export const CONSOLE: Principal = Object.freeze({
  kind: "console",
  id: "console",
  scopes: defaultScopes("console"),
  projects: "*",
  branches: "*",
  human: true,
});

export function isProved(p: Principal): boolean {
  return p.kind !== "anonymous";
}
