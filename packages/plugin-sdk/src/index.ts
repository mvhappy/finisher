// `@finisher/plugin-sdk`: the TypeScript SDK for Finisher code plugins (sub-project 8 spec §3,
// §7.2; plan 8B Task 13). A plugin process is a worker of 5C's `plugin` family and speaks the
// `fin-jobs` envelope on stdin and stdout (plan 8B Decision 2): `hello` with `v` 1, the job loop,
// `ready` after each job under a trailing `--warm`, and typed host calls over `call`/`reply`.
// Protocol types are generated into `@finisher/core-types`, never written here.
export { ENVELOPE_VERSION, EXIT_HOST_GONE, EXIT_PROTOCOL, MAX_LINE_BYTES, SDK_MINOR } from "./envelope.ts";
export { codes, PluginError, toPluginError } from "./errors.ts";
export { InputFile, OutputFile, openGrant } from "./files.ts";
export { type FetchOptions, Host, type HttpResponse, MAX_BODY_BYTES, PAGE_BYTES } from "./host.ts";
export {
  ALIVE_MS,
  CANCEL_GRACE_MS,
  COMMAND_KIND,
  COMMAND_KIND_VERSION,
  type CommandHandler,
  type CommandResult,
  definePlugin,
  HOOK_KIND,
  HOOK_KIND_VERSION,
  type HookHandler,
  type HookResult,
  type PluginDefinition,
  runPlugin,
  toCommandOut,
  toHookOut,
} from "./runtime.ts";
export type { HookReply, HookRequest } from "@finisher/core-types";
