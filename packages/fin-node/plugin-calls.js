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

/** A command's argument schema compiled once (sp8b Task 9): `validate(args)`. */
export class PluginCommandArgs {
  #native;
  constructor(schema) {
    this.#native = sync(() => new native.PluginCommandArgs(JSON.stringify(schema)));
  }
  validate(args) {
    return JSON.parse(sync(() => this.#native.validate(JSON.stringify(args === undefined ? null : args))));
  }
}

/** A `plugin.command` job's result decoded and, for a transaction, checked against the project the plugin saw. */
export function pluginCommandOutcome(projectJson, out, txFields) {
  return JSON.parse(sync(() => native.pluginCommandOutcome(projectJson, JSON.stringify(out === undefined ? null : out), JSON.stringify(txFields))));
}
