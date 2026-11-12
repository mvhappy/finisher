// fin-core's event registry and the journal's `privacy` check (the diagnostics journal is `Journal` in index.js).
import { native } from "./native.js";

/** Every kind name of fin-core's event registry. */
export function kindNames() {
  return native.kindNames();
}

/** The newest payload version of kind `name` that `source` may emit; `null` for none (reserved, unknown, not that source). */
export function kindVersion(name, source) {
  return native.kindVersion(name, source) ?? null;
}

/** Why a `privacy` section (JSON) is not one the journal accepts, naming the key; `undefined` when it is. */
export function validatePrivacy(privacyJson) {
  return native.validatePrivacy(privacyJson) ?? undefined;
}
