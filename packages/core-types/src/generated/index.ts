/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "IdRef".
 */
export type IdRef = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ActorKind".
 */
export type ActorKind = "user" | "agent" | "plugin" | "system" | "unknown";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VerdictBy".
 */
export type VerdictBy = "user" | "critic" | "agent";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Verdict".
 */
export type Verdict = "accepted" | "rejected" | "undone" | "merged" | "abandoned";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentMode".
 */
export type AgentMode = "interactive" | "headless" | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentRole".
 */
export type AgentRole = "main" | "subagent" | "critic" | "other";
/**
 * A name from code or a static table (fin-core events::names); "other" for a value not in the table.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Name".
 */
export type Name = string;
/**
 * Large or structured content (the reserved agent kinds): inline JSON, or a [`BlobRef`] when
 * it would not fit an entry (Decision 20).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Body".
 */
export type Body =
  | {
      inline: unknown;
    }
  | {
      blob: BlobRef;
    };
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BlobHash".
 */
export type BlobHash = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Hashed".
 */
export type Hashed = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Code".
 */
export type Code = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StopReason".
 */
export type StopReason = "endTurn" | "toolUse" | "maxTokens" | "stopSequence" | "refusal" | "other";
/**
 * Horizontal alignment of each line inside the layout box.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Align".
 */
export type Align = "left" | "center" | "right";
/**
 * The safe-zone edge a caption block is placed against.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Anchor".
 */
export type Anchor = "top" | "middle" | "bottom";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AnyPayload".
 */
export type AnyPayload =
  | SessionStart
  | SessionEnd
  | SessionUnclean
  | ConsentChanged
  | JournalDropped
  | LogCommit
  | CliCommand
  | RpcStats
  | EvalStats
  | FontsFetch
  | StorageState
  | ErrorInternal
  | ErrorPanic
  | AgentSession
  | AgentTurn
  | AgentToolCall
  | AgentToolResult
  | AgentEditVerdict
  | UiAction
  | ReviewVerdict;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Version".
 */
export type Version = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Arch".
 */
export type Arch = "x86_64" | "aarch64" | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "OsFamily".
 */
export type OsFamily = "linux" | "macos" | "windows" | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BuildProfile".
 */
export type BuildProfile = "debug" | "release";
/**
 * What decided a purpose (Decision 8): the build default, the environment, or the user's
 * settings. Ordered weakest first; a stamp with two bases carries the weaker
 * ([`Basis::weaker`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Basis".
 */
export type Basis = "default" | "env" | "user";
/**
 * A purpose events are recorded for.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Purpose".
 */
export type Purpose = "diagnostics" | "training";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Purposes".
 */
export type Purposes = Purpose[];
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchId".
 */
export type BranchId = string;
/**
 * A primitive edit operation, tagged by `op`.
 *
 * `clip.insert`, `clip.trim` and `clip.delete` are client aliases of the `item.*` ops (plan 4C
 * Decision 6): [`crate::normalize`] rewrites them before a commit, so a stored transaction
 * never holds one, and internal and replay applies refuse them (`INVALID_OP`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Op".
 */
export type Op =
  | (AssetAdd & {
      op: "asset.add";
    })
  | (ItemInsert & {
      op: "item.insert";
    })
  | (ItemTrim & {
      op: "item.trim";
    })
  | (ItemDelete & {
      op: "item.delete";
    })
  | (ItemSlip & {
      op: "item.slip";
    })
  | (ItemSplit & {
      op: "item.split";
    })
  | (ItemMove & {
      op: "item.move";
    })
  | (ItemSlide & {
      op: "item.slide";
    })
  | (ItemLink & {
      op: "item.link";
    })
  | (ItemUnlink & {
      op: "item.unlink";
    })
  | (TransitionSet & {
      op: "transition.set";
    })
  | (TransitionRemove & {
      op: "transition.remove";
    })
  | (TrackAdd & {
      op: "track.add";
    })
  | (TrackRemove & {
      op: "track.remove";
    })
  | (TrackMove & {
      op: "track.move";
    })
  | (TrackSet & {
      op: "track.set";
    })
  | (TimelineAdd & {
      op: "timeline.add";
    })
  | (TimelineSet & {
      op: "timeline.set";
    })
  | (TimelineRemove & {
      op: "timeline.remove";
    })
  | (MarkerAdd & {
      op: "marker.add";
    })
  | (MarkerSet & {
      op: "marker.set";
    })
  | (MarkerRemove & {
      op: "marker.remove";
    })
  | (BinAdd & {
      op: "bin.add";
    })
  | (BinMove & {
      op: "bin.move";
    })
  | (BinRemove & {
      op: "bin.remove";
    })
  | (AssetMove & {
      op: "asset.move";
    })
  | (TrackRestore & {
      op: "track.restore";
    })
  | (TimelineRestore & {
      op: "timeline.restore";
    })
  | {
      id: string;
      op: "asset.remove";
    }
  | (ItemRemove & {
      op: "item.remove";
    })
  | (ItemRestore & {
      op: "item.restore";
    })
  | (ParamSet & {
      op: "param.set";
    })
  | (EffectAdd & {
      op: "effect.add";
    })
  | (EffectRemove & {
      op: "effect.remove";
    })
  | (EffectMove & {
      op: "effect.move";
    })
  | (KeyframeSet & {
      op: "keyframe.set";
    })
  | (KeyframeRemove & {
      op: "keyframe.remove";
    })
  | (EffectRestore & {
      op: "effect.restore";
    })
  | (StyleAdd & {
      op: "style.add";
    })
  | (StyleSet & {
      op: "style.set";
    })
  | (StyleRemove & {
      op: "style.remove";
    })
  | (FontPinOp & {
      op: "font.pin";
    })
  | (FontUnpin & {
      op: "font.unpin";
    })
  | (FontRepin & {
      op: "font.repin";
    })
  | (CaptionSetText & {
      op: "caption.setText";
    })
  | (CaptionSetWords & {
      op: "caption.setWords";
    })
  | (CaptionSplit & {
      op: "caption.split";
    })
  | (CaptionMerge & {
      op: "caption.merge";
    })
  | (CaptionSetStyle & {
      op: "caption.setStyle";
    })
  | {
      asset: string;
      at: Rational;
      duration: Rational;
      id: string;
      op: "clip.insert";
      sourceIn: Rational;
      track: string;
    }
  | {
      clip: string;
      delta: Rational;
      edge: Edge;
      op: "clip.trim";
    }
  | {
      clip: string;
      op: "clip.delete";
    };
/**
 * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rational".
 */
export type Rational = string;
/**
 * A font file's container format.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontFormat".
 */
export type FontFormat = "ttf" | "otf" | "ttc";
/**
 * `item.insert`: an item of any kind (the flattened `body`, tagged by `kind`) at `at` on
 * `track`, `duration` long, with default common fields. `id` is a `$name` placeholder bound
 * to a new id of the item's kind.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemInsert".
 */
export type ItemInsert = {
  at: Rational;
  duration: Rational;
  id: string;
  mode: InsertMode;
  track: string;
} & ItemInsert1;
/**
 * How an insert makes room (plan 4C Decision 18): `ripple` moves every later item on the
 * track by the item's duration; `overwrite` takes the range over, removing the items inside it
 * and trimming the ones overlapping its ends (an item strictly containing it is
 * `OVERWRITE_SPLITS_ITEM`: split it first, `compose::overwrite`), and moves nothing else.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "InsertMode".
 */
export type InsertMode = "ripple" | "overwrite";
export type ItemInsert1 =
  | {
      asset: string;
      kind: "clip";
      sourceIn: Rational;
      speed?: Rational;
    }
  | {
      kind: "caption";
      overrides?: {
        [k: string]: ParamValue;
      };
      source?: CaptionSource;
      style: string;
      words: Word[];
    }
  | {
      asset: string;
      kind: "graphic";
      slots?: {
        [k: string]: ParamValue;
      };
    }
  | {
      asset: string;
      kind: "image";
    }
  | {
      kind: "adjustment";
    }
  | {
      kind: "nested";
      sourceIn: Rational;
      timeline: string;
    };
/**
 * A parameter value: finite numbers round-trip bit for bit through stored JSON (serde_json's
 * `float_roundtrip`). Times, rationals, colours (`#rrggbb[aa]`) and enum values are text; an
 * asset reference is `{"asset": id}`, so every id a value carries is found without the
 * registry (plan 4C Decision 24).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ParamValue".
 */
export type ParamValue =
  | boolean
  | number
  | [number, number]
  | string
  | {
      asset: string;
    };
/**
 * Which edge of an item a trim moves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Edge".
 */
export type Edge = "start" | "end";
/**
 * How a trim treats the rest of the track: `ripple` moves every later item by the change;
 * `roll` moves the cut between the item and the neighbour touching that edge (both change,
 * nothing else moves; `NOT_ADJACENT` without one); `overwrite` moves the edge itself, carving
 * whatever it extends over and leaving a gap when it shrinks; `strict` (internal: the inverse
 * of carving edits) moves the edge only, and an overlap it would make is `CLIP_OVERLAP` in
 * every apply mode.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrimMode".
 */
export type TrimMode = "ripple" | "roll" | "overwrite" | "strict";
/**
 * How a delete leaves its range: `ripple` closes the gap, `lift` leaves it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DeleteMode".
 */
export type DeleteMode = "ripple" | "lift";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Curve".
 */
export type Curve = "linear" | "equalPower";
/**
 * How a transition mixes its two items (sub-project 6 spec §4.3): both mix by
 * `curve(progress)`; they differ in the space they mix in.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TransitionKind".
 */
export type TransitionKind = "crossfade" | "dissolve";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackKind".
 */
export type TrackKind = "video" | "audio" | "caption";
/**
 * One item on a track. On the wire the body's `kind` and fields and the common fields sit
 * next to `id`, `start` and `duration` (both are flattened).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Item".
 */
export type Item = {
  audio?: AudioParams;
  blend?: BlendMode;
  crop?: Crop;
  duration: Rational;
  effects?: Effect[];
  id: string;
  /**
   * Keyframes by registry path, each list sorted by `at`.
   */
  keyframes?: {
    [k: string]: Keyframe[];
  };
  /**
   * The link group (`lk<n>`) the item belongs to.
   */
  link?: string;
  mask?: Mask;
  opacity?: number;
  start: Rational;
  transform?: Transform;
} & Item1;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BlendMode".
 */
export type BlendMode = "normal" | "multiply" | "screen" | "overlay" | "add" | "darken" | "lighten";
/**
 * How a keyframe's value moves to the next one.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Interp".
 */
export type Interp =
  | {
      type: "hold";
    }
  | {
      type: "linear";
    }
  | {
      type: "bezier";
      x1: number;
      x2: number;
      y1: number;
      y2: number;
    };
export type Item1 =
  | {
      asset: string;
      kind: "clip";
      sourceIn: Rational;
      speed?: Rational;
    }
  | {
      kind: "caption";
      overrides?: {
        [k: string]: ParamValue;
      };
      source?: CaptionSource;
      style: string;
      words: Word[];
    }
  | {
      asset: string;
      kind: "graphic";
      slots?: {
        [k: string]: ParamValue;
      };
    }
  | {
      asset: string;
      kind: "image";
    }
  | {
      kind: "adjustment";
    }
  | {
      kind: "nested";
      sourceIn: Rational;
      timeline: string;
    };
/**
 * Where a pinned file comes from: a catalog entry's style (at the catalog version that
 * pinned it), or an imported font asset.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontSource".
 */
export type FontSource =
  | {
      catalog: string;
      entry: string;
      style: string;
      type: "catalog";
    }
  | {
      asset: string;
      type: "asset";
    };
/**
 * An edit intent (content `txIntent`): the transactions as committed (an edit's one tx, or a
 * history call's records, at most [`MAX_INTENT_TXS`]), or the request's normalised tx on
 * failure.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TxIntent".
 */
export type TxIntent = IntentTx[];
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "IntentOmitted".
 */
export type IntentOmitted = "tooLarge";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CommitOutcome".
 */
export type CommitOutcome = "accepted" | "rebased" | "deduped" | "noOp" | "conflict" | "refused" | "dryRun";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CommitRecord".
 */
export type CommitRecord =
  "edit" | "undo" | "redo" | "merge" | "pick" | "revert" | "checkpoint" | "branch" | "branchSet" | "recover";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FetchOutcome".
 */
export type FetchOutcome = "ok" | "failed" | "cached";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StorageMode".
 */
export type StorageMode = "readOnly" | "writable";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SrcLoc".
 */
export type SrcLoc = string;
/**
 * Finisher-authored text with paths, the home, data and temp directories, the user name and the host name replaced by placeholders.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Scrubbed".
 */
export type Scrubbed = string;
/**
 * The thread that panicked (the plan's `thread: Name`, as a closed enum).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PanicThread".
 */
export type PanicThread = "main" | "worker" | "tick" | "writer" | "other";
/**
 * The process that panicked.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PanicWhere".
 */
export type PanicWhere = "finNode" | "finisherd" | "fin";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind = "added" | "removed" | "updated";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntityKind".
 */
export type EntityKind =
  "asset" | "track" | "timeline" | "item" | "transition" | "link" | "marker" | "bin" | "effect" | "style" | "font";
/**
 * What an asset is: `media` (video and/or audio; every pre-4C asset), `image`, `svg`,
 * `lottieTemplate` (a dotLottie template with replaceable slots), `font`, or `lut` (a `.cube`
 * colour lookup table).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AssetKind".
 */
export type AssetKind = "media" | "image" | "svg" | "lottieTemplate" | "font" | "lut";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ArchiveFormat".
 */
export type ArchiveFormat = "zip";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioCodec".
 */
export type AudioCodec = ("aac" | "opus" | "mp3" | "flac" | "alac" | "pcm" | "ac3" | "eac3") | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ShapeKind".
 */
export type ShapeKind = "fadeIn" | "fadeOut" | "crossIn" | "crossOut";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchState".
 */
export type BranchState = "active" | "archived" | "merged";
/**
 * A record's kind and what it points at (spec §4.2). Serialised (in `CommittedTx.link`) as
 * `{"kind": "<kind>", …fields}`, camelCase, absent optional fields omitted.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Link".
 */
export type Link =
  | {
      kind: "edit";
    }
  | {
      kind: "undo";
      undoes: string[];
    }
  | {
      kind: "redo";
      redoes: string;
    }
  | {
      from: string;
      kind: "merge";
      mergeId: string;
      origin: string;
    }
  | {
      from: string;
      kind: "cherryPick";
      origin: string;
      originHash?: string | null;
      recovers?: string | null;
    }
  | {
      kind: "revert";
      reverts: string;
    }
  | {
      kind: "checkpoint";
      name: string;
      recovers?: string | null;
    }
  | {
      branchId: string;
      epoch: number;
      forkHash?: string | null;
      forkSeq: number;
      kind: "branch";
      name: string;
      recovers?: string | null;
    }
  | {
      archived?: boolean | null;
      branchId: string;
      kind: "branchSet";
      name?: string | null;
      recovers?: string | null;
    };
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnmeasuredReason".
 */
export type UnmeasuredReason = "fontMissing" | "fontFetchFailed" | "fontUnpinned" | "noMeasurer";
/**
 * Chroma subsampling.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Chroma".
 */
export type Chroma = "mono" | "yuv420" | "yuv422" | "yuv444";
/**
 * Where 4:2:0 chroma samples sit relative to luma.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChromaSiting".
 */
export type ChromaSiting = "unknown" | "left" | "center" | "topLeft";
/**
 * A project's collection policy (Decision 13): `finisher.project.json` `collect`, read by
 * `fin-store` and fed to the journal. `never` binds every collaborator's server: no entry of
 * the project is recorded, and setting it purges the project's entries. A project whose
 * policy is not known (not open here) is neither: the journal drops its new entries and
 * purges nothing.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CollectPolicy".
 */
export type CollectPolicy = "never" | "default";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Matrix".
 */
export type Matrix = ("bt709" | "bt601" | "unknown") | "bt2020Ncl" | "identity";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Primaries".
 */
export type Primaries = ("bt709" | "bt2020" | "unknown") | "bt601_625" | "bt601_525" | "displayP3";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Range".
 */
export type Range = "full" | "limited";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Transfer".
 */
export type Transfer = ("srgb" | "linear" | "unknown") | "bt709" | "pq" | "hlg";
/**
 * One purpose's switch.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Switch".
 */
export type Switch = "on" | "off";
/**
 * Training's state: `undecided` until a person answers (the onboarding question). It
 * records no content, exactly like `off`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrainingState".
 */
export type TrainingState = "on" | "off" | "undecided";
/**
 * The container family, from `ftyp`'s brands.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ContainerKind".
 */
export type ContainerKind = "mp4" | "mov";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProjectId".
 */
export type ProjectId = string;
/**
 * A spec's default, as its value would be written (`null`: none, the value is per entity or
 * the parameter is optional).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DefaultValue".
 */
export type DefaultValue = null | boolean | number | [number, number] | string;
/**
 * A project's policy as `fin diagnostics show` reports it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProjectPolicy".
 */
export type ProjectPolicy = "default" | "never" | "unknown";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JournalState".
 */
export type JournalState = "off" | "on" | "suspended";
/**
 * A resolved value (plan 4C Decision 12): colours are sRGB `[r, g, b, a]` in 0…1.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanValue".
 */
export type PlanValue =
  | boolean
  | number
  | [number, number]
  | [number, number, number, number]
  | string
  | {
      asset: string;
    };
/**
 * One engine event (`Engine.onEvent`), tagged by `type`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EngineEvent".
 */
export type EngineEvent =
  | {
      tx: CommittedTx;
      type: "applied";
    }
  | {
      status: unknown;
      type: "status";
    }
  | {
      branch: string;
      fromRev: number;
      generation: number;
      incarnation: string;
      own: boolean;
      quarantined: number;
      type: "rewound";
    }
  | {
      incarnation: string;
      type: "rebuilt";
    }
  | {
      error: CoreError;
      fatal: boolean;
      type: "error";
    }
  | {
      dropped: number;
      type: "lagged";
    }
  | {
      collect: CollectPolicy;
      type: "collect";
    };
/**
 * The type of a directory entry as listed (never by following a link). Windows reparse
 * points with a name-surrogate tag (symlinks, junctions, mount points) are `symlink` in a
 * [`ScanEntry`] (see [`crate::listing`]); anything that is neither a regular file, a
 * directory nor a link (sockets, devices, FIFOs) is `other`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntryType".
 */
export type EntryType = "file" | "dir" | "symlink" | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Ulid".
 */
export type Ulid = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "KindName".
 */
export type KindName = string;
/**
 * Which program emitted an entry. The server sets it from the transport (local socket →
 * `fin`, WebSocket → `ui`), never from the payload.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Source".
 */
export type Source = "fin" | "finisherd" | "finNode" | "ui" | "agent";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EventPayload".
 */
export type EventPayload =
  | {
      kind: "session.start";
      kv: 1;
      payload: SessionStart;
    }
  | {
      kind: "session.end";
      kv: 1;
      payload: SessionEnd;
    }
  | {
      kind: "session.unclean";
      kv: 1;
      payload: SessionUnclean;
    }
  | {
      kind: "consent.changed";
      kv: 1;
      payload: ConsentChanged;
    }
  | {
      kind: "journal.dropped";
      kv: 1;
      payload: JournalDropped;
    }
  | {
      kind: "log.commit";
      kv: 1;
      payload: LogCommit;
    }
  | {
      kind: "cli.command";
      kv: 1;
      payload: CliCommand;
    }
  | {
      kind: "rpc.stats";
      kv: 1;
      payload: RpcStats;
    }
  | {
      kind: "eval.stats";
      kv: 1;
      payload: EvalStats;
    }
  | {
      kind: "fonts.fetch";
      kv: 1;
      payload: FontsFetch;
    }
  | {
      kind: "storage.state";
      kv: 1;
      payload: StorageState;
    }
  | {
      kind: "error.internal";
      kv: 1;
      payload: ErrorInternal;
    }
  | {
      kind: "error.panic";
      kv: 1;
      payload: ErrorPanic;
    }
  | {
      kind: "agent.session";
      kv: 0;
      payload: AgentSession;
    }
  | {
      kind: "agent.turn";
      kv: 0;
      payload: AgentTurn;
    }
  | {
      kind: "agent.toolCall";
      kv: 0;
      payload: AgentToolCall;
    }
  | {
      kind: "agent.toolResult";
      kv: 0;
      payload: AgentToolResult;
    }
  | {
      kind: "agent.editVerdict";
      kv: 0;
      payload: AgentEditVerdict;
    }
  | {
      kind: "ui.action";
      kv: 0;
      payload: UiAction;
    }
  | {
      kind: "review.verdict";
      kv: 0;
      payload: ReviewVerdict;
    };
/**
 * A catalog file's state in this server (Decision 34).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FileState".
 */
export type FileState =
  | {
      state: "cached";
    }
  | {
      state: "missing";
    }
  | {
      state: "queued";
    }
  | {
      bytes: number;
      state: "fetching";
      total: number;
    }
  | {
      reason: string;
      retryAtMs: number;
      state: "failed";
    };
/**
 * The item kinds, as they appear in plans.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemKind".
 */
export type ItemKind = "clip" | "caption" | "graphic" | "image" | "adjustment" | "nested";
/**
 * Which side of a transition a layer is.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Role".
 */
export type Role = "out" | "in";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoCodec".
 */
export type VideoCodec = ("av1" | "h264" | "hevc" | "vp9" | "vp8" | "proRes") | "other";
/**
 * Clockwise display rotation in degrees, from the track matrix.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rotation".
 */
export type Rotation = 0 | 90 | 180 | 270;
/**
 * Where a parameter lives. An effect's paths are `<kind>.<param>`; on an item they are
 * addressed as `effects.<fx>.<param>`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Scope".
 */
export type Scope =
  | {
      kinds: string[];
      type: "item";
    }
  | {
      kind: string;
      type: "effect";
    }
  | {
      type: "track";
    }
  | {
      type: "timeline";
    }
  | {
      type: "marker";
    }
  | {
      type: "style";
    };
/**
 * A parameter's value type. Times and rationals are canonical rational text (`"1/2"`),
 * colours `#rrggbb` or `#rrggbbaa` (lowercase), enums their names; a size is whole pixels as
 * a `vec2`; an asset reference is `{"asset": id}` of one of `kinds`. Ranges are inclusive.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ParamType".
 */
export type ParamType =
  | {
      max: number;
      min: number;
      type: "number";
    }
  | {
      type: "bool";
    }
  | {
      type: "enum";
      values: string[];
    }
  | {
      type: "color";
    }
  | {
      /**
       * @minItems 2
       * @maxItems 2
       */
      max: [number, number];
      /**
       * @minItems 2
       * @maxItems 2
       */
      min: [number, number];
      type: "vec2";
    }
  | {
      min?: string | null;
      type: "time";
    }
  | {
      max: string;
      min: string;
      type: "rational";
    }
  | {
      max: number;
      min: number;
      type: "size";
    }
  | {
      maxLen: number;
      type: "text";
    }
  | {
      kinds: string[];
      type: "assetRef";
    };
/**
 * A cloud placeholder (Decision 12): catalogued from metadata only, never opened unless the
 * library has `hydrate`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Placeholder".
 */
export type Placeholder =
  "dataless" | "icloudStub" | "nextcloud" | "owncloud" | "recallOnOpen" | "recallOnData" | "offline" | "unpinned";
/**
 * What to purge (`fin diagnostics purge`, `diagnostics.purge`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PurgeSelector".
 */
export type PurgeSelector =
  | "all"
  | {
      project: string;
    }
  | {
      before: number;
    };
/**
 * A root's state (Decision 17): `online` carries the identity measured now (its key may
 * differ from the stored one when the change was confirmed harmless); every other state
 * puts the library offline with its catalog intact (Decision 18).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RootState".
 */
export type RootState =
  | {
      caseSensitive: boolean;
      devId: string;
      enumeration: string;
      key: string;
      kind: string;
      mountRoot: boolean;
      remote: boolean;
      rootFileId?: string | null;
      sample: SampleEntry[];
      state: "online";
    }
  | {
      code: string;
      state: "unreachable";
    }
  | {
      state: "unmounted";
    }
  | {
      key: string;
      state: "volumeChanged";
    }
  | {
      state: "empty";
    }
  | {
      state: "timeout";
    };
/**
 * Where an undo takes a unit's inverse from ([`UnitEntry::source`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnitSource".
 */
export type UnitSource =
  | {
      kind: "records";
    }
  | {
      kind: "redo";
      txId: string;
    };

export interface FinCore {
  [k: string]: unknown;
}
/**
 * Who caused an entry: a person, an agent (with its session and the tool call that ran
 * `fin`), a plugin or the system. `session` is the agent or UI session (`FIN_SESSION`), not
 * the envelope's process run; `call` is the harness's tool-call id (`FIN_CALL`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Actor".
 */
export interface Actor {
  call?: IdRef;
  kind: ActorKind;
  session?: IdRef;
}
/**
 * `agent.editVerdict`: what became of an agent's edits.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentEditVerdict".
 */
export interface AgentEditVerdict {
  by: VerdictBy;
  txIds: IdRef[];
  verdict: Verdict;
}
/**
 * `agent.session`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentSession".
 */
export interface AgentSession {
  mode: AgentMode;
  model: Name;
  provider: Name;
  role: AgentRole;
}
/**
 * `agent.toolCall`: the agent ran a command (`callId` is the `FIN_CALL` its `fin` run
 * carries).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentToolCall".
 */
export interface AgentToolCall {
  args?: Body;
  argsHash: Hashed;
  callId: IdRef;
  command: Name;
  turnId: IdRef;
}
/**
 * Large content, stored as a blob (Decision 20): its content address, its length and its
 * media type ([`names::MEDIA_TYPES`]). Allowed only inside a [`super::class::Content`] field
 * (`x-fin-blob` marks it for the schema checks and [`super::redact::blob_refs`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BlobRef".
 */
export interface BlobRef {
  bytes: number;
  hash: BlobHash;
  mediaType: Name;
}
/**
 * `agent.toolResult`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentToolResult".
 */
export interface AgentToolResult {
  callId: IdRef;
  code?: Code;
  micros: number;
  ok: boolean;
  output?: Body;
  txIds?: IdRef[];
}
/**
 * `agent.turn`: one model call. Content: the turn's new messages and the output; the system
 * prompt and the tool definitions by hash.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentTurn".
 */
export interface AgentTurn {
  cacheReadTokens: number;
  cacheWriteTokens: number;
  costMicros?: number;
  inputTokens: number;
  latencyMs: number;
  messages?: Body;
  model: Name;
  output?: Body;
  outputTokens: number;
  provider: Name;
  stopReason: StopReason;
  systemHash: Hashed;
  toolsHash: Hashed;
  turnId: IdRef;
}
/**
 * The process run began (the journal opened with a purpose on, or one turned on). Platform
 * facts only (Decision 5): no host, user, IP, serial or hardware model.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionStart".
 */
export interface SessionStart {
  app: Version;
  arch: Arch;
  /**
   * Logical cores.
   */
  cores: number;
  /**
   * Memory rounded to a power-of-two GiB.
   */
  memGiB: number;
  os: OsFamily;
  osMajor: number;
  profile: BuildProfile;
  /**
   * `@finisher/contracts` `PROTOCOL_VERSION`.
   */
  protocol: number;
}
/**
 * The run ended cleanly.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionEnd".
 */
export interface SessionEnd {
  uptimeS: number;
}
/**
 * A start found the previous run still open in `state.json`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionUnclean".
 */
export interface SessionUnclean {
  previous: IdRef;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ConsentChanged".
 */
export interface ConsentChanged {
  basis: Basis;
  from: Purposes;
  /**
   * The new consent revision.
   */
  rev: number;
  to: Purposes;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JournalDropped".
 */
export interface JournalDropped {
  /**
   * The dropped entries' `approx_bytes`.
   */
  bytes: number;
  count: number;
  /**
   * The start of the counting window (ms since the epoch).
   */
  since: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LogCommit".
 */
export interface LogCommit {
  /**
   * The request's `baseRev` (edits only).
   */
  baseRev?: number;
  code?: Code;
  /**
   * Conflict code → count, from `CONFLICT`'s `details.conflicts`.
   */
  conflicts?: {
    [k: string]: number;
  };
  /**
   * Training only: the intent as committed, or as requested on failure.
   */
  intent?: TxIntent;
  intentOmitted?: IntentOmitted;
  /**
   * The txIds this record names (an undo's undone units, a merge's copies, a pick's or
   * revert's target, a recovery's records).
   */
  links?: IdRef[];
  micros: number;
  /**
   * Op kind → count (`OP_KINDS`).
   */
  ops?: {
    [k: string]: number;
  };
  outcome: CommitOutcome;
  /**
   * Committed `baseRev` − request `baseRev`, when `rebased`.
   */
  rebasedOver?: number;
  record: CommitRecord;
}
/**
 * One transaction of an intent. Built only by [`IntentTx::from_tx`] (or decoded, which
 * refuses a raw `asset.add` path); its fields are read through accessors.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "IntentTx".
 */
export interface IntentTx {
  argsHash?: Hashed;
  author: Hashed;
  baseRev: number;
  branch?: BranchId;
  /**
   * The ops, each `asset.add` path a keyed path token ([`PathRef::token`]).
   */
  ops: Op[];
  schemaVersion: number;
  txId: IdRef;
}
/**
 * `asset.add`: what probing found, by kind (plan 4C Task 7). Each kind carries its own
 * fields and none of the others': `media` a positive `duration` and its `video`/`audio`
 * streams; `image` and `svg` an `image`; `lottieTemplate` a positive `duration` and `lottie`;
 * `font` a `font` (with its whole-file SHA-256); `lut` a `lut`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AssetAdd".
 */
export interface AssetAdd {
  audio?: AudioInfo;
  bin?: string;
  duration?: Rational;
  fingerprint: string;
  font?: FontFile;
  id: string;
  image?: ImageInfo;
  /**
   * `media` when absent: a sub-project 4B (schema 1) `asset.add` names no kind.
   */
  kind?: "media" | "image" | "svg" | "lottieTemplate" | "font" | "lut";
  lottie?: LottieInfo;
  lut?: LutInfo;
  path: string;
  video?: VideoInfo;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioInfo".
 */
export interface AudioInfo {
  channels: number;
  sampleRate: number;
}
/**
 * An imported font file (plan 4C Decision 37): its whole-file SHA-256, size, format and faces.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontFile".
 */
export interface FontFile {
  faces: FontFace[];
  format: FontFormat;
  sha256: string;
  size: number;
}
/**
 * One face of a font file, as `fin_text::font_info` reads it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontFace".
 */
export interface FontFace {
  family: string;
  index: number;
  italic: boolean;
  /**
   * ISO 15924 script codes the face covers.
   */
  scripts: string[];
  weight: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ImageInfo".
 */
export interface ImageInfo {
  height: number;
  width: number;
}
/**
 * A dotLottie template: its canvas, timing, replaceable slots and the font families it names.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LottieInfo".
 */
export interface LottieInfo {
  duration: Rational;
  fonts?: string[];
  height: number;
  rate: Rational;
  slots?: SlotSpec[];
  width: number;
}
/**
 * One replaceable slot of a template: its id and value kind (as the template declares it).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SlotSpec".
 */
export interface SlotSpec {
  id: string;
  kind: string;
}
/**
 * A `.cube` LUT: its edge size.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LutInfo".
 */
export interface LutInfo {
  size: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoInfo".
 */
export interface VideoInfo {
  height: number;
  rate: Rational;
  width: number;
}
/**
 * The transcript artifact a caption came from, and its word range (`[first, end)`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionSource".
 */
export interface CaptionSource {
  artifact: string;
  /**
   * @minItems 2
   * @maxItems 2
   */
  wordRange: [number, number];
}
/**
 * One word; `start` and `end` are relative to the item's start (plan 4C Decision 13).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Word".
 */
export interface Word {
  end: Rational;
  start: Rational;
  text: string;
}
/**
 * `item.trim`: moves one edge of `item` by `delta` seconds (positive moves it later).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemTrim".
 */
export interface ItemTrim {
  /**
   * Internal (every trim's inverse): this item alone, not its link group (plan 4C
   * Decision 19). A `strict` trim is always alone.
   */
  alone?: boolean;
  delta: Rational;
  edge: Edge;
  item: string;
  mode: TrimMode;
}
/**
 * `item.delete`: takes `item` off its track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemDelete".
 */
export interface ItemDelete {
  item: string;
  mode: DeleteMode;
}
/**
 * `item.slip`: moves the source of `item` (a clip or nested item) under it by `delta`
 * source seconds (`sourceIn += delta`); its place on the timeline and its item-relative times
 * stay. The source range is policy.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemSlip".
 */
export interface ItemSlip {
  delta: Rational;
  item: string;
}
/**
 * `item.split`: cuts `item` at `offset` (item-relative seconds, strictly inside it). The left
 * part keeps the id and ends at `offset`; the right part is a copy (effects keep their ids,
 * item-relative times move by `−offset`, no link group) named by `id`, a `$name` placeholder.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemSplit".
 */
export interface ItemSplit {
  id: string;
  item: string;
  offset: Rational;
}
/**
 * `item.move`: moves `item` to `track` at `at` (a position in the frame before the op, plan
 * 4C Decision 15), and every member of its link group by the same time on its own track
 * (only the named item changes track). `ripple` closes the gap it leaves and opens room at
 * the target on each track; `overwrite` lifts it and carves the target range.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemMove".
 */
export interface ItemMove {
  at: Rational;
  item: string;
  mode: InsertMode;
  track: string;
}
/**
 * `item.slide`: moves `item` (alone) by `delta` seconds on its track, trimming the
 * neighbour touching each side so nothing else moves: the one before it at its end, the one
 * after it at its start. A side with no touching neighbour needs room in the gap
 * (`SLIDE_BLOCKED`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemSlide".
 */
export interface ItemSlide {
  delta: Rational;
  item: string;
  sides?: SlideSides;
}
/**
 * The neighbours an internal `item.slide` trims; a side without one trims nothing there.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SlideSides".
 */
export interface SlideSides {
  after?: string;
  before?: string;
}
/**
 * `item.link`: makes `items` (at least two, on one timeline, none linked yet) one link group
 * named `id` (a `$name` placeholder). Internal applies (inverses) may name a literal id: a new
 * group (the inverse of `item.unlink`, one member included), or an existing one the items
 * join (the undo of a removal that left a member alone, 4C Tasks 9–10 review).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemLink".
 */
export interface ItemLink {
  id: string;
  items: string[];
}
/**
 * `item.unlink`: dissolves link group `group` (internal, with `items`: only those members
 * leave it).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemUnlink".
 */
export interface ItemUnlink {
  group: string;
  items?: string[];
}
/**
 * `transition.set`: the transition at the cut between `between[0]` (outgoing) and
 * `between[1]` (incoming), touching items of `track`, centred on the cut. `id` is a `$name`
 * placeholder to create one, or the literal id of the one to update (a literal id naming no
 * transition restores it: internal only).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TransitionSet".
 */
export interface TransitionSet {
  /**
   * @minItems 2
   * @maxItems 2
   */
  between: [string, string];
  curve?: Curve;
  duration: Rational;
  id: string;
  kind: TransitionKind;
  track: string;
}
/**
 * `transition.remove`: takes transition `transition` off its track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TransitionRemove".
 */
export interface TransitionRemove {
  transition: string;
}
/**
 * `track.add`: a new empty track of `kind` on `timeline`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackAdd".
 */
export interface TrackAdd {
  id: string;
  index?: number;
  kind: TrackKind;
  name?: string;
  timeline: string;
}
/**
 * `track.remove`: takes `track` off its timeline, with its items when `withItems`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackRemove".
 */
export interface TrackRemove {
  track: string;
  withItems?: boolean;
}
/**
 * `track.move`: puts `track` at `index` of its timeline's tracks.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackMove".
 */
export interface TrackMove {
  index: number;
  track: string;
}
/**
 * `track.set`: sets `path` of `track` to `value`; no `value` resets it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackSet".
 */
export interface TrackSet {
  path: string;
  track: string;
  value?: ParamValue;
}
/**
 * `timeline.add`: a new empty timeline.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TimelineAdd".
 */
export interface TimelineAdd {
  id: string;
  name: string;
  rate: Rational;
  resolution: Resolution;
}
/**
 * Frame size in pixels.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Resolution".
 */
export interface Resolution {
  height: number;
  width: number;
}
/**
 * `timeline.set`: sets `path` of `timeline` to `value`; no `value` resets it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TimelineSet".
 */
export interface TimelineSet {
  path: string;
  timeline: string;
  value?: ParamValue;
}
/**
 * `timeline.remove`: takes `timeline` off the project with its tracks, items and markers.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TimelineRemove".
 */
export interface TimelineRemove {
  timeline: string;
}
/**
 * `marker.add`: a marker on `timeline` at `at`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MarkerAdd".
 */
export interface MarkerAdd {
  at: Rational;
  color?: string;
  duration?: Rational;
  id: string;
  label: string;
  timeline: string;
}
/**
 * `marker.set`: sets `path` of `marker` to `value`; no `value` clears it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MarkerSet".
 */
export interface MarkerSet {
  marker: string;
  path: string;
  value?: ParamValue;
}
/**
 * `marker.remove`: takes `marker` off its timeline.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MarkerRemove".
 */
export interface MarkerRemove {
  marker: string;
}
/**
 * `bin.add`: a bin named `name`, inside `parent` when given.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BinAdd".
 */
export interface BinAdd {
  id: string;
  name: string;
  parent?: string;
}
/**
 * `bin.move`: puts `bin` inside `parent` (at the top when absent).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BinMove".
 */
export interface BinMove {
  bin: string;
  parent?: string;
}
/**
 * `bin.remove`: takes the empty `bin` off the project.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BinRemove".
 */
export interface BinRemove {
  bin: string;
}
/**
 * `asset.move`: puts `asset` in `bin` (at the top when absent).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AssetMove".
 */
export interface AssetMove {
  asset: string;
  bin?: string;
}
/**
 * `track.restore` (internal): puts `track` back on `timeline` at `index`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackRestore".
 */
export interface TrackRestore {
  index: number;
  timeline: string;
  track: Track;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Track".
 */
export interface Track {
  /**
   * Decibels.
   */
  gain?: number;
  id: string;
  /**
   * Sorted by `start`, non-overlapping.
   */
  items: Item[];
  kind: TrackKind;
  lock?: boolean;
  mute?: boolean;
  name: string;
  solo?: boolean;
  transitions?: Transition[];
}
/**
 * The item's own audio: gain in dB, pan in −1…1, fades in seconds.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioParams".
 */
export interface AudioParams {
  fadeIn?: Rational;
  fadeOut?: Rational;
  gain?: number;
  pan?: number;
}
/**
 * Fractions of the item's size cut from each side.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Crop".
 */
export interface Crop {
  bottom?: number;
  left?: number;
  right?: number;
  top?: number;
}
/**
 * One effect of an item (`fx<n>`, addressed as `(item, effect)`); `params` is sparse.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Effect".
 */
export interface Effect {
  enabled?: boolean;
  id: string;
  kind: string;
  params?: {
    [k: string]: ParamValue;
  };
}
/**
 * A keyframe; `at` is relative to the item's start (plan 4C Decision 13).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Keyframe".
 */
export interface Keyframe {
  at: Rational;
  interp: Interp;
  value: ParamValue;
}
/**
 * A matte (an artifact reference) that masks the item.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Mask".
 */
export interface Mask {
  choke?: number;
  feather?: number;
  invert?: boolean;
  matte: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Transform".
 */
export interface Transform {
  /**
   * Item-normalised.
   *
   * @minItems 2
   * @maxItems 2
   */
  anchor?: [number, number];
  /**
   * Pixels from the frame centre.
   *
   * @minItems 2
   * @maxItems 2
   */
  position?: [number, number];
  /**
   * Degrees.
   */
  rotation?: number;
  /**
   * @minItems 2
   * @maxItems 2
   */
  scale?: [number, number];
}
/**
 * A transition at the cut between two touching items of one track, centred on the cut.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Transition".
 */
export interface Transition {
  /**
   * The outgoing item, then the incoming one.
   *
   * @minItems 2
   * @maxItems 2
   */
  between: [string, string];
  curve?: Curve;
  duration: Rational;
  id: string;
  kind: TransitionKind;
}
/**
 * `timeline.restore` (internal): puts `timeline` back at `index` of the project's timelines.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TimelineRestore".
 */
export interface TimelineRestore {
  index: number;
  timeline: Timeline;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Timeline".
 */
export interface Timeline {
  id: string;
  /**
   * Sorted by `at`. Markers never ripple.
   */
  markers?: Marker[];
  name: string;
  rate: Rational;
  resolution: Resolution;
  /**
   * Compositing order, bottom first.
   */
  tracks: Track[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Marker".
 */
export interface Marker {
  at: Rational;
  color?: string;
  duration?: Rational;
  id: string;
  label: string;
}
/**
 * `item.remove` (internal): takes item `id` off its track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemRemove".
 */
export interface ItemRemove {
  id: string;
  mode: DeleteMode;
}
/**
 * `item.restore` (internal): puts `item` back on `track` at its `start`. Its id may be a
 * `$placeholder` (spec §4.3): the item then gets a newly allocated id.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemRestore".
 */
export interface ItemRestore {
  item: Item;
  mode: DeleteMode;
  track: string;
}
/**
 * `param.set`: sets `path` of `item` to `value`; no `value` resets it to its default.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ParamSet".
 */
export interface ParamSet {
  item: string;
  path: string;
  value?: ParamValue;
}
/**
 * `effect.add`: an effect of `kind` on `item` at index `at` (appended when absent).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EffectAdd".
 */
export interface EffectAdd {
  at?: number;
  id: string;
  item: string;
  kind: string;
  params?: {
    [k: string]: ParamValue;
  };
}
/**
 * `effect.remove`: takes `effect` off `item`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EffectRemove".
 */
export interface EffectRemove {
  effect: string;
  item: string;
}
/**
 * `effect.move`: `effect` of `item` to index `to`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EffectMove".
 */
export interface EffectMove {
  effect: string;
  item: string;
  to: number;
}
/**
 * `keyframe.set`: the keyframe of `path` of `item` at `at`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "KeyframeSet".
 */
export interface KeyframeSet {
  at: Rational;
  interp: Interp;
  item: string;
  path: string;
  value: ParamValue;
}
/**
 * `keyframe.remove`: the keyframe of `path` of `item` at `at`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "KeyframeRemove".
 */
export interface KeyframeRemove {
  at: Rational;
  item: string;
  path: string;
}
/**
 * `effect.restore` (internal): `effect` back on `item` at `index`, with its `keyframes`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EffectRestore".
 */
export interface EffectRestore {
  effect: Effect;
  index: number;
  item: string;
  keyframes?: {
    [k: string]: Keyframe[];
  };
}
/**
 * `style.add`: a project style named `name`, from `from`, with `set` applied.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StyleAdd".
 */
export interface StyleAdd {
  from?: string;
  id: string;
  name: string;
  set?: {
    [k: string]: ParamValue;
  };
}
/**
 * `style.set`: sets `path` of `style` to `value`; no `value` resets it to the default
 * style's (or removes an optional part).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StyleSet".
 */
export interface StyleSet {
  path: string;
  style: string;
  value?: ParamValue;
}
/**
 * `style.remove`: takes `style` off the project.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StyleRemove".
 */
export interface StyleRemove {
  style: string;
}
/**
 * `font.pin`: pins `key` to `pin`, at `fallback` in the fallback order if given.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontPinOp".
 */
export interface FontPinOp {
  fallback?: number;
  key: string;
  pin: FontPin;
}
/**
 * One face of one exact file: `face` is the index inside a collection (0 for a single font).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontPin".
 */
export interface FontPin {
  face: number;
  family: string;
  format: FontFormat;
  italic: boolean;
  /**
   * ISO 15924 script codes the face covers (as `fin_text::font_info` reports them).
   */
  scripts: string[];
  /**
   * The whole file's SHA-256, 64 lowercase hex digits.
   */
  sha256: string;
  /**
   * The file's size in bytes.
   */
  size: number;
  source: FontSource;
  weight: number;
}
/**
 * `font.unpin`: drops pin `key`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontUnpin".
 */
export interface FontUnpin {
  /**
   * Internal: drop only the key's fallback place.
   */
  fallbackOnly?: boolean;
  key: string;
}
/**
 * `font.repin`: pins `key` to `pin` instead.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontRepin".
 */
export interface FontRepin {
  key: string;
  pin: FontPin;
}
/**
 * `caption.setText`: `item`'s words become `text`'s.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionSetText".
 */
export interface CaptionSetText {
  item: string;
  text: string;
}
/**
 * `caption.setWords`: `item`'s words become `words`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionSetWords".
 */
export interface CaptionSetWords {
  item: string;
  source?: CaptionSource1;
  words: Word[];
}
/**
 * The transcript artifact a caption came from, and its word range (`[first, end)`).
 */
export interface CaptionSource1 {
  artifact: string;
  /**
   * @minItems 2
   * @maxItems 2
   */
  wordRange: [number, number];
}
/**
 * `caption.split`: cuts `item` before word `word`; the right part is `id`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionSplit".
 */
export interface CaptionSplit {
  id: string;
  item: string;
  word: number;
}
/**
 * `caption.merge`: `item` takes in `with`, the next caption on its track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionMerge".
 */
export interface CaptionMerge {
  item: string;
  with: string;
}
/**
 * `caption.setStyle`: `item` takes `style`; `overrides`, when given, replace its overrides.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionSetStyle".
 */
export interface CaptionSetStyle {
  item: string;
  overrides?: {
    [k: string]: ParamValue;
  };
  style: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CliCommand".
 */
export interface CliCommand {
  argsHash?: Hashed;
  code?: Code;
  dryRun: boolean;
  exit: number;
  /**
   * The flag names parsed, sorted (`CLI_FLAGS`).
   */
  flags?: Name[];
  micros: number;
  name: Name;
  online: boolean;
  txIds?: IdRef[];
}
/**
 * `rpc.stats`: one non-stream RPC method (`RPC_METHODS`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RpcStats".
 */
export interface RpcStats {
  histogram: Histogram;
  key: Name;
}
/**
 * One window's timings for one key. `buckets[i]` counts the samples with
 * `2^i ≤ micros < 2^(i+1)` (`buckets[0]` also counts 0, the last bucket everything above);
 * trailing empty buckets are omitted.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Histogram".
 */
export interface Histogram {
  buckets: number[];
  count: number;
  /**
   * Error code → count.
   */
  errors?: {
    [k: string]: number;
  };
  maxMicros: number;
  /**
   * The window's start (ms since the epoch) and length.
   */
  start: number;
  windowMs: number;
}
/**
 * `eval.stats`: one of `fin-node`'s eval calls (`EVAL_KEYS`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EvalStats".
 */
export interface EvalStats {
  histogram: Histogram;
  key: Name;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontsFetch".
 */
export interface FontsFetch {
  bundled: boolean;
  bytes: number;
  code?: Code;
  entry: Hashed;
  micros: number;
  outcome: FetchOutcome;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StorageState".
 */
export interface StorageState {
  reason: Code;
  state: StorageMode;
}
/**
 * An `INTERNAL` error, or one of the store's unexpected codes, returned through `fin-node`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ErrorInternal".
 */
export interface ErrorInternal {
  call: Name;
  code: Code;
  /**
   * The keys of the error's `details` (`DETAIL_KEYS`), never their values.
   */
  details?: Name[];
}
/**
 * A panic or an uncaught error. Under diagnostics, metadata only: the repo-relative
 * location (or none, with `thirdParty`), the keyed hash of the scrubbed message (so the same
 * crash groups without its text) and the thread. The scrubbed message itself is content.
 * No backtrace frames.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ErrorPanic".
 */
export interface ErrorPanic {
  location?: SrcLoc;
  message?: Scrubbed;
  messageHash: Hashed;
  thirdParty: boolean;
  thread: PanicThread;
  where: PanicWhere;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UiAction".
 */
export interface UiAction {}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ReviewVerdict".
 */
export interface ReviewVerdict {}
/**
 * The result of applying a transaction to a project.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Applied".
 */
export interface Applied {
  /**
   * Placeholder (`$name`) to allocated id.
   */
  bindings: {
    [k: string]: string;
  };
  diff: Diff;
  inverse: Op[];
  project: Project;
}
/**
 * What a transaction changed, op by op: each entry names the op that recorded it (`op`),
 * and both lists are in op order. An op may record any number of changes and shifts.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Diff".
 */
export interface Diff {
  changes: Change[];
  shifts: Shift[];
}
/**
 * One entity an op changed (plan 4C Decision 4). `op`: the index of the op in its transaction
 * that made the change. The optional fields are omitted on the wire when absent (never
 * `null`):
 * - `track`: for an item, the track it is on (after the op; for a removed item, the track it
 *   was taken off), so a footprint and replay's track check see the tracks of ops that record
 *   no shift (a lift, a strict or overwrite trim, a roll, a slip, a split); none for assets;
 * - `edge`: the edge an `updated` change moved, if that is all it did to the entity;
 * - `path`: the parameter an `updated` change set (a registry path), if that is all it did;
 * - `by`: the content offset of a start-edge move: every item-relative time of the entity
 *   moved by it (plan 4C Decision 13).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Change".
 */
export interface Change {
  by?: Rational;
  change: ChangeKind;
  edge?: Edge;
  id: string;
  kind: EntityKind;
  op: number;
  path?: string;
  timeline?: string;
  track?: string;
}
/**
 * Items on `track` at or after `from` moved by `by` (ripple), recorded by op `op` of the
 * transaction.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Shift".
 */
export interface Shift {
  by: Rational;
  from: Rational;
  op: number;
  track: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Project".
 */
export interface Project {
  assets: {
    [k: string]: Asset;
  };
  bins?: {
    [k: string]: Bin;
  };
  fonts?: Fonts;
  id: string;
  name: string;
  rate: Rational;
  schemaVersion: number;
  styles?: {
    [k: string]: CaptionStyle;
  };
  /**
   * The first is the main timeline.
   */
  timelines: Timeline[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Asset".
 */
export interface Asset {
  audio?: AudioInfo;
  bin?: string;
  duration?: Rational;
  fingerprint: string;
  font?: FontFile;
  id: string;
  image?: ImageInfo;
  kind: AssetKind;
  lottie?: LottieInfo;
  lut?: LutInfo;
  path: string;
  video?: VideoInfo;
}
/**
 * A folder of assets; `parent` nests it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Bin".
 */
export interface Bin {
  id: string;
  name: string;
  parent?: string;
}
/**
 * The pins, keyed by [`font_key`], and the fallback order (pin keys, first tried first).
 * Empty until a caption edit pins a face; omitted from the project JSON while empty.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Fonts".
 */
export interface Fonts {
  fallback?: string[];
  pins?: {
    [k: string]: FontPin;
  };
}
/**
 * A caption style: a built-in preset (`preset:<name>`, `fin_core::captions::presets`) or a
 * project style (`st<n>`, made by `style.add`). Lengths are px at the timeline resolution.
 *
 * - `text` names the face (**family, weight, italic**: it resolves to a font pin, Decision
 *   29) and how lines are set;
 * - `stroke`, `background` and `highlight` (the spoken word's colour) are optional;
 * - `position` places the block inside the safe zone `safeZone` (a name from
 *   `fin_core::captions::safe_zones`);
 * - `maxChars` (per line), `maxLines`, `gap` (a pause at least this long starts a new block;
 *   0 never does) and `maxDuration` are the block-splitting rule (`captions::blocks`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionStyle".
 */
export interface CaptionStyle {
  background?: CaptionBox;
  color: string;
  gap: Rational;
  highlight?: string;
  id: string;
  maxChars: number;
  maxDuration?: Rational;
  maxLines: number;
  name: string;
  position: CaptionPosition;
  safeZone: string;
  stroke?: Stroke;
  text: TextStyle;
}
/**
 * A box behind the block. One equal to the default (transparent, no padding) is no box.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionBox".
 */
export interface CaptionBox {
  color: string;
  padding: number;
  radius: number;
}
/**
 * Where the block sits in its safe zone: against the zone's top, middle or bottom, moved
 * towards the middle by `offset` (a fraction of the zone's height, 0 to 1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionPosition".
 */
export interface CaptionPosition {
  anchor: Anchor;
  offset: number;
}
/**
 * An outline around the glyphs. A stroke equal to the default (width 0) is no stroke.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Stroke".
 */
export interface Stroke {
  color: string;
  width: number;
}
/**
 * The style a text is measured with.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TextStyle".
 */
export interface TextStyle {
  align: Align;
  family: string;
  italic: boolean;
  /**
   * Extra advance after every glyph, in em (a multiple of `size`; may be negative).
   */
  letterSpacing: number;
  /**
   * Line advance as a multiple of `size` (e.g. 1.2).
   */
  lineHeight: number;
  /**
   * Font size in px.
   */
  size: number;
  weight: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ArchiveMember".
 */
export interface ArchiveMember {
  format: ArchiveFormat;
  /**
   * The member's path inside the archive.
   */
  member: string;
}
/**
 * One item's part of the range: it plays `asset` from `sourceFrom` at `speed` over the
 * timeline span `[from, to)`. `gain` and `pan` cover that span without gaps, in order. An
 * item of a nested timeline lists the nested items it sounds through in `via`, outermost
 * first (its `item` and `track` are its own, on the nested timeline).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioItem".
 */
export interface AudioItem {
  asset: string;
  from: Rational;
  gain: GainSegment[];
  item: string;
  pan: PanSegment[];
  sourceFrom: Rational;
  speed: Rational;
  to: Rational;
  track: string;
  via?: string[];
}
/**
 * Gain over `[from, to)` in dB: `db` are the end values of the keyframe span the segment
 * lies in, `u` the segment's progress range through that span, so the dB at progress `x` of
 * the segment is `lerp(db[0], db[1], ease(lerp(u[0], u[1], x)))` with `ease` from `interp`
 * (`hold`: `db[0]` throughout). The `shapes` multiply the amplitude.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "GainSegment".
 */
export interface GainSegment {
  /**
   * @minItems 2
   * @maxItems 2
   */
  db: [number, number];
  from: Rational;
  interp: Interp;
  shapes?: Shape[];
  to: Rational;
  /**
   * @minItems 2
   * @maxItems 2
   */
  u: [number, number];
}
/**
 * A fade or crossfade side over a segment: `p` is its progress at the segment's ends (0 → 1
 * across the whole fade). Its amplitude factor is `curve(p)` for `fadeIn`/`crossIn` and
 * `curve(1 − p)` for `fadeOut`/`crossOut` (`linear`: `x`; `equalPower`: `sqrt(x)`-shaped, as
 * the mixer defines it).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Shape".
 */
export interface Shape {
  curve: Curve;
  kind: ShapeKind;
  /**
   * @minItems 2
   * @maxItems 2
   */
  p: [number, number];
}
/**
 * Pan over `[from, to)` (−1 left … 1 right; the mixer clamps), read like [`GainSegment`].
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PanSegment".
 */
export interface PanSegment {
  from: Rational;
  interp: Interp;
  /**
   * @minItems 2
   * @maxItems 2
   */
  pan: [number, number];
  to: Rational;
  /**
   * @minItems 2
   * @maxItems 2
   */
  u: [number, number];
}
/**
 * The items sounding in `[from, to)` of one timeline, in track then time order.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioPlan".
 */
export interface AudioPlan {
  from: Rational;
  items: AudioItem[];
  timeline: string;
  to: Rational;
}
/**
 * The first audio track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AudioTrackInfo".
 */
export interface AudioTrackInfo {
  channels: number;
  codec: AudioCodec;
  fourcc: string;
  /**
   * Samples to drop at the end.
   */
  padding: number;
  /**
   * Samples to drop at the start (encoder delay: edit-list media time, `roll`, Opus
   * pre-skip).
   */
  priming: number;
  sampleRate: number;
}
/**
 * The answer of [`Store::create_branch`](crate::Store::create_branch): the new branch and
 * its `branch` record on the parent. `dry_run`: planned and rolled back, nothing kept (a
 * retry of a committed create answers with that create, `dry_run` false).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchCreated".
 */
export interface BranchCreated {
  branch: BranchView;
  dryRun: boolean;
  record: CommittedTx;
}
/**
 * A branch as listings show it ([`Store::branches`](crate::Store::branches), `branch.list`):
 * `display` is the reference that names it (its name, or `<name>#<id>` for an archived
 * branch or the second holder of a duplicate name); `head` its head seq (its fork seq until
 * its first record).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BranchView".
 */
export interface BranchView {
  createdBy: string;
  display: string;
  forkSeq: number;
  head: number;
  id: string;
  name: string;
  parent?: string | null;
  state: BranchState;
}
/**
 * A transaction as persisted: with its revision, resolved bindings and diff.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CommittedTx".
 */
export interface CommittedTx {
  bindings: {
    [k: string]: string;
  };
  diff: Diff;
  /**
   * The record's kind and links; omitted for an `edit`. Boxed: most txs are edits, and a
   * link would double the size of every `CommittedTx`.
   */
  link?: Link | null;
  rev: number;
  tx: Tx;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Tx".
 */
export interface Tx {
  /**
   * What produced the tx, for a client that sends an idempotency key as `txId`:
   * `blake3:<64 hex>` of the command input (op kinds and canonical args, before they were
   * resolved against a snapshot). A retry rebuilds `ops` against a newer head (positions
   * move; the stored tx is rebased), so `ops` cannot tell a retry from another command;
   * this can. The store refuses a `txId` it holds with another `argsHash`
   * (`IDEM_KEY_REUSED`). Stored with the record, so every server that imported it checks.
   */
  argsHash?: string | null;
  author: string;
  baseRev: number;
  branch: string;
  ops: Op[];
  schemaVersion: number;
  txId: string;
}
/**
 * A caption's words at the plan's time, and its layout when it was measured. `layout` is
 * `null` when it was not, and `unmeasured` says why (plan 4C Decision 36).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionPlan".
 */
export interface CaptionPlan {
  layout: TextLayout | null;
  style: string;
  unmeasured?: Unmeasured;
  /**
   * Every word, in order (indices match the layout's word boxes).
   */
  words: WordPlan[];
}
/**
 * A measured layout, all lengths in 1/64 px. `width` is the widest line, `height` the
 * lines' total advance. `overflow` is set when a line is wider than `max_width` or there are
 * more lines than `max_lines`. `missing` lists the code points no face maps (ascending);
 * `faces` the sha256s whose glyphs the layout uses, in request order.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TextLayout".
 */
export interface TextLayout {
  faces: string[];
  height: number;
  lines: LineBox[];
  missing: number[];
  overflow: boolean;
  width: number;
  words: WordBox[];
}
/**
 * One line. `x` is the left edge of its ink box after alignment, `top` its top; `baseline` is
 * measured from the layout's top. `words` is the half-open range of word indices on it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LineBox".
 */
export interface LineBox {
  baseline: number;
  endWord: number;
  firstWord: number;
  height: number;
  rtl: boolean;
  top: number;
  width: number;
  x: number;
}
/**
 * One word's horizontal extent on its line (visual order is given by `x`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WordBox".
 */
export interface WordBox {
  line: number;
  width: number;
  x: number;
}
/**
 * Why a caption has no layout, and the font keys concerned.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Unmeasured".
 */
export interface Unmeasured {
  keys: string[];
  reason: UnmeasuredReason;
}
/**
 * One word: item-relative times clamped to the item, and whether it is spoken at the plan's
 * time (`start ≤ local < end`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WordPlan".
 */
export interface WordPlan {
  active: boolean;
  end: Rational;
  start: Rational;
  text: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CatalogEntry".
 */
export interface CatalogEntry {
  family: string;
  homepage: string;
  /**
   * `[a-z0-9-]`, unique in the catalog.
   */
  id: string;
  licence: Licence;
  /**
   * The ISO 15924 scripts the entry is offered for (a fallback is chosen by these); each
   * style's default face covers them.
   */
  scripts: string[];
  styles: CatalogStyle[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Licence".
 */
export interface Licence {
  /**
   * An SPDX id from the permissive allow list (`catalog/rules.json`).
   */
  spdx: string;
  /**
   * The licence text at a pinned commit or tag.
   */
  url: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CatalogStyle".
 */
export interface CatalogStyle {
  /**
   * The face a fallback or an entry-level pin uses (a collection holds several).
   */
  defaultFace?: number;
  file: CatalogFile;
  /**
   * `[a-z0-9-]`, unique in its entry (`regular`, `bold`).
   */
  id: string;
  italic: boolean;
  /**
   * Tried in order: the upstream first, then mirrors serving the same bytes.
   */
  sources: CatalogSource[];
  weight: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CatalogFile".
 */
export interface CatalogFile {
  /**
   * Every face, as `fin_text::font_info` reads it (`warm_cache` checks).
   */
  faces: CatalogFace[];
  format: FontFormat;
  /**
   * Lowercase hex SHA-256 of the whole file: its cache name and its pin.
   */
  sha256: string;
  /**
   * Bytes; a download must match it exactly.
   */
  size: number;
  /**
   * A variable font's `wght` axis. Only a variable file may serve several styles (one per
   * weight in the range; the request's weight drives the axis).
   */
  variable?: Variable | null;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CatalogFace".
 */
export interface CatalogFace {
  family: string;
  index: number;
  italic: boolean;
  /**
   * ISO 15924 coverage by `fin_text::font_info`'s rule, sorted.
   */
  scripts: string[];
  weight: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Variable".
 */
export interface Variable {
  /**
   * The `fvar` `wght` axis' `[min, max]` (`warm_cache` checks).
   *
   * @minItems 2
   * @maxItems 2
   */
  wght: [number, number];
}
/**
 * Where a file can be fetched. A plain source serves the file itself; an archive source
 * serves an archive (its own `sha256` and `size`) holding the file as `archive.member`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CatalogSource".
 */
export interface CatalogSource {
  archive?: ArchiveMember | null;
  sha256?: string | null;
  size?: number | null;
  url: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ColourTags".
 */
export interface ColourTags {
  matrix: Matrix;
  primaries: Primaries;
  range: Range;
  siting: ChromaSiting;
  transfer: Transfer;
}
/**
 * One op that cannot be carried to the head: which op of which tx, why (a reason code), the
 * record to blame when there is one (else null), and what to do. A re-run failure keeps its
 * error's `details` (an `INVALID_RANGE`'s `min` and `max`, say); other conflicts have none.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Conflict".
 */
export interface Conflict {
  againstTx?: string | null;
  /**
   * Boxed: rarely present, and it keeps `Result<_, Conflict>` small.
   */
  details?: {
    [k: string]: unknown;
  };
  fix: string;
  opIndex: number;
  reason: string;
  txId: string;
}
/**
 * The consent an entry was written under (Decisions 12 and 14): the consent revision, the
 * purposes in force (recorded ∩ current), and the weaker basis of the two purposes.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ConsentStamp".
 */
export interface ConsentStamp {
  basis: Basis;
  purposes: Purposes;
  rev: number;
}
/**
 * The effective consent as `fin diagnostics show` and `diagnostics.consent` report it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ConsentState".
 */
export interface ConsentState {
  basis: Basis;
  diagnostics: Switch;
  doNotTrack: boolean;
  env: Purposes;
  /**
   * The consent revision: monotonic, persisted in `events/state.json`.
   */
  rev: number;
  training: TrainingState;
}
/**
 * CTA-861.3 content light level (`clli`), in cd/m².
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ContentLight".
 */
export interface ContentLight {
  maxCll: number;
  maxFall: number;
}
/**
 * A structured, machine-readable error. `code` is a stable `SCREAMING_SNAKE` string.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CoreError".
 */
export interface CoreError {
  code: string;
  details: unknown;
  fix?: string | null;
  message: string;
}
/**
 * What an entry is about: the project, branch and transaction it joins the op log on, the
 * author (keyed-hashed: `FIN_AUTHOR` is free text) and the actor.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Ctx".
 */
export interface Ctx {
  actor?: Actor;
  author?: Hashed;
  branch?: BranchId;
  project?: ProjectId;
  txId?: IdRef;
}
/**
 * `diagnostics.status` (`fin diagnostics show`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DiagnosticsReport".
 */
export interface DiagnosticsReport {
  blobBytes: number;
  /**
   * Blobs on disk and their bytes (they count against `maxMiB` too).
   */
  blobs: number;
  /**
   * The `j-*` files' bytes (blobs not included).
   */
  bytes: number;
  code?: string;
  consent: ConsentState;
  /**
   * Entries dropped since the journal opened.
   */
  dropped: number;
  files: number;
  /**
   * Entries stored per kind.
   */
  kinds: {
    [k: string]: number;
  };
  newest?: number;
  /**
   * The oldest entry's `ts` and the newest's (ms since the epoch).
   */
  oldest?: number;
  /**
   * Every project this server was told about, with its flag now.
   */
  projects: {
    [k: string]: ProjectPolicy;
  };
  /**
   * Entries not written since the journal opened because their project's flag was not
   * known (not open here): not drops, but a sign a caller forgot to open a project.
   */
  skippedUnknownProject?: number;
  state: JournalState;
}
/**
 * Closes one listing of `dir` (Decision 25): every entry of that listing precedes it in the
 * job's batches. `complete: false` (an error, a timeout, a placeholder or automount
 * directory) marks nothing missing; `code` says why. `entries` counts the listing's entries.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DirDone".
 */
export interface DirDone {
  code?: string | null;
  complete: boolean;
  dir: RawRel;
  entries: number;
  mtimeNs?: string | null;
}
/**
 * Raw components below a library root, outermost first (empty for the root): base64 of each name's bytes (Unix bytes; Windows UTF-16 as WTF-8).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RawRel".
 */
export interface RawRel {
  components: string[];
}
/**
 * A Dolby Vision configuration record (`dvcC`/`dvvC`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DolbyVision".
 */
export interface DolbyVision {
  baseLayer: boolean;
  /**
   * The base layer's signal compatibility id (8.1 HDR10, 8.4 HLG, …).
   */
  compatibility: number;
  enhancementLayer: boolean;
  level: number;
  profile: number;
  rpu: boolean;
}
/**
 * One enabled effect with its parameters resolved at the plan's time. `params` holds the
 * effect's stored and keyframed parameters (the registry's defaults fill the rest).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EffectPlan".
 */
export interface EffectPlan {
  id: string;
  kind: string;
  params?: {
    [k: string]: PlanValue;
  };
}
/**
 * One journal entry (Decision 2), as read from disk: `payload` is a `Value` (whose schema
 * is [`AnyPayload`]'s). In memory a payload is an [`AnyPayload`], and the journal's writer
 * thread serialises an [`EnvelopeRef`], which has the same JSON.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Envelope".
 */
export interface Envelope {
  consent: ConsentStamp;
  ctx?: Ctx;
  id: Ulid;
  kind: KindName;
  /**
   * The kind's payload version.
   */
  kv: number;
  payload: AnyPayload;
  session: Ulid;
  source: Source;
  /**
   * Milliseconds since the Unix epoch, UTC.
   */
  ts: number;
  /**
   * [`ENVELOPE_VERSION`].
   */
  v: number;
}
/**
 * What an open must find (Decision 19 step 3): the catalogued size, mtime and, when the
 * catalog has one, file id. TS `ExpectWire`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Expect".
 */
export interface Expect {
  fileId?: string | null;
  mtimeNs: string;
  size: number;
}
/**
 * One page of `diagnostics.export`, oldest first. `next` is an opaque position: pass it as
 * the next request's `cursor`; absent on the last page.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ExportPage".
 */
export interface ExportPage {
  lines: Envelope[];
  next?: string;
}
/**
 * Which entries to read. Empty fields match everything.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Filter".
 */
export interface Filter {
  /**
   * Entries of these kinds.
   */
  kinds?: string[];
  /**
   * Only the newest `limit` matches (still oldest first).
   */
  limit?: number;
  /**
   * Entries whose `ctx.project` is this id (in any case).
   */
  project?: string;
  /**
   * Entries with `ts` ≥ this (ms since the epoch).
   */
  since?: number;
  /**
   * Entries with `ts` < this.
   */
  until?: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontCatalog".
 */
export interface FontCatalog {
  /**
   * `YYYY.MM.N`, e.g. `2026.11.1`.
   */
  catalogVersion: string;
  entries: CatalogEntry[];
  /**
   * Entry ids, in the order a text's uncovered scripts look for a fallback.
   */
  fallback: string[];
}
/**
 * One font file. `entry` and `style` name its catalog style, when the catalog lists it;
 * `keys` are the font keys (`<family>|<weight>|<normal|italic>`) open projects pin it as.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontFileStatus".
 */
export interface FontFileStatus {
  entry?: string;
  family: string;
  italic: boolean;
  keys: string[];
  sha256: string;
  size: number;
  state: FileState;
  style?: string;
  weight: number;
}
/**
 * `fonts.plan`: the `font.pin` ops a client prepends to its tx, and notes about faces or
 * scripts the catalog cannot pin (`FONT_NOT_IN_CATALOG`), which never fail the plan.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontsPlan".
 */
export interface FontsPlan {
  notes: CoreError[];
  ops: Op[];
}
/**
 * `fonts.status`: every catalog file's state in this server, then the files pinned by open
 * projects that the catalog does not list (font assets, files of another catalog).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontsStatus".
 */
export interface FontsStatus {
  /**
   * Background fetching is on (`fonts.autoFetch`, and `FIN_FONTS_OFFLINE` unset).
   */
  autoFetch: boolean;
  catalogVersion: string;
  /**
   * The font cache or the catalog could not be used (`FONTS_DAMAGED`): nothing is
   * fetched, and only font assets measure.
   */
  damaged: {
    code: string;
    details: unknown;
    fix?: string | null;
    message: string;
  };
  files: FontFileStatus[];
}
/**
 * The layers of one timeline at `t`, bottom first.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FramePlan".
 */
export interface FramePlan {
  /**
   * Bottom first: tracks in compositing order, items in time order within a track (a
   * transition's outgoing item before its incoming one).
   */
  layers: Layer[];
  resolution: Resolution;
  t: Rational;
  /**
   * The text engine (`TextMeasurer::fingerprint`) that produced the plan's caption layouts;
   * absent when no caption was measured.
   */
  textEngine?: string;
  timeline: string;
}
/**
 * One item drawn at the plan's time. `local` is the time from the item's start: negative for
 * a transition's incoming item before its cut, past `duration` for the outgoing one after it.
 * An adjustment layer carries effects only (its transform and crop are the defaults); a
 * nested item is a group layer whose `children` are its timeline's layers.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Layer".
 */
export interface Layer {
  blend: BlendMode;
  caption?: CaptionPlan;
  /**
   * A nested item's layers, bottom first.
   */
  children?: Layer[];
  crop: PlanCrop;
  /**
   * Enabled effects, in order.
   */
  effects?: EffectPlan[];
  graphic?: GraphicPlan;
  item: string;
  kind: ItemKind;
  local: Rational;
  mask?: MaskPlan;
  nested?: NestedPlan;
  opacity: number;
  source?: SourceRef;
  track: string;
  transform: PlanTransform;
  transition?: TransitionState;
}
/**
 * A layer's crop, resolved at the plan's time: fractions of the item's size cut from each
 * side, each in `[0, 1]`, every field present.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanCrop".
 */
export interface PlanCrop {
  bottom: number;
  left: number;
  right: number;
  top: number;
}
/**
 * A graphic's template, its time (from the item's start) and its slot values.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "GraphicPlan".
 */
export interface GraphicPlan {
  asset: string;
  slots?: {
    [k: string]: PlanValue;
  };
  time: Rational;
}
/**
 * The mask with its values resolved (`feather` and `choke` at least 0).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MaskPlan".
 */
export interface MaskPlan {
  choke: number;
  feather: number;
  invert: boolean;
  matte: string;
}
/**
 * A nested item's timeline, time and frame size.
 */
export interface NestedPlan {
  resolution: Resolution;
  time: Rational;
  timeline: string;
}
/**
 * The asset and its time: a clip's source time (through speed), an image's 0, a
 * graphic's template time.
 */
export interface SourceRef {
  asset: string;
  time: Rational;
}
/**
 * A layer's transform, resolved at the plan's time: every field is present (the model's
 * `Transform` leaves defaults out).
 *
 * **The base box** (sub-project 6 spec §5.1, owner question 4: "contain"): a source's display
 * size (after its rotation and pixel aspect) fitted inside the timeline frame and centred.
 * The item is that box with item-normalised coordinates 0…1; its `anchor` point is placed at
 * the frame centre plus `position`, and the item is scaled by `scale` and rotated `rotation`
 * degrees clockwise about it. The defaults leave the base box where it is. `crop` cuts
 * fractions of the base box from each side without moving the rest.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanTransform".
 */
export interface PlanTransform {
  /**
   * Item-normalised.
   *
   * @minItems 2
   * @maxItems 2
   */
  anchor: [number, number];
  /**
   * Pixels from the frame centre.
   *
   * @minItems 2
   * @maxItems 2
   */
  position: [number, number];
  /**
   * Degrees.
   */
  rotation: number;
  /**
   * @minItems 2
   * @maxItems 2
   */
  scale: [number, number];
}
/**
 * A layer inside a transition window `[cut − d/2, cut + d/2)`: `progress` runs 0 → 1 over it.
 *
 * **Looks** (sub-project 6 spec §4.3, §5.1): the outgoing layer is drawn as it is; the incoming
 * one is drawn over it and mixed with what is below by `curve(progress)` (`linear`: the
 * progress; `equalPower`: its square root). `crossfade` mixes display-encoded values,
 * `dissolve` linear light.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TransitionState".
 */
export interface TransitionState {
  curve: Curve;
  id: string;
  kind: TransitionKind;
  progress: number;
  role: Role;
}
/**
 * A branch generation as a caller holds it: the database's incarnation and the branch's
 * rewind count in it (module docs).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Generation".
 */
export interface Generation {
  incarnation: string;
  n: number;
}
/**
 * HDR metadata a source carries (kept for HDR export later, spec §10.1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HdrInfo".
 */
export interface HdrInfo {
  contentLight?: ContentLight;
  dolbyVision?: DolbyVision;
  mastering?: MasteringDisplay;
}
/**
 * SMPTE ST 2086 mastering display colour volume (`mdcv`), in the box's own units:
 * chromaticities in 0.00002, luminance in 0.0001 cd/m².
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MasteringDisplay".
 */
export interface MasteringDisplay {
  maxLuminance: number;
  minLuminance: number;
  /**
   * `[x, y]` for green, blue, red (the box's order).
   *
   * @minItems 3
   * @maxItems 3
   */
  primaries: [[number, number], [number, number], [number, number]];
  /**
   * @minItems 2
   * @maxItems 2
   */
  whitePoint: [number, number];
}
/**
 * The answer of a history operation ([`Store::undo`](crate::Store::undo),
 * [`Store::redo`](crate::Store::redo)): the records it committed, in order (or planned and
 * rolled back, `dry_run`; a retry of a committed operation answers with its records,
 * `dry_run` false), and what they act on: the units an undo undoes, the undo a redo
 * reverses, or the merge group a revert reverts. `no_op`: why a cherry-pick or revert
 * committed nothing (`ALREADY_PRESENT`: the target holds the record's origin root;
 * `ALREADY_REVERTED`: the merge group's effect is gone there); absent otherwise.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HistoryResult".
 */
export interface HistoryResult {
  committed: CommittedTx[];
  dryRun: boolean;
  noOp?: string | null;
  undone: string[];
}
/**
 * A track `fin-container` does not interpret (a second video or audio track, timecode, …).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "IgnoredTrack".
 */
export interface IgnoredTrack {
  /**
   * The `hdlr` handler type (`vide`, `soun`, `tmcd`, …).
   */
  handler: string;
  id: number;
}
/**
 * What the journal holds and whether it is writing (`fin diagnostics show`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JournalStatus".
 */
export interface JournalStatus {
  blobBytes: number;
  /**
   * Blobs on disk and their bytes (they count against `maxMiB` too).
   */
  blobs: number;
  /**
   * The `j-*` files' bytes (blobs not included).
   */
  bytes: number;
  code?: string;
  /**
   * Entries dropped since the journal opened.
   */
  dropped: number;
  files: number;
  newest?: number;
  /**
   * The oldest entry's `ts` and the newest's (ms since the epoch).
   */
  oldest?: number;
  /**
   * Entries not written since the journal opened because their project's flag was not
   * known (not open here): not drops, but a sign a caller forgot to open a project.
   */
  skippedUnknownProject?: number;
  state: JournalState;
}
/**
 * One directory the catalog knows (Decision 27), one line of a scan job's `in/dirs.ndjson`
 * (written by the indexer, read by the walker, [`crate::walk::KnownDirs`]):
 *
 * - `mtime_ns`: the mtime its last listing recorded (`DirDone.mtime_ns`); a poll re-lists the
 *   directory when it differs;
 * - `entries`: how many entries that listing held: a listing that drops from N > 0 to 0 is
 *   `DIR_EMPTY_SUSPECT` (Decision 25), the root's is `ROOT_EMPTY_SUSPECT` (Decision 18);
 * - `listed_gen`: the full-scan generation in which it was last listed completely; a
 *   resumed full scan of that generation skips its listing and still descends;
 * - `mount_root`: it was the root of a nested mount; when it no longer is, its subtree is
 *   `ROOT_UNMOUNTED` (Decision 17), never listed and never missing.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "KnownDir".
 */
export interface KnownDir {
  dir: RawRel;
  entries?: number;
  listedGen?: number | null;
  mountRoot?: boolean;
  mtimeNs?: string | null;
}
/**
 * What a media file is (spec §6.3).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MediaInfo".
 */
export interface MediaInfo {
  audio?: AudioTrackInfo;
  container: ContainerKind;
  /**
   * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
   */
  duration: string;
  fragmented?: boolean;
  ignored?: IgnoredTrack[];
  video?: VideoTrackInfo;
}
/**
 * The first video track.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoTrackInfo".
 */
export interface VideoTrackInfo {
  bitDepth: number;
  chroma: Chroma;
  codec: VideoCodec;
  coded: Size;
  colour: ColourTags;
  display: Size1;
  /**
   * The sample entry type (`av01`, `hvc1`, …).
   */
  fourcc: string;
  /**
   * Frames presented, after edit lists.
   */
  frames: number;
  hdr?: HdrInfo;
  /**
   * An ICC profile was given instead of code points (`colr` `prof`/`rICC`).
   */
  icc?: boolean;
  level?: number;
  /**
   * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
   */
  nominalRate: string;
  /**
   * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
   */
  pixelAspect: string;
  profile?: number;
  rotation: Rotation;
  /**
   * Frame durations differ by more than 0.1 % (the last frame ignored).
   */
  vfr: boolean;
}
/**
 * The sample entry's coded size.
 */
export interface Size {
  height: number;
  width: number;
}
/**
 * `coded` after the pixel aspect and the rotation ([`display_size`]).
 */
export interface Size1 {
  height: number;
  width: number;
}
/**
 * The answer of [`Store::merge`](crate::Store::merge): merge group `merge_id`'s records on
 * the target, in order (or planned and rolled back, `dry_run`); `merged`: the source txIds
 * they copy; `skipped`: the candidates that conflict and were left on the source (with
 * `skipConflicting`; they are offered again by the next merge); `excluded`: source records
 * never offered (`UNDOES_TARGET_RECORDS`); `source_state`: the source branch's state after
 * the merge. A retry of a committed merge answers from the log: its records and their
 * origins, `skipped` and `excluded` empty, `dry_run` false. Nothing left to merge is an
 * empty answer, not an error.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MergeReport".
 */
export interface MergeReport {
  committed: CommittedTx[];
  dryRun: boolean;
  excluded: Conflict[];
  mergeId: string;
  merged: string[];
  skipped: Conflict[];
  sourceState: BranchState;
}
/**
 * The nested timeline of a group layer, the time evaluated on it (`sourceIn + local`) and
 * its frame size.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "NestedPlan".
 */
export interface NestedPlan1 {
  resolution: Resolution;
  time: Rational;
  timeline: string;
}
/**
 * One parameter: its path in its scope, type, whether keyframes may animate it, default, unit
 * and fix text (`{min}`, `{max}`, `{unit}` and `{path}` are filled in errors).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ParamSpec".
 */
export interface ParamSpec {
  animatable: boolean;
  default: DefaultValue;
  fix: string;
  path: string;
  scope: Scope;
  type: ParamType;
  unit?: string | null;
}
/**
 * `settings.json` `privacy` (Decision 8). Absent keys fall through to the defaults; unknown
 * keys are ignored.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PrivacySettings".
 */
export interface PrivacySettings {
  diagnostics?: Switch | null;
  /**
   * The size cap in MiB over the files and blobs, 16–4096 (default 128).
   */
  maxMiB?: number | null;
  /**
   * The age cap in days, 1–365 (default 30).
   */
  retentionDays?: number | null;
  training?: Switch | null;
}
/**
 * What the server needs to register an asset: its kind and that kind's fields (the
 * `asset.add` fields, plan 4C Task 7). Its serde form is what `asset.probe` answers
 * (`@finisher/core-types` exports it as `Probe`); absent fields are omitted.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Probe".
 */
export interface Probe {
  audio?: AudioInfo | null;
  /**
   * Seconds: `media` and `lottieTemplate` only.
   */
  duration?: Rational | null;
  fingerprint: string;
  /**
   * A font's whole-file SHA-256, format and faces (Decision 37).
   */
  font?: FontFile | null;
  /**
   * `image` and `svg`: the pixel size.
   */
  image?: ImageInfo | null;
  kind: AssetKind;
  lottie?: LottieInfo | null;
  lut?: LutInfo | null;
  media?: MediaInfo1;
  size: number;
  video?: VideoInfo | null;
}
/**
 * MP4/MOV: what the file is (sub-project 6 spec §6.3): display size and rotation, colour,
 * HDR, VFR, edit lists. In the `asset.probe` answer only: an `asset.add` never carries it
 * (spec §4.4).
 */
export interface MediaInfo1 {
  audio?: AudioTrackInfo;
  container: ContainerKind;
  /**
   * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
   */
  duration: string;
  fragmented?: boolean;
  ignored?: IgnoredTrack[];
  video?: VideoTrackInfo;
}
/**
 * What a purge removed.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PurgeReport".
 */
export interface PurgeReport {
  blobs: number;
  /**
   * Files deleted or rewritten.
   */
  files: number;
  /**
   * Entries.
   */
  removed: number;
}
/**
 * One group of quarantined records ([`Store::recover_groups`](crate::Store::recover_groups)).
 *
 * - `id`: the first record's `txHash`, 12 hex digits (stable on every server);
 * - `records_branch`: the branch the records were on;
 * - `branch`, `fork_seq`, `fork_hash`: the fork `create` uses: an existing branch, the seq
 *   and its record; or, with `fork_group`, the branch that group makes (`branch` its name,
 *   `fork_hash` absent: its record is written then); none for a group that cannot be made
 *   for want of a fork (`NO_COMMON_HISTORY`);
 * - `records`, `reasons`: how many, and per quarantine reason; `origin`: the first row's;
 * - `server`: the writer's server id (`unknown` when it cannot be told);
 * - `first_committed_at`: unix ms; `name`: the branch `create` makes (empty for a group
 *   that makes none: it has neither records with ops nor checkpoints, and its child groups
 *   fork where it does);
 * - `children`: the groups forking from it, or made by its `branch` records;
 * - `recoverable`, `problems`: why `create` leaves it whole (fix each, or dismiss it);
 * - `notes`: rows `create` leaves in quarantine while making the rest (`NAME_TAKEN`,
 *   `BRANCH_NOT_FOUND`, …; they come back as a group of their own), and copies whose source
 *   is not here (`SOURCE_NOT_FOUND`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RecoverGroup".
 */
export interface RecoverGroup {
  branch: string;
  children: string[];
  firstCommittedAt: number;
  forkGroup?: string | null;
  forkHash?: string | null;
  forkSeq: number;
  id: string;
  name: string;
  notes: Conflict[];
  origin: string;
  problems: Conflict[];
  reasons: {
    [k: string]: number;
  };
  records: number;
  recordsBranch: string;
  recoverable: boolean;
  server: string;
}
/**
 * [`Store::recover`](crate::Store::recover)'s answer: the groups as found (before `create`),
 * the branches made and every record written (`branch` records on the parents, the copies,
 * checkpoints, `branchSet` records), in order, for publishing; `imported`: quarantine rows
 * `import_dir` added or made current again, and `skipped_files` the files it did not read;
 * `dismissed`: rows `dismiss` set aside from listing and counting; `manual`: the procedure
 * for a read-only state recovery cannot lift by itself (`CHAIN_FORK`, `CONFLICT_COPY`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RecoverReport".
 */
export interface RecoverReport {
  committed: CommittedTx[];
  created: BranchView[];
  dismissed: number;
  dryRun: boolean;
  groups: RecoverGroup[];
  imported: number;
  manual?: string | null;
  skippedFiles: SkippedFile[];
}
/**
 * A file `import_dir` did not read: its path and why (`not a segment: …`, `another
 * project`, `unreadable: …`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SkippedFile".
 */
export interface SkippedFile {
  path: string;
  reason: string;
}
/**
 * One top-level entry of a root, kept to confirm a volume change (Decision 17). No name:
 * entries are matched by file id, size and mtime.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SampleEntry".
 */
export interface SampleEntry {
  fileId?: string | null;
  mtimeNs: string;
  size: number;
}
/**
 * One batch file of a scan (Decision 26): up to 4,096 entries, in `seq` order per job.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ScanBatch".
 */
export interface ScanBatch {
  dirsDone: DirDone[];
  entries: ScanEntry[];
  seq: number;
}
/**
 * One entry of a directory listing, as a scan reports it (Decision 25 keys rows by
 * `(dir, name)` on raw bytes).
 *
 * - `name`: the raw component the entry is opened by; for a Nextcloud, ownCloud or iCloud
 *   stub it is the name the stub **stands for** (`placeholder` says which stub), so the row
 *   survives hydration; a stub whose real file is also listed is `excluded` with
 *   `STUB_SHADOWED` under its own name.
 * - `name_nfc`, `name_lossy`: the NFC display name ([`crate::names`]); `rel_key`: the
 *   lookup key, `/`-joined and case-folded on case-insensitive roots. Neither is unique.
 * - `dev`, `file_id`: rename hints only (`dev:fileId`, Decision 22); `None` where the OS or
 *   server gives none.
 * - `mount_root`: the entry is the root of a nested mount; `automount`: an automount point,
 *   never descended (`excluded: AUTOMOUNT`).
 * - `excluded`: the exclusion code ([`crate::exclude::code`]) or `None`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ScanEntry".
 */
export interface ScanEntry {
  automount: boolean;
  dev?: string | null;
  dir: RawRel;
  entryType: EntryType;
  excluded?: string | null;
  fileId?: string | null;
  mountRoot: boolean;
  mtimeNs: string;
  name: string;
  nameLossy: boolean;
  nameNfc: string;
  placeholder?: Placeholder | null;
  relKey: string;
  reparseTag?: number | null;
  size: number;
}
/**
 * A record kept in quarantine ([`TxStatus::SetAside`](crate::TxStatus)). `reason`:
 * `SUPERSEDED`, `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`,
 * `MALFORMED`, `CONFLICT_COPY`, `ID_COLLISION`, `IMPORTED`; `origin`: `local`, `folder`,
 * `foreign`, `salvage`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SetAside".
 */
export interface SetAside {
  branch: string;
  origin: string;
  reason: string;
  txId: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Size".
 */
export interface Size2 {
  height: number;
  width: number;
}
/**
 * An asset and the time to read from it, in seconds.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SourceRef".
 */
export interface SourceRef1 {
  asset: string;
  time: Rational;
}
/**
 * An author's undo and redo stacks on a branch, bottom first (the top is the last entry).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Stacks".
 */
export interface Stacks {
  redo: UnitEntry[];
  undo: UnitEntry[];
}
/**
 * One entry of an undo or redo stack.
 *
 * - `id`: what an `undo` names in `undoes` (a record's txId, a merge group's `mergeId`, the
 *   redo record's txId for a redo unit), or, on the redo stack, the undo record's txId;
 * - `records`: the txIds of its records, oldest first: whose stored `inverse` an undo (or,
 *   for an undo record, a redo) takes;
 * - `last_seq`: the seq of its last record, the frame its inverses apply in.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnitEntry".
 */
export interface UnitEntry {
  id: string;
  lastSeq: number;
  records: string[];
  /**
   * The record whose stored `inverse` an undo uses: the unit's records, or the redo
   * record that recreated it (Decision 23).
   */
  source:
    | {
        kind: "records";
      }
    | {
        kind: "redo";
        txId: string;
      };
}
/**
 * `tx.get` for a transaction that is not (or no longer) on its branch.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TxSetAside".
 */
export interface TxSetAside {
  setAside: SetAside;
}
/**
 * A root's volume (Decision 15), measured at add and refreshed at each full scan.
 *
 * - `key`: BLAKE3-16 (hex) of the OS's identity fields ([`crate::volume::key`]); the raw
 *   fields are never stored;
 * - `kind`: the filesystem type (`apfs`, `smbfs`, `ext4`, `nfs4`, `NTFS`, …);
 * - `dev_id`: the device id, informational only (never compared alone);
 * - `mount_root`: the root is a mount root (Decision 17's unmount rule);
 * - `enumeration`: how directories are read (`getdents64`, `getattrlistbulk`, a Windows
 *   information class, `fake`);
 * - `root_file_id`, `sample`: the root's file id and up to 16 top-level entries, to confirm a
 *   key change.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VolumeIdentity".
 */
export interface VolumeIdentity {
  caseSensitive: boolean;
  devId: string;
  enumeration: string;
  key: string;
  kind: string;
  mountRoot: boolean;
  remote: boolean;
  rootFileId?: string | null;
  sample: SampleEntry[];
}
