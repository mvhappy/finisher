// Plugin commands over RPC (sub-project 8B Task 9; spec §3.8). The shapes are fin-plugin-api's
// generated types (`PluginCommandList`, `PluginCommandRunRequest`, `PluginCommandRunResult`);
// the request schema below decodes the wire and is pinned to the generated type, so the two
// cannot drift (`_RequestMatches`).
import type { PluginCommandList, PluginCommandRunRequest, PluginCommandRunResult } from "@finisher/core-types";
import { Schema } from "effect";

export type { CommandOutcome, PluginCommandInfo, PluginCommandList, PluginCommandRunRequest, PluginCommandRunResult } from "@finisher/core-types";

/** `plugin:<@publisher/name>/<command>`: the registry name 7B's command rules match. */
export const PLUGIN_COMMAND_NAME = /^plugin:@[a-z0-9][a-z0-9-]{0,63}\/[a-z0-9][a-z0-9-]{0,63}\/[a-z0-9][a-z0-9-]{0,63}$/;

/** `plugin.commands` (`plugin.read`): the commands the caller can run and their digest. */
export const PluginCommandsPayload = Schema.Struct({});
export const PluginCommandsSuccess = Schema.Unknown;
export type PluginCommandsResult = PluginCommandList;

/**
 * `plugin.command.run` (`project.read`; `project.write` when the command writes): runs the
 * command on `projectId`'s `branch` (`main` when absent) with `args`; `dryRun` validates the
 * returned transaction against the snapshot the plugin saw and commits nothing.
 */
export const PluginCommandRunPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  name: Schema.String,
  args: Schema.optional(Schema.Unknown),
  dryRun: Schema.optional(Schema.Boolean),
});
export const PluginCommandRunSuccess = Schema.Unknown;
export type PluginCommandRunReply = PluginCommandRunResult;

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
type Wire = { -readonly [K in keyof typeof PluginCommandRunPayload.Type]: (typeof PluginCommandRunPayload.Type)[K] };
type Generated = { [K in keyof PluginCommandRunRequest]-?: Exclude<PluginCommandRunRequest[K], null> };
type WireAll = { [K in keyof Wire]-?: Exclude<Wire[K], undefined> };
/** Compile-time: the wire schema has the generated request's fields and types. */
export const _RequestMatches: Same<keyof WireAll, keyof Generated> = true;
