// The plugin host's call helpers (sp8b Task 7; `crates/fin-node/src/plugin_calls.rs`).
import { native, sync } from "./native.js";

/** A host call's params decoded by its `fin-plugin-api` type (see `index.d.ts`). */
export function decodePluginCall(method, params) {
  return JSON.parse(sync(() => native.decodePluginCall(method, JSON.stringify(params === undefined ? null : params))));
}

/** `fin view`'s screenplay of a snapshot's project, offline: `{lines, elided, tokens}`. */
export function pluginProjectView(projectJson, range, budget) {
  return JSON.parse(sync(() => native.pluginProjectView(projectJson, range ?? null, budget ?? null)));
}
