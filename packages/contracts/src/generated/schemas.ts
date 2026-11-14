/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by packages/core-types/scripts/generate.mjs
 * (src/codegen/effect.ts): the Effect schemas of the RPC results, each checked against its generated type.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */
import type * as T from "@finisher/core-types";
import { Schema } from "effect";

type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : never) : never;
/** A value Rust has validated, passed through: `Schema.Unknown` typed as its generated type. */
const opaque = <A>() => Schema.Unknown as unknown as Schema.Codec<A>;

export const AgentModeSchema = Schema.Literals(["plan", "draft", "auto"]);
export const agentModeMatches: Same<typeof AgentModeSchema.Type, T.AgentMode> = true;
export const AssetKindSchema = Schema.Literals(["media", "image", "svg", "lottieTemplate", "font", "lut"]);
export const assetKindMatches: Same<typeof AssetKindSchema.Type, T.AssetKind> = true;
export const AudioCodecSchema = Schema.Union([Schema.Literals(["aac", "opus", "mp3", "flac", "alac", "pcm", "ac3", "eac3"]), Schema.Literal("other")], { mode: "oneOf" });
export const audioCodecMatches: Same<typeof AudioCodecSchema.Type, T.AudioCodec> = true;
export const AudioInfoSchema = Schema.Struct({
  channels: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)),
  sampleRate: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const audioInfoMatches: Same<typeof AudioInfoSchema.Type, T.AudioInfo> = true;
export const AudioTrackInfoSchema = Schema.Struct({
  channels: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)),
  codec: AudioCodecSchema,
  fourcc: Schema.String,
  padding: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  priming: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  sampleRate: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const audioTrackInfoMatches: Same<typeof AudioTrackInfoSchema.Type, T.AudioTrackInfo> = true;
export const BranchStateSchema = Schema.Literals(["active", "archived", "merged"]);
export const branchStateMatches: Same<typeof BranchStateSchema.Type, T.BranchState> = true;
export const BranchViewSchema = Schema.Struct({
  createdBy: Schema.String,
  display: Schema.String,
  forkSeq: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  head: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  id: Schema.String,
  name: Schema.String,
  parent: Schema.optionalKey(Schema.NullOr(Schema.String)),
  state: BranchStateSchema,
});
export const branchViewMatches: Same<typeof BranchViewSchema.Type, T.BranchView> = true;
export const CommittedTxSchema = opaque<T.CommittedTx>();
export const committedTxMatches: Same<typeof CommittedTxSchema.Type, T.CommittedTx> = true;
export const BranchCreatedSchema = Schema.Struct({
  branch: BranchViewSchema,
  dryRun: Schema.Boolean,
  record: CommittedTxSchema,
});
export const branchCreatedMatches: Same<typeof BranchCreatedSchema.Type, T.BranchCreated> = true;
export const BranchStatusSchema = Schema.Struct({
  branch: Schema.String,
  exportedThrough: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  forkSeq: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  generation: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  head: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  name: Schema.String,
  parent: Schema.optionalKey(Schema.NullOr(Schema.String)),
  state: BranchStateSchema,
});
export const branchStatusMatches: Same<typeof BranchStatusSchema.Type, T.BranchStatus> = true;
export const BusySchema = Schema.Struct({
  operation: Schema.String,
  sinceMs: Schema.Number.check(Schema.isInt()),
});
export const busyMatches: Same<typeof BusySchema.Type, T.Busy> = true;
export const NeighbourChangeSchema = Schema.Literals(["moved", "removed"]);
export const neighbourChangeMatches: Same<typeof NeighbourChangeSchema.Type, T.NeighbourChange> = true;
export const CauseSchema = Schema.Struct({
  author: Schema.optionalKey(Schema.String),
  change: NeighbourChangeSchema,
  neighbour: Schema.String,
  txId: Schema.String,
});
export const causeMatches: Same<typeof CauseSchema.Type, T.Cause> = true;
export const CheckSchema = Schema.Struct({
  details: Schema.Unknown,
  name: Schema.String,
  ok: Schema.Boolean,
});
export const checkMatches: Same<typeof CheckSchema.Type, T.Check> = true;
export const ChromaSchema = Schema.Literals(["mono", "yuv420", "yuv422", "yuv444"]);
export const chromaMatches: Same<typeof ChromaSchema.Type, T.Chroma> = true;
export const ChromaSitingSchema = Schema.Union([Schema.Literal("unknown"), Schema.Literal("left"), Schema.Literal("center"), Schema.Literal("topLeft")], { mode: "oneOf" });
export const chromaSitingMatches: Same<typeof ChromaSitingSchema.Type, T.ChromaSiting> = true;
export const MatrixSchema = Schema.Union([Schema.Literals(["bt709", "bt601", "unknown"]), Schema.Literal("bt2020Ncl"), Schema.Literal("identity")], { mode: "oneOf" });
export const matrixMatches: Same<typeof MatrixSchema.Type, T.Matrix> = true;
export const PrimariesSchema = Schema.Union([Schema.Literals(["bt709", "bt2020", "unknown"]), Schema.Literal("bt601_625"), Schema.Literal("bt601_525"), Schema.Literal("displayP3")], { mode: "oneOf" });
export const primariesMatches: Same<typeof PrimariesSchema.Type, T.Primaries> = true;
export const RangeSchema = Schema.Union([Schema.Literal("full"), Schema.Literal("limited")], { mode: "oneOf" });
export const rangeMatches: Same<typeof RangeSchema.Type, T.Range> = true;
export const TransferSchema = Schema.Union([Schema.Literals(["srgb", "linear", "unknown"]), Schema.Literal("bt709"), Schema.Literal("pq"), Schema.Literal("hlg")], { mode: "oneOf" });
export const transferMatches: Same<typeof TransferSchema.Type, T.Transfer> = true;
export const ColourTagsSchema = Schema.Struct({
  matrix: MatrixSchema,
  primaries: PrimariesSchema,
  range: RangeSchema,
  siting: ChromaSitingSchema,
  transfer: TransferSchema,
});
export const colourTagsMatches: Same<typeof ColourTagsSchema.Type, T.ColourTags> = true;
export const CompactReportSchema = Schema.Struct({
  abandoned: Schema.mutable(Schema.Array(Schema.mutable(Schema.Tuple([Schema.String, Schema.String])))),
  deleted: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  finished: Schema.mutable(Schema.Array(Schema.String)),
  manifests: Schema.mutable(Schema.Array(Schema.String)),
  packsWritten: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  replaced: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  skipped: Schema.optionalKey(Schema.NullOr(Schema.String)),
  waiting: Schema.mutable(Schema.Array(Schema.mutable(Schema.Tuple([Schema.String, Schema.String])))),
});
export const compactReportMatches: Same<typeof CompactReportSchema.Type, T.CompactReport> = true;
export const PendingCompactionSchema = Schema.Struct({
  branch: Schema.optionalKey(Schema.NullOr(Schema.String)),
  graceEndsMs: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt()))),
  manifest: Schema.String,
  reason: Schema.optionalKey(Schema.NullOr(Schema.String)),
  state: Schema.String,
  writerId: Schema.optionalKey(Schema.NullOr(Schema.String)),
});
export const pendingCompactionMatches: Same<typeof PendingCompactionSchema.Type, T.PendingCompaction> = true;
export const CompactionStatusSchema = Schema.Struct({
  lastError: Schema.optionalKey(Schema.NullOr(Schema.String)),
  lastRunMs: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt()))),
  pending: Schema.mutable(Schema.Array(PendingCompactionSchema)),
});
export const compactionStatusMatches: Same<typeof CompactionStatusSchema.Type, T.CompactionStatus> = true;
export const ConflictSchema = Schema.Struct({
  againstTx: Schema.optionalKey(Schema.NullOr(Schema.String)),
  details: Schema.optionalKey(Schema.Unknown),
  fix: Schema.String,
  opIndex: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  reason: Schema.String,
  txId: Schema.String,
});
export const conflictMatches: Same<typeof ConflictSchema.Type, T.Conflict> = true;
export const ContainerKindSchema = Schema.Union([Schema.Literal("mp4"), Schema.Literal("mov"), Schema.Literal("webm"), Schema.Literal("matroska")], { mode: "oneOf" });
export const containerKindMatches: Same<typeof ContainerKindSchema.Type, T.ContainerKind> = true;
export const ContentLightSchema = Schema.Struct({
  maxCll: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)),
  maxFall: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)),
});
export const contentLightMatches: Same<typeof ContentLightSchema.Type, T.ContentLight> = true;
export const CoreErrorSchema = Schema.Struct({
  code: Schema.String,
  details: Schema.Unknown,
  fix: Schema.optionalKey(Schema.String),
  message: Schema.String,
});
export const coreErrorMatches: Same<typeof CoreErrorSchema.Type, T.CoreError> = true;
export const DisplaySizeSchema = Schema.Struct({
  height: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  width: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const displaySizeMatches: Same<typeof DisplaySizeSchema.Type, T.DisplaySize> = true;
export const DolbyVisionSchema = Schema.Struct({
  baseLayer: Schema.Boolean,
  compatibility: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255)),
  enhancementLayer: Schema.Boolean,
  level: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255)),
  profile: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255)),
  rpu: Schema.Boolean,
});
export const dolbyVisionMatches: Same<typeof DolbyVisionSchema.Type, T.DolbyVision> = true;
export const FontFaceSchema = Schema.Struct({
  family: Schema.String,
  index: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  italic: Schema.Boolean,
  scripts: Schema.mutable(Schema.Array(Schema.String)),
  weight: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)),
});
export const fontFaceMatches: Same<typeof FontFaceSchema.Type, T.FontFace> = true;
export const FontFailedSchema = Schema.Struct({
  key: Schema.String,
  reason: Schema.String,
  retryAt: Schema.Number.check(Schema.isInt()),
  sha256: Schema.String,
});
export const fontFailedMatches: Same<typeof FontFailedSchema.Type, T.FontFailed> = true;
export const FontFetchingSchema = Schema.Struct({
  bytes: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  key: Schema.String,
  sha256: Schema.String,
  total: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const fontFetchingMatches: Same<typeof FontFetchingSchema.Type, T.FontFetching> = true;
export const FontFormatSchema = Schema.Literals(["ttf", "otf", "ttc"]);
export const fontFormatMatches: Same<typeof FontFormatSchema.Type, T.FontFormat> = true;
export const FontFileSchema = Schema.Struct({
  faces: Schema.mutable(Schema.Array(FontFaceSchema)),
  format: FontFormatSchema,
  sha256: Schema.String,
  size: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const fontFileMatches: Same<typeof FontFileSchema.Type, T.FontFile> = true;
export const FontMissingSchema = Schema.Struct({
  key: Schema.String,
  reason: Schema.String,
  sha256: Schema.String,
});
export const fontMissingMatches: Same<typeof FontMissingSchema.Type, T.FontMissing> = true;
export const FontsSummarySchema = Schema.Struct({
  failed: Schema.mutable(Schema.Array(FontFailedSchema)),
  fetching: Schema.mutable(Schema.Array(FontFetchingSchema)),
  missing: Schema.mutable(Schema.Array(FontMissingSchema)),
});
export const fontsSummaryMatches: Same<typeof FontsSummarySchema.Type, T.FontsSummary> = true;
export const HarnessSchema = Schema.Literals(["builtin", "claudeCode", "codex", "fake", "plugin"]);
export const harnessMatches: Same<typeof HarnessSchema.Type, T.Harness> = true;
export const MasteringDisplaySchema = Schema.Struct({
  maxLuminance: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  minLuminance: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  primaries: Schema.mutable(Schema.Tuple([Schema.mutable(Schema.Tuple([Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)), Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535))])), Schema.mutable(Schema.Tuple([Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)), Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535))])), Schema.mutable(Schema.Tuple([Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)), Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535))]))])),
  whitePoint: Schema.mutable(Schema.Tuple([Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535)), Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(65535))])),
});
export const masteringDisplayMatches: Same<typeof MasteringDisplaySchema.Type, T.MasteringDisplay> = true;
export const HdrInfoSchema = Schema.Struct({
  contentLight: Schema.optionalKey(ContentLightSchema),
  dolbyVision: Schema.optionalKey(DolbyVisionSchema),
  mastering: Schema.optionalKey(MasteringDisplaySchema),
});
export const hdrInfoMatches: Same<typeof HdrInfoSchema.Type, T.HdrInfo> = true;
export const HistoryLogSchema = Schema.Struct({
  head: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  records: Schema.mutable(Schema.Array(CommittedTxSchema)),
});
export const historyLogMatches: Same<typeof HistoryLogSchema.Type, T.HistoryLog> = true;
export const RationalSchema = Schema.String.check(Schema.isPattern(new RegExp("^(0|-?[1-9]\\d*(/([2-9]\\d*|1\\d+))?)$", "u")));
export const rationalMatches: Same<typeof RationalSchema.Type, T.Rational> = true;
export const LandedSchema = Schema.Struct({
  after: Schema.optionalKey(Schema.String),
  at: RationalSchema,
  before: Schema.optionalKey(Schema.String),
  frame: Schema.optionalKey(Schema.Number.check(Schema.isInt())),
  track: Schema.String,
});
export const landedMatches: Same<typeof LandedSchema.Type, T.Landed> = true;
export const PlacementOutcomeSchema = Schema.Literals(["clean", "relocated", "conflict"]);
export const placementOutcomeMatches: Same<typeof PlacementOutcomeSchema.Type, T.PlacementOutcome> = true;
export const PlacementSchema = Schema.Struct({
  causes: Schema.optionalKey(Schema.mutable(Schema.Array(CauseSchema))),
  item: Schema.String,
  landed: Schema.optionalKey(LandedSchema),
  opIndex: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  outcome: PlacementOutcomeSchema,
  txId: Schema.String,
});
export const placementMatches: Same<typeof PlacementSchema.Type, T.Placement> = true;
export const HistoryResultSchema = Schema.Struct({
  committed: Schema.mutable(Schema.Array(CommittedTxSchema)),
  dryRun: Schema.Boolean,
  noOp: Schema.optionalKey(Schema.String),
  origins: Schema.optionalKey(Schema.mutable(Schema.Array(Schema.String))),
  placements: Schema.optionalKey(Schema.mutable(Schema.Array(PlacementSchema))),
  undone: Schema.mutable(Schema.Array(Schema.String)),
});
export const historyResultMatches: Same<typeof HistoryResultSchema.Type, T.HistoryResult> = true;
export const HookEventSchema = Schema.Literals(["SessionStart", "SessionEnd", "UserPrompt", "PreCommand", "PostCommand", "BranchReady", "PreMerge", "PostMerge", "PreRender", "PostRender", "JobDone", "Approval", "Notification"]);
export const hookEventMatches: Same<typeof HookEventSchema.Type, T.HookEvent> = true;
export const HookProjectSchema = Schema.Struct({
  dir: Schema.String,
  id: Schema.String,
});
export const hookProjectMatches: Same<typeof HookProjectSchema.Type, T.HookProject> = true;
export const HookReplySchema = Schema.Union([Schema.Struct({
  reply: Schema.Literal("allow"),
}), Schema.Struct({
  reason: Schema.String,
  reply: Schema.Literal("block"),
}), Schema.Struct({
  reply: Schema.Literal("context"),
  text: Schema.String,
})], { mode: "oneOf" });
export const hookReplyMatches: Same<typeof HookReplySchema.Type, T.HookReply> = true;
export const HookSessionSchema = Schema.Struct({
  harness: HarnessSchema,
  id: Schema.String,
  mode: AgentModeSchema,
  role: Schema.String,
  rootBranch: Schema.optionalKey(Schema.String),
});
export const hookSessionMatches: Same<typeof HookSessionSchema.Type, T.HookSession> = true;
export const HookRequestSchema = Schema.Struct({
  event: HookEventSchema,
  payload: Schema.Unknown,
  project: Schema.optionalKey(HookProjectSchema),
  session: Schema.optionalKey(HookSessionSchema),
  v: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const hookRequestMatches: Same<typeof HookRequestSchema.Type, T.HookRequest> = true;
export const IgnoredTrackSchema = Schema.Struct({
  handler: Schema.String,
  id: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const ignoredTrackMatches: Same<typeof IgnoredTrackSchema.Type, T.IgnoredTrack> = true;
export const ImageInfoSchema = Schema.Struct({
  height: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  width: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const imageInfoMatches: Same<typeof ImageInfoSchema.Type, T.ImageInfo> = true;
export const LeaseStateSchema = Schema.Union([Schema.Struct({
  epoch: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  state: Schema.Literal("held"),
}), Schema.Struct({
  ageServerMs: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt()))),
  epoch: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  holder: Schema.String,
  state: Schema.Literal("heldElsewhere"),
}), Schema.Struct({
  epoch: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  holder: Schema.String,
  state: Schema.Literal("stale"),
}), Schema.Struct({
  reason: Schema.String,
  state: Schema.Literal("lost"),
})], { mode: "oneOf" });
export const leaseStateMatches: Same<typeof LeaseStateSchema.Type, T.LeaseState> = true;
export const SlotSpecSchema = Schema.Struct({
  id: Schema.String,
  kind: Schema.String,
});
export const slotSpecMatches: Same<typeof SlotSpecSchema.Type, T.SlotSpec> = true;
export const LottieInfoSchema = Schema.Struct({
  duration: RationalSchema,
  fonts: Schema.optionalKey(Schema.mutable(Schema.Array(Schema.String))),
  height: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  rate: RationalSchema,
  slots: Schema.optionalKey(Schema.mutable(Schema.Array(SlotSpecSchema))),
  width: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const lottieInfoMatches: Same<typeof LottieInfoSchema.Type, T.LottieInfo> = true;
export const LutInfoSchema = Schema.Struct({
  size: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const lutInfoMatches: Same<typeof LutInfoSchema.Type, T.LutInfo> = true;
export const RotationSchema = Schema.Literals([0, 90, 180, 270]);
export const rotationMatches: Same<typeof RotationSchema.Type, T.Rotation> = true;
export const SizeSchema = Schema.Struct({
  height: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  width: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const sizeMatches: Same<typeof SizeSchema.Type, T.Size> = true;
export const VideoCodecSchema = Schema.Union([Schema.Literals(["av1", "h264", "hevc", "vp9", "vp8", "proRes"]), Schema.Literal("other")], { mode: "oneOf" });
export const videoCodecMatches: Same<typeof VideoCodecSchema.Type, T.VideoCodec> = true;
export const VideoTrackInfoSchema = Schema.Struct({
  bitDepth: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255)),
  chroma: ChromaSchema,
  codec: VideoCodecSchema,
  coded: SizeSchema,
  colour: ColourTagsSchema,
  display: SizeSchema,
  fourcc: Schema.String,
  frames: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  hdr: Schema.optionalKey(HdrInfoSchema),
  icc: Schema.optionalKey(Schema.Boolean),
  level: Schema.optionalKey(Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255))),
  nominalRate: RationalSchema,
  pixelAspect: RationalSchema,
  profile: Schema.optionalKey(Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0), Schema.isLessThanOrEqualTo(255))),
  rotation: RotationSchema,
  vfr: Schema.Boolean,
});
export const videoTrackInfoMatches: Same<typeof VideoTrackInfoSchema.Type, T.VideoTrackInfo> = true;
export const MediaInfoSchema = Schema.Struct({
  audio: Schema.optionalKey(AudioTrackInfoSchema),
  container: ContainerKindSchema,
  duration: RationalSchema,
  fragmented: Schema.optionalKey(Schema.Boolean),
  ignored: Schema.optionalKey(Schema.mutable(Schema.Array(IgnoredTrackSchema))),
  video: Schema.optionalKey(VideoTrackInfoSchema),
});
export const mediaInfoMatches: Same<typeof MediaInfoSchema.Type, T.MediaInfo> = true;
export const MergeReportSchema = Schema.Struct({
  committed: Schema.mutable(Schema.Array(CommittedTxSchema)),
  dryRun: Schema.Boolean,
  excluded: Schema.mutable(Schema.Array(ConflictSchema)),
  mergeId: Schema.String,
  merged: Schema.mutable(Schema.Array(Schema.String)),
  notIncluded: Schema.optionalKey(Schema.mutable(Schema.Array(Schema.String))),
  origins: Schema.optionalKey(Schema.mutable(Schema.Array(Schema.String))),
  placements: Schema.optionalKey(Schema.mutable(Schema.Array(PlacementSchema))),
  skipped: Schema.mutable(Schema.Array(ConflictSchema)),
  sourceState: BranchStateSchema,
});
export const mergeReportMatches: Same<typeof MergeReportSchema.Type, T.MergeReport> = true;
export const VideoInfoSchema = Schema.Struct({
  display: Schema.optionalKey(DisplaySizeSchema),
  height: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  pixelAspect: Schema.optionalKey(RationalSchema),
  rate: RationalSchema,
  rotation: Schema.optionalKey(RotationSchema),
  width: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
});
export const videoInfoMatches: Same<typeof VideoInfoSchema.Type, T.VideoInfo> = true;
export const ProbeSchema = Schema.Struct({
  audio: Schema.optionalKey(AudioInfoSchema),
  duration: Schema.optionalKey(RationalSchema),
  fingerprint: Schema.String,
  font: Schema.optionalKey(FontFileSchema),
  image: Schema.optionalKey(ImageInfoSchema),
  kind: AssetKindSchema,
  lottie: Schema.optionalKey(LottieInfoSchema),
  lut: Schema.optionalKey(LutInfoSchema),
  media: Schema.optionalKey(MediaInfoSchema),
  size: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  video: Schema.optionalKey(VideoInfoSchema),
});
export const probeMatches: Same<typeof ProbeSchema.Type, T.Probe> = true;
export const QuarantineSummarySchema = Schema.Struct({
  count: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  origins: Schema.mutable(Schema.Array(Schema.String)),
  reasons: Schema.Record(Schema.String, Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0))),
});
export const quarantineSummaryMatches: Same<typeof QuarantineSummarySchema.Type, T.QuarantineSummary> = true;
export const ReadOnlyReasonSchema = Schema.Literals(["LEASE_HELD", "LEASE_LOST", "CHAIN_FORK", "CONFLICT_COPY", "NEWER_FORMAT", "IMPORT_ERROR", "CATCHING_UP"]);
export const readOnlyReasonMatches: Same<typeof ReadOnlyReasonSchema.Type, T.ReadOnlyReason> = true;
export const ReadOnlySchema = Schema.Struct({
  fix: Schema.String,
  reason: ReadOnlyReasonSchema,
});
export const readOnlyMatches: Same<typeof ReadOnlySchema.Type, T.ReadOnly> = true;
export const StatusWarningSchema = Schema.Struct({
  code: Schema.String,
  details: Schema.optionalKey(Schema.Unknown),
  fix: Schema.optionalKey(Schema.String),
  message: Schema.String,
});
export const statusWarningMatches: Same<typeof StatusWarningSchema.Type, T.StatusWarning> = true;
export const StorageKindSchema = Schema.Literals(["local", "network", "sync:dropbox", "sync:syncthing", "sync:icloud", "sync:onedrive", "sync:googledrive", "sync:unknown"]);
export const storageKindMatches: Same<typeof StorageKindSchema.Type, T.StorageKind> = true;
export const ProjectStatusSchema = Schema.Struct({
  branches: Schema.mutable(Schema.Array(BranchStatusSchema)),
  compaction: CompactionStatusSchema,
  exportError: Schema.optionalKey(Schema.NullOr(CoreErrorSchema)),
  foreign: Schema.mutable(Schema.Array(Schema.String)),
  importError: Schema.optionalKey(Schema.NullOr(CoreErrorSchema)),
  incarnation: Schema.String,
  lastExportMs: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt()))),
  lease: LeaseStateSchema,
  leaseCheck: Schema.optionalKey(Schema.NullOr(Schema.String)),
  pending: Schema.mutable(Schema.Array(Schema.String)),
  projectId: Schema.String,
  quarantine: QuarantineSummarySchema,
  readOnly: Schema.optionalKey(Schema.NullOr(ReadOnlySchema)),
  storage: StorageKindSchema,
  storageWarning: Schema.optionalKey(Schema.NullOr(Schema.String)),
  unknownBranches: Schema.mutable(Schema.Array(Schema.String)),
  waiting: Schema.mutable(Schema.Array(Schema.String)),
  warnings: Schema.mutable(Schema.Array(StatusWarningSchema)),
});
export const projectStatusMatches: Same<typeof ProjectStatusSchema.Type, T.ProjectStatus> = true;
export const ProjectStatusViewSchema = Schema.Struct({
  branches: Schema.mutable(Schema.Array(BranchStatusSchema)),
  busy: Schema.optionalKey(Schema.NullOr(BusySchema)),
  compaction: CompactionStatusSchema,
  exportError: Schema.optionalKey(Schema.NullOr(CoreErrorSchema)),
  fonts: Schema.optionalKey(FontsSummarySchema),
  foreign: Schema.mutable(Schema.Array(Schema.String)),
  importError: Schema.optionalKey(Schema.NullOr(CoreErrorSchema)),
  incarnation: Schema.String,
  lastExportMs: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt()))),
  lease: LeaseStateSchema,
  leaseCheck: Schema.optionalKey(Schema.NullOr(Schema.String)),
  pending: Schema.mutable(Schema.Array(Schema.String)),
  projectId: Schema.String,
  quarantine: QuarantineSummarySchema,
  readOnly: Schema.optionalKey(Schema.NullOr(ReadOnlySchema)),
  storage: StorageKindSchema,
  storageWarning: Schema.optionalKey(Schema.NullOr(Schema.String)),
  unknownBranches: Schema.mutable(Schema.Array(Schema.String)),
  waiting: Schema.mutable(Schema.Array(Schema.String)),
  warnings: Schema.mutable(Schema.Array(StatusWarningSchema)),
});
export const projectStatusViewMatches: Same<typeof ProjectStatusViewSchema.Type, T.ProjectStatusView> = true;
export const VerifyReportSchema = Schema.Struct({
  checks: Schema.mutable(Schema.Array(CheckSchema)),
  ok: Schema.Boolean,
});
export const verifyReportMatches: Same<typeof VerifyReportSchema.Type, T.VerifyReport> = true;
export const RebuildReportSchema = Schema.Struct({
  corruptCopy: Schema.optionalKey(Schema.NullOr(Schema.String)),
  genesisMismatch: Schema.Boolean,
  importError: Schema.optionalKey(Schema.NullOr(CoreErrorSchema)),
  keptForRecovery: Schema.mutable(Schema.Array(Schema.String)),
  missingFromFolder: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  problem: Schema.optionalKey(Schema.NullOr(Schema.String)),
  reason: Schema.String,
  rebuiltThrough: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  records: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  salvageComplete: Schema.Boolean,
  salvageErrors: Schema.mutable(Schema.Array(Schema.String)),
  salvaged: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  salvagedCopies: Schema.mutable(Schema.Array(Schema.String)),
  seededFromSnapshot: Schema.optionalKey(Schema.NullOr(Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)))),
  verify: VerifyReportSchema,
});
export const rebuildReportMatches: Same<typeof RebuildReportSchema.Type, T.RebuildReport> = true;
export const RecoverGroupSchema = Schema.Struct({
  branch: Schema.String,
  children: Schema.mutable(Schema.Array(Schema.String)),
  firstCommittedAt: Schema.Number.check(Schema.isInt()),
  forkGroup: Schema.optionalKey(Schema.NullOr(Schema.String)),
  forkHash: Schema.optionalKey(Schema.NullOr(Schema.String)),
  forkSeq: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  id: Schema.String,
  name: Schema.String,
  notes: Schema.mutable(Schema.Array(ConflictSchema)),
  origin: Schema.String,
  problems: Schema.mutable(Schema.Array(ConflictSchema)),
  reasons: Schema.Record(Schema.String, Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0))),
  records: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  recordsBranch: Schema.String,
  recoverable: Schema.Boolean,
  server: Schema.String,
});
export const recoverGroupMatches: Same<typeof RecoverGroupSchema.Type, T.RecoverGroup> = true;
export const SkippedFileSchema = Schema.Struct({
  path: Schema.String,
  reason: Schema.String,
});
export const skippedFileMatches: Same<typeof SkippedFileSchema.Type, T.SkippedFile> = true;
export const RecoverReportSchema = Schema.Struct({
  committed: Schema.mutable(Schema.Array(CommittedTxSchema)),
  created: Schema.mutable(Schema.Array(BranchViewSchema)),
  dismissed: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  dryRun: Schema.Boolean,
  groups: Schema.mutable(Schema.Array(RecoverGroupSchema)),
  imported: Schema.Number.check(Schema.isInt(), Schema.isGreaterThanOrEqualTo(0)),
  manual: Schema.optionalKey(Schema.NullOr(Schema.String)),
  skippedFiles: Schema.mutable(Schema.Array(SkippedFileSchema)),
});
export const recoverReportMatches: Same<typeof RecoverReportSchema.Type, T.RecoverReport> = true;
