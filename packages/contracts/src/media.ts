// The evaluation, `media.*` and `fonts.*` payloads of `FinRpcs` (`rpc.ts`).
import type { AudioPlan, FontCatalog, FontsPlan, FontsStatus, FramePlan } from "@finisher/core-types";
import { Schema } from "effect";

/**
 * Plan 4C Decisions 40–43: evaluation runs on the server. `branch` defaults to `main` (an id,
 * a name or `<name>#<id>`), `timeline` to the main (first) timeline; times are rational
 * strings (`"12"`, `"1001/30000"`). Captions are measured with the server's shared measurer
 * from the project's pinned font bytes; a caption whose bytes are missing is unmeasured
 * (`layout: null`, `unmeasured`), never an error. `rev`: the branch rev the caller resolved
 * `timeline` at (`snapshot.get`'s); a named timeline missing from a newer head was removed
 * since, `CONFLICT` (`details.reason: TIMELINE_GONE`; read the project again and retry), while
 * one missing at that rev (or with no `rev`) is `NOT_FOUND`.
 */
export const EvalFramePayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  rev: Schema.optional(Schema.Number),
  at: Schema.String,
});
export const EvalAudioPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  rev: Schema.optional(Schema.Number),
  from: Schema.String,
  to: Schema.String,
});
/**
 * Plan 6A Task 13: frames of a timeline as PNGs, rendered by a `media.look` job on the media
 * worker. `at`: 1 to 64 rational times (`"12"`, `"1001/30000"`, canonical; past the timeline's
 * end is `INVALID_RANGE`). `branch` defaults to `main`, `timeline` to the first. `out`: an
 * absolute folder for the files (`workspace/shared/look/` of the project when absent; never
 * inside `.finisher/`, `assets/` or the server's data, `EXPORT_DESTINATION`); naming it needs
 * `project.write` on top of `project.read` and `jobs.submit`; an agent or MCP principal writes
 * only inside its project's folder, a device inside the server's allowed roots (5B Decision 32). `size`: each side 1 to 8192 (the timeline's resolution when
 * absent). `wait` (default true) answers the `LookResult` (frames with their final paths, what
 * was `skipped`, the adapter) once the job finished, or its error; `wait: false` answers
 * `{job}` at once.
 */
export const MediaLookPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.optional(Schema.String),
  timeline: Schema.optional(Schema.String),
  at: Schema.Array(Schema.String),
  out: Schema.optional(Schema.String),
  size: Schema.optional(Schema.Struct({ width: Schema.Number, height: Schema.Number })),
  wait: Schema.optional(Schema.Boolean),
});
/** The generated `LookResult` (`wait`), or `{job}` (the job's view: `wait: false`). */
export const MediaLookSuccess = Schema.Unknown;
/** The generated `MediaCapabilities`: `{codecs, gpu, worker}`. */
export const MediaCapabilitiesSuccess = Schema.Unknown;
/** fin-core's `FramePlan` / `AudioPlan` (the generated types). */
export const FramePlanSchema = Schema.Unknown;
export const AudioPlanSchema = Schema.Unknown;
export type FramePlanPayload = FramePlan;
export type AudioPlanPayload = AudioPlan;

/** fin-fonts' `FontCatalog`: the catalog this server fetches from. */
export const FontCatalogSchema = Schema.Unknown;
export type FontCatalogPayload = FontCatalog;
/**
 * fin-fonts' `FontsStatus`: every font file's state in this server (`cached | missing |
 * queued | fetching {bytes, total} | failed {reason, retryAtMs}`), `autoFetch` and
 * `damaged`. A project's own font summary is in `project.status` (`fonts {missing, fetching,
 * failed}`, warnings `FONT_MISSING`, `FONT_FETCH_FAILED`, `FONTS_DAMAGED`).
 */
export const FontStatusSchema = Schema.Unknown;
export type FontStatusPayload = FontsStatus;
/**
 * `fonts.fetch`: fetch now and wait (background fetching being off does not stop it).
 * `sha256`: catalog files; `entries`: catalog entry ids (each of their styles); `keys`: font
 * keys `<family>|<weight>|<normal|italic>` (the catalog's face); `pinned`: a project id
 * (what its branches pin). Fails with `FONT_FETCH_FAILED` (`details {key, entry, style,
 * sha256, reason, retryAt}`), `FONT_NOT_IN_CATALOG`, `PROJECT_NOT_FOUND`, `FONTS_DAMAGED`.
 */
export const FontsFetchPayload = Schema.Struct({
  sha256: Schema.optional(Schema.Array(Schema.String)),
  entries: Schema.optional(Schema.Array(Schema.String)),
  keys: Schema.optional(Schema.Array(Schema.String)),
  pinned: Schema.optional(Schema.String),
});
/**
 * `fonts.plan` (Decision 35): the `font.pin` ops a client prepends to its own tx of `ops`
 * before committing it; `notes` list faces or scripts the catalog cannot pin. The server
 * applies `ops` to a copy of the head (a tx that cannot apply fails here as at commit) and
 * never alters a client's tx. Planned files are fetched.
 */
export const FontsPlanPayload = Schema.Struct({
  projectId: Schema.String,
  branch: Schema.String,
  ops: Schema.Array(Schema.Unknown),
});
export const FontsPlanSchema = Schema.Unknown;
export type FontsPlanResult = FontsPlan;
