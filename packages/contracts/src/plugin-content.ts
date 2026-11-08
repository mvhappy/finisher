// What installed content packs contribute, over RPC (sub-project 8A Task 12a; spec §4.2). Font
// catalogs join the server's catalog by themselves (`fonts.catalog` lists them); a caption
// style is read here and copied into a project by `fin style add --from @pub/pack:<name>`
// (Task 12c) through the existing `style.add` op.
import type { CaptionStyle } from "@finisher/core-types";
import { Schema } from "effect";
import { PluginIdSchema } from "./plugin-lifecycle.ts";

/** A caption style's name in its pack (`[a-z0-9][a-z0-9-]{0,63}`, 8A Decision 4's manifest rule). */
export const PackStyleNameSchema = Schema.String.check(Schema.isPattern(/^[a-z0-9][a-z0-9-]{0,63}$/));

/**
 * `plugin.content.get` (`plugin.read`): the contribution `name` of `kind` from the content
 * pack `id`, which must be installed, enabled and not revoked (`PLUGIN_NOT_FOUND` otherwise,
 * whether or not it is installed: a disabled pack serves nothing). Only caption styles are read
 * this way; font catalogs are merged into `fonts.catalog`.
 */
export const PluginContentGetPayload = Schema.Struct({
  id: PluginIdSchema,
  kind: Schema.Literal("captionStyle"),
  name: PackStyleNameSchema,
});
export type PluginContentGetRequest = typeof PluginContentGetPayload.Type;

/**
 * The style as the pack ships it, checked as `style.add` checks its values: a `CaptionStyle`
 * without its `id` (the project gives its copy one). `title` is the pack's `en` message for the
 * style, else its key.
 */
export interface PluginContentResult {
  readonly id: string;
  readonly version: string;
  readonly kind: "captionStyle";
  readonly name: string;
  readonly title?: string;
  readonly style: Omit<CaptionStyle, "id">;
}
export const PluginContentGetSuccess = Schema.Unknown;
