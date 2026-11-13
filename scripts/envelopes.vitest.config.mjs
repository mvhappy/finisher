// vitest config of the envelope gate (scripts/check-envelopes.mjs): the one contracts test, none of
// the repository's global setup (that builds the native test-hooks binary).
import { defineConfig } from "vitest/config";

export default defineConfig({
  test: { include: ["packages/contracts/src/envelopes.test.ts"] },
});
