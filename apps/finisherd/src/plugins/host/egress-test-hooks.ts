// Test-only hooks of the egress broker (sp8b plan Global Constraints, Task 8): a resolver in
// place of `dns.lookup`, named `(host, address)` pairs let through the private-address check
// (so the tests' loopback HTTPS server is reachable by name, never by a literal), the test
// server's throwaway CA and shorter limits. `makeEgress` refuses hooks unless fin-node is its
// `test-hooks` build, and `check-test-hooks` refuses an import of this file from `src/`.
import { testHooks } from "@finisher/fin-node";
import type { EgressTestHooks } from "./egress.ts";

export function egressTestHooks(h: Omit<EgressTestHooks, "testOnly">): EgressTestHooks {
  if (testHooks === undefined) throw new Error("egress test hooks need a test-hooks build of fin-node");
  return Object.freeze({ ...h, testOnly: true as const });
}
