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
 * pack `id`, which must be installed, enabled and not revoked. Answers, by what is wrong:
 * `PLUGIN_NOT_INSTALLED` (no such plugin), `NOT_FOUND` (a code plugin, or a pack with no style
 * of that name; `details.available` lists the pack's), `PLUGIN_REVOKED` (an advisory revoked
 * it), `PLUGIN_BUNDLED_MISMATCH` (a bundled pack that does not match the app) and
 * `PLUGIN_DISABLED` (the owner switched it off), and `PLUGIN_MANIFEST_INVALID {reason:
 * contentInvalid}` when its files no longer validate. Only caption styles are read this way;
 * font catalogs are merged into `fonts.catalog`.
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
 * style, else its key. The server validated `style` with the host's own parser
 * (`parse_style_file`), so the schema checks the envelope and the type below narrows `style` to
 * the generated `CaptionStyle`.
 */
export const PluginContentGetSuccess = Schema.Struct({
  id: Schema.String,
  version: Schema.String,
  kind: Schema.Literal("captionStyle"),
  name: PackStyleNameSchema,
  title: Schema.optional(Schema.String),
  style: Schema.Record(Schema.String, Schema.Unknown),
});
export type PluginContentResult = Omit<typeof PluginContentGetSuccess.Type, "style"> & { readonly style: Omit<CaptionStyle, "id"> };
