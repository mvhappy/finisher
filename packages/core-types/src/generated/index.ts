/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change fin-core and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ChangeKind".
 */
export type ChangeKind = "added" | "removed" | "updated";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "EntityKind".
 */
export type EntityKind = "asset" | "track" | "clip" | "timeline";
/**
 * Exact rational seconds: "n/d" in lowest terms, or "n" when d is 1.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rational".
 */
export type Rational = string;
/**
 * A primitive edit operation, tagged by `op`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Op".
 */
export type Op =
  | {
      audio?: AudioInfo | null;
      duration: Rational;
      fingerprint: string;
      id: string;
      op: "asset.add";
      path: string;
      video?: VideoInfo | null;
    }
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
      id: string;
      op: "asset.remove";
    }
  | {
      id: string;
      op: "clip.remove";
      ripple: boolean;
    }
  | {
      clip: Clip;
      op: "clip.restore";
      ripple?: boolean;
      track: string;
    };
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrackKind".
 */
export type TrackKind = "video" | "audio";

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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Diff".
 */
export interface Diff {
  changes: Change[];
  shifts: Shift[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Change".
 */
export interface Change {
  change: ChangeKind;
  id: string;
  kind: EntityKind;
}
/**
 * Items on `track` at or after `from` moved by `by` (ripple).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Shift".
 */
export interface Shift {
  by: Rational;
  from: Rational;
  track: string;
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoInfo".
 */
export interface VideoInfo {
  height: number;
  rate: Rational;
  width: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Clip".
 */
export interface Clip {
  asset: string;
  duration: Rational;
  id: string;
  sourceIn: Rational;
  start: Rational;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Project".
 */
export interface Project {
  assets: {
    [k: string]: Asset;
  };
  id: string;
  name: string;
  rate: Rational;
  schemaVersion: number;
  timeline: Timeline;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Asset".
 */
export interface Asset {
  audio?: AudioInfo | null;
  duration: Rational;
  fingerprint: string;
  id: string;
  path: string;
  video?: VideoInfo | null;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Timeline".
 */
export interface Timeline {
  id: string;
  tracks: Track[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Track".
 */
export interface Track {
  id: string;
  /**
   * Sorted by `start`, non-overlapping.
   */
  items: Clip[];
  kind: TrackKind;
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
  rev: number;
  tx: Tx;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Tx".
 */
export interface Tx {
  author: string;
  baseRev: number;
  branch: string;
  ops: Op[];
  schemaVersion: number;
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
