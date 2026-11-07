/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * Exact rational seconds in canonical form: "n/d" in lowest terms with d > 1, or "n" when d is 1; no leading zeros, no "+", no "-0".
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rational".
 */
export type Rational = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind = "added" | "removed" | "updated";
/**
 * Which edge of an item a trim moves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Edge".
 */
export type Edge = "start" | "end";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntityKind".
 */
export type EntityKind = "asset" | "track" | "timeline" | "item" | "transition" | "effect";
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
 * What an asset is: `media` (video and/or audio; every pre-4C asset), `image`, `svg`,
 * `lottieTemplate` (a dotLottie template with replaceable slots), `font`, or `lut` (a `.cube`
 * colour lookup table).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AssetKind".
 */
export type AssetKind = "media" | "image" | "svg" | "lottieTemplate" | "font" | "lut";
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackKind".
 */
export type TrackKind = "video" | "audio" | "caption";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Curve".
 */
export type Curve = "linear" | "equalPower";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TransitionKind".
 */
export type TransitionKind = "crossfade" | "dissolve";
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
 * A spec's default, as its value would be written (`null`: none, the value is per entity or
 * the parameter is optional).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DefaultValue".
 */
export type DefaultValue = null | boolean | number | [number, number] | string;
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
    };
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
 * A project caption style (`st<n>`). Its fields arrive with captions (plan 4C Task 18); until
 * then the project holds none.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionStyle".
 */
export interface CaptionStyle {}
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
 * What the server needs to register a media asset. Its serde form is what `asset.probe`
 * answers (`@finisher/core-types` exports it as `Probe`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Probe".
 */
export interface Probe {
  audio?: AudioInfo | null;
  duration: Rational;
  fingerprint: string;
  size: number;
  video?: VideoInfo | null;
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
