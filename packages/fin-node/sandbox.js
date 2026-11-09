// The sandbox status (plan 7C Task 3), the session wiring's data (Task 11) and the ACL broker (Task 10).
import { sandboxExecPath as pathIn } from "./loader.js";
import { binaryDir, call, native, sync } from "./native.js";

/** `fin-sandbox-exec[.exe]` in `dir` (default: the directory the fin-node binary was loaded from). */
export function sandboxExecPath(dir) {
  return pathIn(dir ?? binaryDir, process.platform);
}

export function sandboxStatus() {
  return JSON.parse(sync(() => native.sandboxStatus()));
}

export function sandboxSessionGitConfig() {
  return native.sandboxSessionGitConfig();
}

export function sandboxToolchainBase() {
  return native.sandboxToolchainBase();
}

export function sandboxAgentCheck(input) {
  return native.sandboxAgentCheck(JSON.stringify(input)) ?? null;
}

// The ACL broker (plan 7C Task 10): grants and revokes run on the libuv pool.
export function aclGrant(serverData, grant) {
  return call(native.aclGrant(serverData, grant.path, grant.sid, grant.mask, grant.inherit, grant.owner));
}

export function aclRevoke(serverData, owner, path) {
  return call(native.aclRevoke(serverData, owner, path ?? null));
}

export function aclSweep(serverData, liveOwners) {
  return call(native.aclSweep(serverData, [...liveOwners]));
}

export function sandboxAgentGrants(input, extraRead) {
  return JSON.parse(sync(() => native.sandboxAgentGrants(JSON.stringify(input), extraRead === undefined ? undefined : [...extraRead])));
}

export function sandboxHookGrants(input) {
  return JSON.parse(sync(() => native.sandboxHookGrants(JSON.stringify(input))));
}

export function sandboxVisitGrant(workerDir, jobDir) {
  return JSON.parse(sync(() => native.sandboxVisitGrant(workerDir, jobDir)));
}

export function sandboxProjectContainer(projectDir) {
  return native.sandboxProjectContainer(projectDir);
}

export function sandboxContainerSid(name) {
  return sync(() => native.sandboxContainerSid(name)) ?? null;
}

/** @internal Test builds only: the broker's live grants. */
export function aclGrantsForTest(serverData) {
  if (!native.aclGrantsForTest) throw new Error("fin-node was built without the test-hooks feature");
  return JSON.parse(sync(() => native.aclGrantsForTest(serverData)));
}
