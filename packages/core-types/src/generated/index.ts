/* eslint-disable */
/**
 * Generated from schema/fin-core.schema.json by scripts/generate.mjs.
 * Do not edit by hand: change the Rust types and run `pnpm --filter @finisher/core-types generate`.
 */

/**
 * How long a grant lives.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AclScope".
 */
export type AclScope = "static" | "owner";
/**
 * Whom an ACE names: a SID (a capability's, derived here) or a container by name (its SID
 * is Windows', [`super::appcontainer::sid`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AclTrustee".
 */
export type AclTrustee =
  | {
      kind: "sid";
      value: string;
    }
  | {
      kind: "container";
      value: string;
    };
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
 * A GPU adapter's type (`wgpu::DeviceType`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AdapterType".
 */
export type AdapterType = "integrated" | "discrete" | "virtual" | "cpu" | "other";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AdvisoryAction".
 */
export type AdvisoryAction = "warn" | "disable";
/**
 * Who decided an approval.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DecidedBy".
 */
export type DecidedBy = "user" | "policy";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ApprovalDecision".
 */
export type ApprovalDecision = "allow" | "deny" | "expired";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ApprovalKind".
 */
export type ApprovalKind = "rule" | "budget" | "provider" | "plugin";
/**
 * What runs the agent.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Harness".
 */
export type Harness = "builtin" | "claudeCode" | "codex" | "fake" | "plugin";
/**
 * A name from code or a static table (fin-core events::names); "other" for a value not in the table.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Name".
 */
export type Name = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Hashed".
 */
export type Hashed = string;
/**
 * The model's weights class, from the catalogue at the time (§12.7).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Weights".
 */
export type Weights = "open" | "closed" | "unknown";
/**
 * Large or structured content (the agent kinds): inline JSON, or a [`BlobRef`] when
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
 * How `fin` inside the sandbox reaches `finisherd` (spec §8.2–8.4).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FinEndpoint".
 */
export type FinEndpoint =
  | {
      kind: "tcp";
      port: number;
    }
  | {
      kind: "unixSocket";
      path: string;
    }
  | {
      kind: "pipe";
    };
/**
 * The session's mode (§6.1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentMode".
 */
export type AgentMode = "plan" | "draft" | "auto";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionPhase".
 */
export type SessionPhase = "started" | "resumed" | "interrupted" | "ended";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Code".
 */
export type Code = string;
/**
 * The kind of sign-in a CLI harness reports (CLI harnesses only).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SignIn".
 */
export type SignIn = "plan" | "key" | "unknown";
/**
 * What started the session.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionTrigger".
 */
export type SessionTrigger = "user" | "savedPrompt" | "subagent" | "hook" | "eval";
/**
 * How a tool result's txIds were joined to its call (§12.3).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Attribution".
 */
export type Attribution = "exact" | "inferred" | "ambiguous" | "none";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Blake3Hex".
 */
export type Blake3Hex = string;
/**
 * How faithful a turn's content is (§12.6).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Capture".
 */
export type Capture = "exact" | "stream";
/**
 * Where a turn's cost comes from.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CostBasis".
 */
export type CostBasis = "unknown" | "reported" | "priced" | "subscription";
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
  | PluginLifecycle
  | RegistryRefresh
  | ErrorInternal
  | ErrorPanic
  | JobDone
  | WorkerExit
  | LibraryScan
  | SandboxUnconfined
  | AuthPair
  | AgentSession
  | AgentTurn
  | AgentToolCall
  | AgentToolResult
  | AgentApproval
  | UiAction
  | ReviewVerdict
  | MediaCapabilitiesEvent
  | RenderStats
  | PreviewStats
  | RenderSample;
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
 * `content` or `code`; `unknown` when the call failed before a manifest was read.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginKindName".
 */
export type PluginKindName = "content" | "code" | "unknown";
/**
 * How a plugin or registry call ended.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginOutcome".
 */
export type PluginOutcome = "ok" | "refused" | "failed";
/**
 * A plugin as the journal names it: a bundled plugin by its table name, any other by the
 * journal's keyed hash of its id.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginRef".
 */
export type PluginRef =
  | {
      bundled: Name;
    }
  | {
      hashed: Hashed;
    };
/**
 * The overall risk shown to people (spec §4.3); `unknown` as for [`PluginKindName`].
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RiskName".
 */
export type RiskName = "low" | "medium" | "high" | "unknown";
/**
 * Where an installed plugin came from (spec §5.2).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginSourceName".
 */
export type PluginSourceName = "bundled" | "registry" | "dev";
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
 * How frames travel between the decoder, the compositor and the encoder (spec §5.4): through
 * host memory (`copy`; all of 6A) or as shared GPU surfaces (`zeroCopy`, 6G).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CopyPath".
 */
export type CopyPath = "copy" | "zeroCopy";
/**
 * A job's terminal state.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobOutcome".
 */
export type JobOutcome = "succeeded" | "failed" | "cancelled" | "interrupted";
/**
 * A job's priority as submitted.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobPriority".
 */
export type JobPriority = "interactive" | "normal" | "background";
/**
 * A helper's kind.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProcessHelperKind".
 */
export type ProcessHelperKind = "codec" | "plugin";
/**
 * Why a process ended. A closed list (`worker_exit_reasons_closed`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ExitReason".
 */
export type ExitReason =
  | "done"
  | "idleExpired"
  | "recycled"
  | "cancelled"
  | "killed"
  | "crashed"
  | "hung"
  | "stalled"
  | "abandoned"
  | "spawnFailed"
  | "oom"
  | "protocolError";
/**
 * What the process was.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProcessRole".
 */
export type ProcessRole = "worker" | "helper";
/**
 * A library's effective storage kind.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryStorageName".
 */
export type LibraryStorageName = "local" | "network" | "sync";
/**
 * What started a scan: `add` (the first scan), `schedule` (the poll cadence or the full-scan
 * schedule), `demand` (someone asked, a freshness pass, a file found changed), `watch` (a local
 * root's watcher), `mount` (the root came back online).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ScanTrigger".
 */
export type ScanTrigger = "add" | "schedule" | "demand" | "watch" | "mount";
/**
 * The operating system a sandbox applies on.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxOs".
 */
export type SandboxOs = "linux" | "macos" | "windows";
/**
 * A sandbox profile (spec §8.1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxProfileName".
 */
export type SandboxProfileName = "agent" | "hook" | "worker" | "plugin";
/**
 * Why a profile runs unconfined (F2: both run, with persistent warnings).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "UnconfinedReason".
 */
export type UnconfinedReason = "landlockDisabled" | "profileNotReady";
/**
 * A pairing step.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PairOutcome".
 */
export type PairOutcome = "offered" | "redeemed" | "approved" | "rejected" | "expired" | "revoked";
/**
 * What a device runs on (`PLATFORMS`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PairPlatform".
 */
export type PairPlatform = "macos" | "windows" | "linux" | "ios" | "android" | "web" | "other";
/**
 * How a device reached the server (`ROUTE_KINDS`): a kind, never an address.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PairRoute".
 */
export type PairRoute = "loopback" | "lan" | "tailscale" | "public" | "connect" | "other";
/**
 * Who provides a backend (spec §6.1): the export-encoder policy and the capability report key
 * on it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BackendClass".
 */
export type BackendClass = ("os" | "gpu" | "software" | "plugin") | "other";
/**
 * How a codec direction is served: hardware, software, or not at all.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SupportClass".
 */
export type SupportClass = "hw" | "sw" | "none";
/**
 * The worker sandbox in force (spec §3.5): `none` in 6A.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Sandbox".
 */
export type Sandbox = "none" | "landlock" | "worker";
/**
 * A stage of a render, for [`RenderStats`].
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RenderPhase".
 */
export type RenderPhase = ("decode" | "composite" | "readback") | "encode";
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
 * How a mapped file may be used.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FileMode".
 */
export type FileMode = "read" | "write" | "readWrite";
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
 * Why a `cancel` was sent.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CancelReason".
 */
export type CancelReason = "cancel" | "pause";
/**
 * How a codec direction is served on this machine (spec §6.2).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CapSupport".
 */
export type CapSupport = "hardware" | "software" | "none";
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VideoCodec".
 */
export type VideoCodec = ("av1" | "h264" | "hevc" | "vp9" | "vp8" | "proRes") | "other";
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
 * Every key of spec §4.2, consumed or not.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ContributionKind".
 */
export type ContributionKind =
  | "fontCatalogs"
  | "captionStyles"
  | "commands"
  | "hooks"
  | "skills"
  | "prompts"
  | "roles"
  | "rubrics"
  | "analyzers"
  | "providers"
  | "codecBackends"
  | "matteProducers"
  | "audioProcessors"
  | "luts"
  | "effects"
  | "transitions"
  | "lottieTemplates"
  | "formatTemplates"
  | "exportPresets"
  | "qaProfiles"
  | "duckingRules"
  | "editPresets"
  | "ui"
  | "agentProviders"
  | "slots"
  | "importers"
  | "embedders";
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DeviceAccess".
 */
export type DeviceAccess = "read" | "readWrite";
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
 * Which program emitted an entry. The server sets it from the connection's principal (5A
 * Decision 27: `local` and `agent` → `fin`, `mcp` → `mcp`, `console` → `console`, `device` →
 * `ui`), never from the payload. `console`, a person at the server, sends no events of its
 * own; `mcp`, the MCP adapter (D16), sends the kinds `fin` does.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Source".
 */
export type Source = "fin" | "finisherd" | "finNode" | "ui" | "agent" | "console" | "mcp";
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
      kind: "plugin.lifecycle";
      kv: 1;
      payload: PluginLifecycle;
    }
  | {
      kind: "registry.refresh";
      kv: 1;
      payload: RegistryRefresh;
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
      kind: "job.done";
      kv: 2;
      payload: JobDone;
    }
  | {
      kind: "worker.exit";
      kv: 1;
      payload: WorkerExit;
    }
  | {
      kind: "library.scan";
      kv: 1;
      payload: LibraryScan;
    }
  | {
      kind: "sandbox.unconfined";
      kv: 1;
      payload: SandboxUnconfined;
    }
  | {
      kind: "auth.pair";
      kv: 1;
      payload: AuthPair;
    }
  | {
      kind: "agent.session";
      kv: 1;
      payload: AgentSession;
    }
  | {
      kind: "agent.turn";
      kv: 1;
      payload: AgentTurn;
    }
  | {
      kind: "agent.toolCall";
      kv: 1;
      payload: AgentToolCall;
    }
  | {
      kind: "agent.toolResult";
      kv: 1;
      payload: AgentToolResult;
    }
  | {
      kind: "agent.approval";
      kv: 1;
      payload: AgentApproval;
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
    }
  | {
      kind: "media.capabilities";
      kv: 1;
      payload: MediaCapabilitiesEvent;
    }
  | {
      kind: "render.stats";
      kv: 1;
      payload: RenderStats;
    }
  | {
      kind: "preview.stats";
      kv: 0;
      payload: PreviewStats;
    }
  | {
      kind: "render.sample";
      kv: 0;
      payload: RenderSample;
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
 * via the `definition` "FsAccess".
 */
export type FsAccess = "read" | "readWrite";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FsScope".
 */
export type FsScope = "pluginData";
/**
 * A helper's kind.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HelperKind".
 */
export type HelperKind = "codec" | "plugin";
/**
 * What a process is.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HelloRole".
 */
export type HelloRole = "worker" | "helper";
/**
 * A message from the host to a worker or a helper.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HostMsg".
 */
export type HostMsg =
  | (Start & {
      t: "start";
    })
  | {
      job: string;
      reason: CancelReason;
      t: "cancel";
    }
  | {
      t: "shutdown";
    }
  | (Attach & {
      t: "attach";
    })
  | {
      error?: JobError;
      id: number;
      t: "reply";
      value?: unknown;
    };
/**
 * How the engine may retry after a worker `error` (Decision 31).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Retry".
 */
export type Retry = "never" | "backoff" | "fresh";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanAction".
 */
export type PlanAction = "install" | "update" | "downgrade";
/**
 * One permission, tagged by `perm`. The set is closed: an unknown `perm` or field fails the
 * parse (a unit permission with any other field too, which serde's tagged enums allow).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Permission".
 */
export type Permission =
  | {
      perm: "project:read";
    }
  | {
      perm: "project:write";
    }
  | {
      perm: "media:read";
    }
  | {
      perm: "media:file";
    }
  | {
      perm: "jobs:submit";
    }
  | {
      perm: "gpu";
    }
  | {
      origins: Origin[];
      perm: "net";
    }
  | {
      mode: SecretMode;
      names: string[];
      perm: "secrets";
    }
  | {
      access: FsAccess;
      perm: "fs";
      scope: FsScope;
    }
  | {
      hooks: string[];
      perm: "hooks:observe";
    }
  | {
      hooks: string[];
      perm: "hooks:modify";
    };
/**
 * `https://host[:port]`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Origin".
 */
export type Origin = string;
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SecretMode".
 */
export type SecretMode = "brokered" | "raw";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LicenceClass".
 */
export type LicenceClass = "permissive" | "weakCopyleft" | "copyleft" | "nonCommercial" | "unknown";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginKind".
 */
export type PluginKind = "content" | "code";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Risk".
 */
export type Risk = "low" | "medium" | "high";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SourceKind".
 */
export type SourceKind = "bundled" | "registry" | "dev";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostKillTree".
 */
export type JobHostKillTree = "jobObject" | "cgroup" | "processGroup";
/**
 * A sandbox profile's state on this host.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostProfileState".
 */
export type JobHostProfileState = "enforced" | "disabled" | "unavailable";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostCgroups".
 */
export type JobHostCgroups = "auto" | "off";
/**
 * What the host reports. Per process: `spawned` (its `spawn` has resolved), `hello`, its
 * messages and `stderr` lines, then `exit`; `groupEmpty` follows the last `exit` of a group.
 * `progress` is coalesced (at most every 250 ms, the newest value, and always before the
 * next other message); `alive` never arrives. `memory` (the worker group's bytes) comes with
 * `ready` only.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostEvent".
 */
export type JobHostEvent =
  | {
      group: number;
      proc: number;
      type: "spawned";
    }
  | {
      hello: Hello;
      proc: number;
      type: "hello";
    }
  | {
      memory?: number;
      msg: WorkerMsg;
      proc: number;
      type: "message";
    }
  | {
      line: string;
      proc: number;
      type: "stderr";
    }
  | {
      info: JobHostExitInfo;
      proc: number;
      type: "exit";
    }
  | {
      group: number;
      type: "groupEmpty";
    }
  | {
      proc: number;
      type: "abandonedExit";
    };
/**
 * A message from a worker or a helper to its host. Any message counts as alive.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WorkerMsg".
 */
export type WorkerMsg =
  | (Hello & {
      t: "hello";
    })
  | {
      item: string;
      job: string;
      t: "begin";
    }
  | {
      error?: JobError;
      item: string;
      job: string;
      t: "item";
      value?: unknown;
    }
  | (Progress & {
      t: "progress";
    })
  | {
      data: unknown;
      job: string;
      t: "checkpoint";
    }
  | {
      t: "alive";
    }
  | {
      job: string;
      t: "result";
      value: unknown;
    }
  | {
      error: JobError;
      itemErrors?: ItemError[];
      job: string;
      t: "error";
    }
  | {
      t: "ready";
    }
  | {
      job?: string;
      kind: string;
      payload: unknown;
      t: "diag";
    }
  | {
      id: number;
      method: string;
      params: unknown;
      t: "call";
    };
/**
 * The unit of a progress figure.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProgressUnit".
 */
export type ProgressUnit = "bytes" | "items" | "frames" | "seconds";
/**
 * An `exit` event's facts.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostExitInfo".
 */
export type JobHostExitInfo = {
  /**
   * The exit code (Windows: the NTSTATUS or code; a process the host killed: 137).
   */
  code?: number;
  /**
   * Processes in the group the host did not start.
   */
  foreign?: number;
  lifetimeMs: number;
  memoryPeak?: number;
  /**
   * Lower case without `SIG` (fin-core's `SIGNALS`; `other` outside them). Never on Windows.
   */
  signal?: string;
  /**
   * The last 8 KiB of stderr.
   */
  stderrTail: string;
} & JobHostExitInfo1;
export type JobHostExitInfo1 =
  | {
      reason: "exited";
    }
  | {
      kill: JobHostKillReason;
      reason: "killed";
    }
  | {
      reason: "hung";
    }
  | {
      reason: "stalled";
    }
  | {
      reason: "timedOut";
    }
  | {
      reason: "oom";
    }
  | {
      reason: "resourceFault";
    }
  | {
      reason: "protocolError";
      what: string;
    }
  | {
      expected: string;
      got: string;
      reason: "protocolMismatch";
    }
  | {
      reason: "noHello";
    }
  | {
      reason: "spawnFailed";
      what: string;
    }
  | {
      reason: "abandoned";
    };
/**
 * Why the host killed a group.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostKillReason".
 */
export type JobHostKillReason =
  "cancel" | "shutdown" | "recycle" | "idleExpired" | "helperFailed" | "jobEnded" | "hung" | "test";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostQos".
 */
export type JobHostQos = "normal" | "utility" | "background";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnProfile".
 */
export type JobSpawnProfile = "worker" | "plugin";
/**
 * `bundled`: `fin-workers`, `fin-analyze` or `fin-sandbox-exec` in `binaryDir`; `path`: plugin processes only.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnProgram".
 */
export type JobSpawnProgram =
  | {
      bundled: string;
    }
  | {
      path: string;
    };
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnRole".
 */
export type JobSpawnRole = "worker" | "helper";
/**
 * Clockwise display rotation in degrees, from the track matrix.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Rotation".
 */
export type Rotation = 0 | 90 | 180 | 270;
/**
 * What a scan lists (`WalkMode`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryScanMode".
 */
export type LibraryScanMode = "full" | "dirs" | "poll";
/**
 * `@<publisher>/<name>` (8A Decision 2).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginId".
 */
export type PluginId = string;
/**
 * The shipped OS and architecture pairs (the triples of `deny.toml`), in Node's spelling.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Platform".
 */
export type Platform = "darwin-arm64" | "darwin-x64" | "linux-x64" | "linux-arm64" | "win32-x64" | "win32-arm64";
/**
 * How a code plugin runs (spec §3.2). `python` and `wasm` are after v1.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Runtime".
 */
export type Runtime =
  | {
      exec: {
        "darwin-arm64"?: string;
        "darwin-x64"?: string;
        "linux-arm64"?: string;
        "linux-x64"?: string;
        "win32-arm64"?: string;
        "win32-x64"?: string;
      };
      type: "native";
    }
  | {
      main: string;
      node: string;
      type: "node";
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
 * A cloud placeholder (Decision 12): catalogued from metadata only, never opened unless the
 * library has `hydrate`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Placeholder".
 */
export type Placeholder =
  "dataless" | "icloudStub" | "nextcloud" | "owncloud" | "recallOnOpen" | "recallOnData" | "offline" | "unpinned";
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Severity".
 */
export type Severity = "low" | "medium" | "high" | "critical";
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
 * A frame size as a bucket: the smallest standard size the frame fits within, by pixel count
 * (so a portrait clip buckets as its landscape twin): `sd` ≤ 960×540, `hd720` ≤ 1280×720,
 * `hd1080` ≤ 2048×1080, `qhd` ≤ 2560×1440, `uhd4k` ≤ 4096×2160, `uhd8k` above.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ResolutionBucket".
 */
export type ResolutionBucket = "sd" | "hd720" | "hd1080" | "qhd" | "uhd4k" | "uhd8k";
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
 * Whether `fin-sandbox-exec` can hide Unix sockets from `agent` and `hook` processes in a user
 * and mount namespace on this host (plan 7C Decision 26). Only the launcher binary can tell
 * (an AppArmor profile attaches to it by path), so `finisherd` asks it: `--probe`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxNamespaces".
 */
export type SandboxNamespaces =
  | {
      kind: "available";
    }
  | {
      kind: "unavailable";
      reason: string;
    }
  | {
      kind: "notApplicable";
    };
/**
 * What a profile gets on this host.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxMechanism".
 */
export type SandboxMechanism =
  | {
      detail: string;
      kind: "enforced";
    }
  | {
      kind: "disabled";
      reason: UnconfinedReason;
    }
  | {
      kind: "unavailable";
      need: string;
    };
/**
 * The four profiles (spec §8.10). `SandboxProfile` in TypeScript.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxProfile".
 */
export type SandboxProfile = "agent" | "hook" | "worker" | "plugin";
/**
 * How `test.crash` dies: `abort`, `SIGSEGV`/an access violation, `SIGBUS`/
 * `EXCEPTION_IN_PAGE_ERROR`, Windows `__fastfail`, `exit(3)`, or by allocating until its
 * group's memory limit kills it (`oom`; it aborts past 2 GiB).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCrashHow".
 */
export type TestCrashHow = "abort" | "segv" | "bus" | "fastfail" | "exit" | "oom";
/**
 * `test.spawn`: the grandchild stays in the group or leaves it with `setsid`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestDetach".
 */
export type TestDetach = "none" | "setsid";
/**
 * What `test.echo` reports about its own process.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestEchoReport".
 */
export type TestEchoReport = "pid" | "env" | "args" | "fds" | "cwd" | "pdeathsig" | "nice" | "cgroup";
/**
 * `test.garbage`: the protocol violation to commit.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestGarbage".
 */
export type TestGarbage = "json" | "seq" | "long" | "otherJob" | "unknownType";
/**
 * The test helper's behaviour (`fin-workers --helper test:<behaviour>`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestHelperBehaviour".
 */
export type TestHelperBehaviour = "ok" | "exitEarly" | "hang" | "spawn" | "call";
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
/**
 * What one worker serves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WorkerScope".
 */
export type WorkerScope =
  | {
      /**
       * `<serverData>/jobs/<jobId>`.
       */
      jobDir: string;
      kind: "cold";
      /**
       * The manifest's sources, per file where the OS allows it (read-only).
       */
      sources?: string[];
    }
  | {
      kind: "warm";
      /**
       * The project's `sources/` and the library roots it references (read-only).
       */
      libraryRoots?: string[];
      /**
       * `<serverData>/jobs/<name>`.
       */
      workerDir: string;
    };

export interface FinCore {
  [k: string]: unknown;
}
/**
 * What a person accepted with an approval, beyond the permissions (spec §5.3).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Accept".
 */
export interface Accept {
  copyleft?: boolean;
  thirdPartyCode?: boolean;
}
/**
 * One allow ACE, inherited by the subtree.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AclGrant".
 */
export interface AclGrant {
  mask: number;
  path: string;
  scope: AclScope;
  trustee: AclTrustee;
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
 * `agent.approval`: an approval was answered or expired.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentApproval".
 */
export interface AgentApproval {
  approvalId: IdRef;
  by: DecidedBy;
  decision: ApprovalDecision;
  kind: ApprovalKind;
  latencyMs: number;
  provenance?: Provenance;
  sessionId: IdRef;
  subject?: Body;
}
/**
 * A session's provenance, as the agent-trace store stamps it on every content entry that is
 * not a session or a turn (those carry the same fields flat).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Provenance".
 */
export interface Provenance {
  harness: Harness;
  model: Name;
  modelHash?: Hashed;
  provider: Name;
  weights: Weights;
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
 * A CLI agent session (spec §8.1, §8.5, §8.6; plan 7C Decision 6).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentInput".
 */
export interface AgentInput {
  fin?: FinEndpoint | null;
  /**
   * What runs the agent.
   */
  harness: "builtin" | "claudeCode" | "codex" | "fake" | "plugin";
  home: string;
  /**
   * The roots of the libraries the project references (read-only).
   */
  libraryRoots?: string[];
  mode: AgentMode;
  projectDir: string;
  serverData: string;
  /**
   * `<serverData>/agents/<sessionId>`.
   */
  sessionDir: string;
  /**
   * One path component (it names `workspace/<sessionId>/` and `agents/<sessionId>/`).
   */
  sessionId: string;
  /**
   * `<projectDir>/workspace/shared`; written only outside `plan` mode.
   */
  sharedDir?: string | null;
  /**
   * The harness's sign-in store file, inside `<serverData>/providers/<harness>/home/`
   * (spec §8.6): read and written in place, never its directory.
   */
  signInStore?: string | null;
  /**
   * The base toolchain list plus the harness's own (files and directories, read-only).
   */
  toolchains?: string[];
  /**
   * `<projectDir>/workspace/<sessionId>`.
   */
  workspaceDir: string;
}
/**
 * `agent.session`: a session started, resumed, was interrupted or ended.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentSession".
 */
export interface AgentSession {
  harness: Harness;
  mode: AgentMode;
  model: Name;
  modelHash?: Hashed;
  parent?: IdRef;
  phase: SessionPhase;
  provider: Name;
  reason?: Code;
  role: Name;
  sessionId: IdRef;
  signIn?: SignIn;
  totals?: SessionTotals;
  trigger: SessionTrigger;
  weights: Weights;
}
/**
 * A session's totals at `ended`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SessionTotals".
 */
export interface SessionTotals {
  costMicros?: number;
  inputTokens: number;
  outputTokens: number;
  toolCalls: number;
  turns: number;
  txs: number;
  wallS: number;
}
/**
 * `agent.toolCall`: the agent called a tool. `callId` is the `FIN_CALL` its `fin` run
 * carries; `command` names the `fin` command when the tool ran `fin`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentToolCall".
 */
export interface AgentToolCall {
  args?: Body;
  argsHash: Hashed;
  callId: IdRef;
  command?: Name;
  provenance?: Provenance;
  tool: Name;
  turnId: IdRef;
}
/**
 * `agent.toolResult`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentToolResult".
 */
export interface AgentToolResult {
  attribution: Attribution;
  callId: IdRef;
  code?: Code;
  exit?: number;
  micros: number;
  ok: boolean;
  output?: Body;
  provenance?: Provenance;
  truncated?: boolean;
  /**
   * @maxItems 64
   */
  txIds?: IdRef[];
}
/**
 * `agent.turn`: one model call. Content (training only, in the agent-trace store): the
 * turn's new input messages, the output, and the system prompt and tool definitions, which
 * are always blobs so identical prompts are stored once.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AgentTurn".
 */
export interface AgentTurn {
  bodyBlake3?: Blake3Hex;
  bodyHash?: Hashed;
  cacheReadTokens: number;
  cacheWriteTokens: number;
  capture: Capture;
  costBasis: CostBasis;
  costMicros?: number;
  harness: Harness;
  inputTokens: number;
  latencyMs: number;
  messagesDelta?: Body;
  model: Name;
  modelHash?: Hashed;
  output?: Body;
  outputTokens: number;
  parentTurnId?: IdRef;
  provider: Name;
  reasoningTokens: number;
  runId: IdRef;
  seq: number;
  stopReason: StopReason;
  system?: Body;
  systemHash?: Hashed;
  tools?: Body;
  toolsHash?: Hashed;
  ttftMs?: number;
  turnId: IdRef;
  upstream?: Name;
  upstreamHash?: Hashed;
  weights: Weights;
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
 * `plugin.lifecycle`: one plugin call (plan, install, update, downgrade, remove, enable,
 * disable, revoke).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginLifecycle".
 */
export interface PluginLifecycle {
  /**
   * A name from code or a static table (fin-core events::names); "other" for a value not in the table.
   */
  action: string;
  /**
   * The package's unpacked bytes as a log₂ bucket ([`log2_bucket`]).
   */
  bytesLog2: number;
  code?: Code;
  kind: PluginKindName;
  micros: number;
  outcome: PluginOutcome;
  /**
   * Permissions added or widened against the installed version (all of them for a first
   * install), capped at 255.
   */
  permissionsAdded: number;
  plugin?: PluginRef;
  risk: RiskName;
  source: PluginSourceName;
}
/**
 * `registry.refresh`: one refresh of a registry's trusted metadata.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RegistryRefresh".
 */
export interface RegistryRefresh {
  /**
   * A human principal asked for it (agents' and `system`'s refreshes install nothing).
   */
  byHuman: boolean;
  /**
   * A `TUF_*` code, `CLOCK_SUSPECT`, `REGISTRY_UNREACHABLE`, ...
   */
  code?: string;
  micros: number;
  outcome: PluginOutcome;
  /**
   * Roles whose trusted version changed.
   */
  rolesUpdated: number;
  /**
   * Days the registry's metadata is past its expiry (relaxed freshness), when it is.
   */
  staleDays?: number;
  /**
   * Targets listed after the refresh, as a log₂ bucket.
   */
  targetsLog2: number;
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
 * `job.done` (`kv` 2: `kv` 1 plus the optional `media` object of sub-project 6).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobDone".
 */
export interface JobDone {
  attempts: number;
  code?: Code;
  family: Name;
  /**
   * The helper processes started over all attempts.
   */
  helpers: number;
  /**
   * Whether isolation (bisection) ran.
   */
  isolated: boolean;
  /**
   * Batch kinds: the job's item count.
   */
  items?: number;
  /**
   * Batch kinds: the items that failed.
   */
  itemsFailed?: number;
  kind: Name;
  media?: JobMedia;
  outcome: JobOutcome;
  priority: JobPriority;
  /**
   * From submit to the first start.
   */
  queuedMs: number;
  /**
   * The sum of every attempt's run time.
   */
  runMicros: number;
  warm: boolean;
}
/**
 * The `media` object of `job.done` `kv` 2: what a media job did. `codec`, `backend` and `hw`
 * describe the decode that served the job (the first source it decoded; `none` when it
 * decoded nothing).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobMedia".
 */
export interface JobMedia {
  backend: Name;
  /**
   * Every source the job needed was already open in the warm worker (and it needed one).
   */
  cacheHit: boolean;
  codec: Name;
  copyPath: CopyPath;
  /**
   * Frames the job produced.
   */
  frames: number;
  hw: boolean;
  /**
   * The largest frame the job produced.
   */
  resolution: "sd" | "hd720" | "hd1080" | "qhd" | "uhd4k" | "uhd8k";
  /**
   * Media seconds produced per wall second of rendering, in thousandths (1000 = real
   * time): the frames' durations at the timeline's rate over decode + composite +
   * readback time. `0` when the job took no measurable time.
   */
  rtFactorMilli: number;
}
/**
 * `worker.exit` (`kv` 1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WorkerExit".
 */
export interface WorkerExit {
  exitCode?: number;
  family: Name;
  helperKind?: ProcessHelperKind;
  jobsServed: number;
  lifetimeS: number;
  plugin?: Hashed;
  reason: ExitReason;
  role: ProcessRole;
  sandboxed: boolean;
  signal?: Name;
}
/**
 * `library.scan` (`kv` 1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryScan".
 */
export interface LibraryScan {
  added: number;
  changed: number;
  code?: Code;
  /**
   * The catalog's live directories after the scan, as a log₂ bucket.
   */
  dirs: number;
  /**
   * The catalog's live files after the scan, as a log₂ bucket.
   */
  files: number;
  /**
   * A full scan (else a poll, a targeted scan or a watch rescan).
   */
  full: boolean;
  micros: number;
  missing: number;
  /**
   * Rows that kept their history under a new path (renames by file id).
   */
  moved: number;
  outcome: JobOutcome;
  /**
   * Catalogued cloud-only files and directories after the scan.
   */
  placeholders: number;
  storage: LibraryStorageName;
  trigger: ScanTrigger;
}
/**
 * `sandbox.unconfined` (`kv` 1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxUnconfined".
 */
export interface SandboxUnconfined {
  os: SandboxOs;
  profile: SandboxProfileName;
  reason: UnconfinedReason;
}
/**
 * `auth.pair` (`kv` 1).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "AuthPair".
 */
export interface AuthPair {
  /**
   * From the redeem to the owner's answer (`approved`, `rejected`).
   */
  micros?: number | null;
  outcome: PairOutcome;
  /**
   * The device's platform, once a device redeemed the offer.
   */
  platform?: PairPlatform | null;
  /**
   * How the device reached the server, once it redeemed.
   */
  route?: PairRoute | null;
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
 * `media.capabilities` (`kv` 1): once per server run, what the media worker can decode,
 * encode and draw on.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MediaCapabilitiesEvent".
 */
export interface MediaCapabilitiesEvent {
  codecs: CodecEntry[];
  copyPath: CopyPath;
  gpu?: GpuEntry;
  /**
   * `RENDER_VERSION`: part of every cache key.
   */
  renderVersion: number;
  sandbox: Sandbox;
}
/**
 * One codec's support on this machine.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CodecEntry".
 */
export interface CodecEntry {
  /**
   * The backends that would be used, decode first.
   */
  backends: BackendEntry[];
  codec: Name;
  decode: SupportClass;
  encode: SupportClass;
}
/**
 * A backend and who provides it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "BackendEntry".
 */
export interface BackendEntry {
  class: BackendClass;
  name: Name;
}
/**
 * The GPU adapter frames are composited on: numeric ids only (owner decision 7). The
 * adapter's and driver's name strings identify hardware and are never recorded.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "GpuEntry".
 */
export interface GpuEntry {
  adapterType: AdapterType;
  backend: Name;
  /**
   * The PCI device id.
   */
  deviceId: number;
  /**
   * The driver's major version, when its version string has one.
   */
  driverMajor?: number;
  /**
   * A CPU adapter (lavapipe, WARP).
   */
  software: boolean;
  /**
   * The PCI vendor id.
   */
  vendorId: number;
}
/**
 * `render.stats` (`kv` 1): per-minute histograms of render stages in micros, one entry per
 * phase and backend. `backend` is the decode backend for `decode` and the GPU API for
 * `composite` and `readback`. A worker sends what it measured since its last job ended, so
 * several entries may share a window (`histogram.start`): readers add them.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RenderStats".
 */
export interface RenderStats {
  backend: Name;
  histogram: Histogram;
  phase: RenderPhase;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PreviewStats".
 */
export interface PreviewStats {}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RenderSample".
 */
export interface RenderSample {}
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
 * `attach`: a helper's job, its files only, and its directories.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Attach".
 */
export interface Attach {
  dirs: Dirs;
  files?: FileGrant[];
  job: string;
}
/**
 * The job directory and the process's private temp directory.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Dirs".
 */
export interface Dirs {
  job: string;
  tmp: string;
}
/**
 * One mapped file (or directory, `name` ending in `/`) of the job directory.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FileGrant".
 */
export interface FileGrant {
  /**
   * The size 5C created it at; absent for a file written by `prepare` and for directories.
   */
  bytes?: number;
  mode: FileMode;
  name: string;
  path: string;
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
 * `media.capabilities` v1 takes no params.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CapabilitiesParams".
 */
export interface CapabilitiesParams {}
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
 * A caption style (`fin_core::CaptionStyle` JSON) that `fin style add --from
 * @pub/pack:<name>` copies into a project.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CaptionStyleContribution".
 */
export interface CaptionStyleContribution {
  file: string;
  /**
   * `[a-z0-9][a-z0-9-]{0,63}`, unique in the pack.
   */
  name: string;
  /**
   * A message key in the `en` catalogue for the style's display name.
   */
  title?: string | null;
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
 * What this machine can decode and encode: the shape of `fin-codecs`' `Capabilities`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CodecCapabilities".
 */
export interface CodecCapabilities {
  video: CodecCaps[];
}
/**
 * One codec's support: the shape of `fin-codecs`' `CodecSupport`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "CodecCaps".
 */
export interface CodecCaps {
  /**
   * The backends that would be used, decode first.
   */
  backends: string[];
  codec: VideoCodec;
  decode: CapSupport;
  encode: CapSupport;
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
 * Contributions by kind (spec §4.2): the kinds this host consumes. Validation refuses any
 * other key as `PLUGIN_CONTRIBUTION_UNSUPPORTED`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Contributes".
 */
export interface Contributes {
  captionStyles?: CaptionStyleContribution[];
  fontCatalogs?: FontCatalogContribution[];
}
/**
 * A font catalog file (`fin_fonts::FontCatalog` JSON) whose entries join the server's
 * catalog by entry id (8A Decision 22).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontCatalogContribution".
 */
export interface FontCatalogContribution {
  file: string;
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
 * A device node a profile may open (spec §8.10 GPU rows).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "DeviceRule".
 */
export interface DeviceRule {
  access: DeviceAccess;
  path: string;
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
 * directory, a nested mount unmounted or replaced) marks nothing missing; `code` says why.
 * `entries` counts the listing's entries. `volume`: the volume measured at `dir` when it is a
 * nested mount root (Decision 17, per directory), which the indexer keeps as
 * [`KnownDir::volume`].
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
  volume?: VolumeIdentity | null;
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
 * The library's patterns, as stored and as sent to a scan worker.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ExcludeSpec".
 */
export interface ExcludeSpec {
  /**
   * The root's case sensitivity: keys and patterns are folded when `false`.
   */
  caseSensitive: boolean;
  exclude?: string[];
  include?: string[] | null;
}
/**
 * What an open must find (Decision 19 step 3): the catalogued size, mtime and, when the
 * catalog has one, file id. TS `ExpectWire`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Expect".
 */
export interface Expect {
  /**
   * The catalogued entry's device (`ScanEntry.dev`). Nested mounts are catalogued, so the
   * opener crosses them and checks the opened handle against this device; without one,
   * the handle must be on the root's mount.
   */
  dev?: string | null;
  fileId?: string | null;
  mtimeNs: string;
  size: number;
}
/**
 * One page of `diagnostics.export`, oldest first. `next` is an opaque position: pass it as
 * the next request's `cursor`; absent on the last page. `redacted`: content fields (and the
 * blob references inside them) were removed, as for a caller that is not a person (5A
 * Decision 22); absent otherwise.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ExportPage".
 */
export interface ExportPage {
  lines: Envelope[];
  next?: string;
  redacted?: boolean;
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
 * A project font asset's file and the SHA-256 its asset pins.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "FontFileRef".
 */
export interface FontFileRef {
  original: OriginalFile;
  sha256: string;
}
/**
 * A file a worker opens, with what it looked like when the manifest was written.
 *
 * `path` may be absolute anywhere on the machine (the model stores the path the user gave:
 * plan 6A Decision 5) and the directories above the file may be links: **by design** only the
 * file itself is opened no-follow and checked against these facts. The manifest is the host's,
 * written from the project's own model, never from anything a worker or a client sent in the
 * job; `--replay` of a directory that is not the caller's own is the exception, and confines
 * originals to the directory (`--originals-anywhere` lifts that). The worker
 * opens it through `fin-library`'s no-follow opener, which refuses it once its size, mtime or
 * file id changed (`ORIGINALS_MISSING {reason: changed}`); `mtimeNs` and `fileId` are
 * encoded as `fin-library`'s `Expect` encodes them (a decimal string; standard base64).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "OriginalFile".
 */
export interface OriginalFile {
  fileId?: string;
  mtimeNs: string;
  /**
   * Absolute.
   */
  path: string;
  size: number;
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
 * The GPU adapter frames were composited on (numeric ids, spec §21 decision 7).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "GpuAdapter".
 */
export interface GpuAdapter {
  backend: string;
  device: number;
  deviceType: string;
  /**
   * The driver's major version, when its version text has a number (the text itself is
   * not kept).
   */
  driverMajor?: number;
  name: string;
  /**
   * A CPU adapter (lavapipe, WARP).
   */
  software: boolean;
  vendor: number;
}
/**
 * What the `library.hash` kind reports for one file; `hash` is `blake3:<hex>`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HashOutcome".
 */
export interface HashOutcome {
  code?: string | null;
  file: number;
  hash?: string | null;
  /**
   * As [`crate::probe::ProbeOutcome::state`]: `present`, or why there is no hash.
   */
  state: string;
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
 * `hello` (Decisions 3, 8). A worker names its `family`, `kinds` and `warm`; a helper its
 * helper `kind` and `id`. `envelope` is [`crate::envelope_id`] (bundled processes must send
 * it; plugin processes may omit it); `selfSandbox` the profile applied (Decision 9a).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Hello".
 */
export interface Hello {
  envelope?: string;
  family?: string;
  /**
   * A helper's id (helpers only).
   */
  id?: string;
  kind?: HelperKind;
  kinds?: KindVersion[];
  /**
   * The protocol minor ([`crate::MINOR`]).
   */
  minor: number;
  pid: number;
  role: HelloRole;
  selfSandbox?: string;
  /**
   * The protocol major ([`crate::PROTOCOL`]).
   */
  v: number;
  warm?: boolean;
}
/**
 * A kind at a version that a worker serves.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "KindVersion".
 */
export interface KindVersion {
  kind: string;
  version: number;
}
/**
 * A helper started beside the worker: its id, kind and the names of the files it sees.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HelperGrant".
 */
export interface HelperGrant {
  files?: string[];
  id: string;
  kind: HelperKind;
}
/**
 * The answer of a history operation ([`Store::undo`](crate::Store::undo),
 * [`Store::redo`](crate::Store::redo)): the records it committed, in order (or planned and
 * rolled back, `dry_run`; a retry of a committed operation answers with its records,
 * `dry_run` false), and what they act on: the units an undo undoes, the undo a redo
 * reverses, or the merge group a revert reverts. `no_op`: why a cherry-pick or revert
 * committed nothing (`ALREADY_PRESENT`: the target holds the record's origin root;
 * `ALREADY_REVERTED`: the merge group's effect is gone there); absent otherwise.
 * `origins`: the txIds the operation acted on, each with the records its `origin` chain
 * leads to (a copy of a copy), first seen first: an undo's units, a cherry-pick's origin, a
 * reverted group's records; empty (and omitted) for a redo and for nothing done (7A
 * Decision 27).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HistoryResult".
 */
export interface HistoryResult {
  committed: CommittedTx[];
  dryRun: boolean;
  noOp?: string | null;
  origins?: string[];
  undone: string[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HookFin".
 */
export interface HookFin {
  /**
   * The `plan`-scoped credential 7F mints for the call (read-only).
   */
  credential: string;
  endpoint: FinEndpoint;
}
/**
 * A hook command (spec §8.10; plan 7C Decision 7).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "HookInput".
 */
export interface HookInput {
  /**
   * Present only when the hook declares `fin`: its credential file and the endpoint. Without
   * it the hook has no network and no credential.
   */
  fin?: HookFin | null;
  home: string;
  /**
   * The hook's own file.
   */
  hookFile: string;
  /**
   * Read-only.
   */
  projectDir: string;
  serverData: string;
  /**
   * The hook's time limit (`Limits.wall_ms`): the launcher ends the whole tree when it passes
   * (on Windows also the backstop for a Node process stuck on piped stdio, Decision 25).
   */
  timeoutMs?: number | null;
  /**
   * A private temporary directory, the only write root.
   */
  tmp: string;
  toolchains?: string[];
}
/**
 * `start`: the job, its limits, its mapped files, its helpers and directories.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Start".
 */
export interface Start {
  /**
   * Whether `diag` messages are forwarded (Decision 7).
   */
  diagnostics?: boolean;
  dirs: Dirs;
  files?: FileGrant[];
  helpers?: HelperGrant[];
  job: StartJob;
  limits: Limits;
}
/**
 * The job a `start` runs.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "StartJob".
 */
export interface StartJob {
  /**
   * 1 on the first attempt.
   */
  attempt: number;
  /**
   * The last checkpoint (resumable kinds).
   */
  checkpoint?: {
    [k: string]: unknown;
  };
  /**
   * What the kind's `prepare` hook returned (Decision 40).
   */
  extra?: {
    [k: string]: unknown;
  };
  id: string;
  kind: string;
  kindVersion: number;
  params: unknown;
}
/**
 * The limits a process runs under.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Limits".
 */
export interface Limits {
  /**
   * The `alive` interval (5 s).
   */
  aliveMs: number;
  /**
   * The job's group memory limit (Decision 20), informative: the host enforces it.
   */
  memoryBytes?: number;
  /**
   * The read budget ([`crate::ReadBudget`]).
   */
  readBytesPerSec?: number;
}
/**
 * An error a worker, a helper or a host call answers: `{code, message, details, retry}`.
 * In a `reply`, `retry` is absent (it reads as `never`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobError".
 */
export interface JobError {
  code: string;
  details?: {
    [k: string]: unknown;
  };
  message: string;
  retry?: Retry;
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
 * What a person approves: made by [`super::make_plan`], kept in `staging/<planId>/plan.json`
 * beside the staged tree, committed by [`crate::store::Store::commit`].
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "InstallPlan".
 */
export interface InstallPlan {
  action: PlanAction;
  /**
   * The package contributes agent-facing content (spec §4.2): an update of it never
   * installs without a person's approval (§5.3).
   */
  agentFacing?: boolean;
  /**
   * Unpacked bytes.
   */
  bytes: number;
  diff: PermissionDiff;
  /**
   * BLAKE3 (hex) of the plan's canonical JSON with `digest` empty ([`super::plan_digest`]):
   * it binds the permissions, risks, licence and the staged tree hash.
   */
  digest: string;
  /**
   * Unix ms.
   */
  expiresAt: number;
  /**
   * The installed version an update or downgrade replaces.
   */
  from?: string | null;
  licence: PlanLicence;
  models?: PlanModel[];
  /**
   * Every permission of the new version with its risk, in manifest order.
   */
  permissions: PlannedPermission[];
  planId: string;
  plugin: PlanPlugin;
  registryStaleDays?: number | null;
  risk: Risk;
  source: PlanSource;
  /**
   * The signed target's SHA-256, for registry plans.
   */
  targetSha256?: string | null;
  /**
   * `package::tree_hash` of the staged tree; re-checked at commit.
   */
  treeHash: string;
  /**
   * `thirdPartyCode` first when present (spec §3.1), then the others.
   */
  warnings?: PlanWarning[];
}
/**
 * What an update changes in a plugin's grants. Order inside lists never matters.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PermissionDiff".
 */
export interface PermissionDiff {
  /**
   * Permissions whose key the old set did not have.
   */
  added: Permission[];
  /**
   * Keys the new set no longer has.
   */
  removed: Permission[];
  /**
   * The overall risk rose, or a kept permission's risk rose through a combination.
   */
  riskUp: boolean;
  /**
   * Permissions of one key where the new one grants more.
   */
  widened: Widening[];
}
/**
 * One permission an update widens: the same `perm`, granting more.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Widening".
 */
export interface Widening {
  from: Permission;
  to: Permission;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanLicence".
 */
export interface PlanLicence {
  class: LicenceClass;
  spdx: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanModel".
 */
export interface PlanModel {
  attribution?: string | null;
  bytes: number;
  id: string;
  licence: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlannedPermission".
 */
export interface PlannedPermission {
  permission: Permission;
  /**
   * Within the whole set (combinations count, spec §4.3).
   */
  risk: "low" | "medium" | "high";
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanPlugin".
 */
export interface PlanPlugin {
  /**
   * The id's name until manifests carry a display name.
   */
  displayName: string;
  id: string;
  kind: PluginKind;
  /**
   * The manifest's publisher name (shown to people, never journaled).
   */
  publisher: string;
  version: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanSource".
 */
export interface PlanSource {
  kind: SourceKind;
  registryId?: string | null;
}
/**
 * A warning shown with the plan: `thirdPartyCode {confined}`, `yanked`, `advisory` (from the
 * registry), `lookalikeId {similar}`, `copyleft {spdx}`, `licenceUnknown {spdx}`,
 * `registryStale {days}`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanWarning".
 */
export interface PlanWarning {
  code: string;
  details?: {
    [k: string]: unknown;
  };
}
/**
 * The installed version a plan is made against (from `control.db` in `finisherd`), and what a
 * commit returns.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "InstalledPlugin".
 */
export interface InstalledPlugin {
  /**
   * The package directory, relative to `<server-data>/plugins` (`/`-separated).
   */
  dir: string;
  id: string;
  kind: PluginKind;
  /**
   * The granted permissions.
   */
  permissions?: Permission[];
  registryId?: string | null;
  source: SourceKind;
  targetSha256?: string | null;
  treeHash: string;
  version: string;
}
/**
 * One violation, shaped as an error `{code, message, details}`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Invalid".
 */
export interface Invalid {
  code: string;
  details: {
    [k: string]: unknown;
  };
  message: string;
}
/**
 * A failed item of a batch job (`itemErrors`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ItemError".
 */
export interface ItemError {
  error: JobError;
  item: string;
}
/**
 * `openGroup`'s spec.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobGroupSpec".
 */
export interface JobGroupSpec {
  label: string;
  /**
   * The group's memory limit: the sum of its processes (Decision 20).
   */
  memoryBytes?: number;
  qos?: "normal" | "utility" | "background";
}
/**
 * What this host can enforce.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostCaps".
 */
export interface JobHostCaps {
  /**
   * Group memory limits are enforced by the kernel (cgroup `memory.max` or a Job).
   */
  cgroupMemory: boolean;
  killTree: JobHostKillTree;
  /**
   * Per-process limits are enforced (leaf Jobs or leaf cgroups).
   */
  leafLimits: boolean;
  /**
   * Warm workers can be granted per job (plan 7C Task 12): warm kinds run warm under an
   * enforced profile only then.
   */
  perJobGrants: boolean;
  profiles: JobHostProfiles;
  /**
   * The spawner's name (`none` unsandboxed).
   */
  sandbox: string;
  /**
   * Every profile is enforced.
   */
  sandboxed: boolean;
  watchdog: boolean;
}
/**
 * What each sandbox profile gets (plan 7C Decision 4).
 */
export interface JobHostProfiles {
  plugin: JobHostProfileState;
  worker: JobHostProfileState;
}
/**
 * `progress`. The runtime fills `job` ([`crate::worker::JobCtx::progress`]).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Progress".
 */
export interface Progress {
  done: number;
  job: string;
  phase?: string;
  total?: number;
  unit: ProgressUnit;
}
/**
 * `JobHost.start`'s options. Timers in milliseconds; unset ones take the defaults.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostOptions".
 */
export interface JobHostOptions {
  /**
   * Where `{ bundled }` programs resolve: the directory of `workersBinaryPath()`.
   */
  binaryDir: string;
  /**
   * Delegated cgroups where they are ours (Linux; default `auto`).
   */
  cgroups?: "auto" | "off";
  /**
   * Events waiting for the JS thread before the host's readers wait too (default 1,024).
   */
  eventQueue?: number;
  /**
   * The summed JSON bytes of those events (default 16 MiB; one larger event still goes
   * alone). `exit`, `spawned` and `groupEmpty` are not counted: a process has one of each.
   */
  eventQueueBytes?: number;
  /**
   * `<server-data>`: the sandbox spawner places each process's grants against it (plan 7C
   * Task 7). Without it, a profile that is enforced on this host refuses every spawn.
   */
  serverData?: string;
  timing?: JobHostTiming;
  /**
   * The Unix host watchdog (default on; ignored on Windows).
   */
  watchdog?: boolean;
}
/**
 * The host's timers in milliseconds; unset ones take the defaults.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostTiming".
 */
export interface JobHostTiming {
  exitDrainMs?: number;
  helloMs?: number;
  hungMs?: number;
  killWaitMs?: number;
  rssSampleMs?: number;
}
/**
 * [`JobHostProfileState`] per profile.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobHostProfiles".
 */
export interface JobHostProfiles1 {
  plugin: JobHostProfileState;
  worker: JobHostProfileState;
}
/**
 * A group memory rule (Decision 20).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobMemoryRule".
 */
export interface JobMemoryRule {
  capMiB?: number;
  fixedMiB?: number;
  ramFraction?: number;
}
/**
 * `in/snapshot.json` (plan 6A Decision 2).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSnapshot".
 */
export interface JobSnapshot {
  branch: string;
  project: Project;
  rev: number;
}
/**
 * A path the process may use (for a sandbox's rules).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnGrant".
 */
export interface JobSpawnGrant {
  mode: FileMode;
  path: string;
}
/**
 * A helper's `{kind, id}`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnHelper".
 */
export interface JobSpawnHelper {
  id: string;
  kind: HelperKind;
}
/**
 * One process to start. `env` is its whole environment: nothing else is inherited. `helper`
 * is required for a helper and refused for a worker.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobSpawnRequest".
 */
export interface JobSpawnRequest {
  args?: string[];
  cwd: string;
  env?: {
    [k: string]: string;
  };
  family: string;
  grants?: JobSpawnGrant[];
  helper?: JobSpawnHelper;
  label?: string;
  profile?: JobSpawnProfile;
  program: JobSpawnProgram;
  qos?: "normal" | "utility" | "background";
  role: JobSpawnRole;
}
/**
 * `watch`'s argument: `stallMs` without `begin`, `item` or `progress`; `timeoutMs` from now.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "JobWatch".
 */
export interface JobWatch {
  job: string;
  stallMs?: number;
  timeoutMs?: number;
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
 * One directory the catalog knows (Decision 27) as the walker holds it. A line of a scan
 * job's `in/dirs.ndjson` is `{dir, mtimeNs, listed}` (written by the indexer; `listed`
 * becomes `listed_gen` of the scan's generation) with these fields optional
 * ([`crate::walk::KnownDirs::from_ndjson`]):
 *
 * - `mtime_ns`: the mtime its last listing recorded (`DirDone.mtime_ns`); a poll re-lists the
 *   directory when it differs;
 * - `entries`: how many entries that listing held: a listing that drops from N > 0 to 0 is
 *   `DIR_EMPTY_SUSPECT` (Decision 25), the root's is `ROOT_EMPTY_SUSPECT` (Decision 18);
 * - `listed_gen`: the full-scan generation in which it was last listed completely; a
 *   resumed full scan of that generation skips its listing and still descends;
 * - `mount_root`: it was the root of a nested mount; when it no longer is, its subtree is
 *   `ROOT_UNMOUNTED` (Decision 17), never listed and never missing;
 * - `volume`: that nested mount's volume as last listed (`DirDone.volume`); a different
 *   volume there now (a different key its file ids and sample do not confirm,
 *   [`crate::volume::same_volume`]) is `ROOT_VOLUME_CHANGED`, also never listed and never
 *   missing.
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
  volume?: VolumeIdentity | null;
}
/**
 * The `library.batch` call's params.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryBatchCall".
 */
export interface LibraryBatchCall {
  /**
   * A plain file name inside the job's `out/`.
   */
  file: string;
  seq: number;
}
/**
 * One line of `in/files.ndjson`: a catalogued file to read (Decision 20).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryFileLine".
 */
export interface LibraryFileLine {
  expect: Expect;
  id: number;
  rel: RawRel;
}
/**
 * `library.hash` (Decision 38): as [`LibraryProbeParams`], without the extension filter.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryHashParams".
 */
export interface LibraryHashParams {
  files?: string;
  hydrate: boolean;
  items: string[];
  library: string;
  readBps?: number | null;
  root: string;
  volume: VolumeIdentity;
}
/**
 * What `library.hash` reports: one value per item (`blake3:<hex>`) went through `item`;
 * here only the counts.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryHashResult".
 */
export interface LibraryHashResult {
  changed: number;
  datalessPolicy: string;
  states: {
    [k: string]: number;
  };
}
/**
 * `library.probe` (Decision 34): items are catalog file ids (decimal strings); the
 * `in/files.ndjson` grant holds a [`FileLine`] for each, and only the lines named by `items`
 * run (the engine's `restrict` narrows `items`, not the file).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryProbeParams".
 */
export interface LibraryProbeParams {
  /**
   * Extensions to probe (lower case, no dot); absent probes everything (`kinds: all`).
   * Others are `skipped` with code [`KIND_FILTER`] (Decision 37).
   */
  extensions?: string[] | null;
  /**
   * The grant that holds the lines.
   */
  files?: string;
  /**
   * The library's setting: a placeholder is read only when `true` (Decision 6).
   */
  hydrate: boolean;
  items: string[];
  library: string;
  /**
   * The library's bandwidth budget for this job's slot, in bytes per second (Decision 36).
   */
  readBps?: number | null;
  root: string;
  volume: VolumeIdentity1;
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
 */
export interface VolumeIdentity1 {
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
/**
 * What `library.probe` reports: outcomes by state (the outcomes themselves went through
 * `library.probed`).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryProbeResult".
 */
export interface LibraryProbeResult {
  /**
   * Files whose outcome was `changed`: their directories need a rescan (Task 5).
   */
  changed: number;
  datalessPolicy: string;
  states: {
    [k: string]: number;
  };
}
/**
 * The `library.probed` call's params.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryProbedCall".
 */
export interface LibraryProbedCall {
  results: ProbeOutcome[];
}
/**
 * What `library.probed` reports for one file.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ProbeOutcome".
 */
export interface ProbeOutcome {
  /**
   * The error code of a state other than `present`.
   */
  code?: string | null;
  file: number;
  /**
   * `fp1`, whenever the file could be read that far, even if the probe failed.
   */
  fingerprint?: string | null;
  probe?: Probe | null;
  probeVersion: number;
  state: string;
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
  media?: MediaInfo;
  size: number;
  video?: VideoInfo | null;
}
/**
 * MP4/MOV: what the file is (sub-project 6 spec §6.3): display size and rotation, colour,
 * HDR, VFR, edit lists. In the `asset.probe` answer only: an `asset.add` never carries it
 * (spec §4.4).
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
 * `library.scan`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryScanParams".
 */
export interface LibraryScanParams {
  /**
   * `mode: dirs`: the directories to list.
   */
  dirs?: RawRel[] | null;
  excludes: ExcludeSpec;
  /**
   * The generation a full scan writes (`gen`; a word reserved in Rust 2024).
   */
  gen: number;
  library: string;
  mode: LibraryScanMode;
  /**
   * The library's canonical root: an absolute directory.
   */
  root: string;
  /**
   * Read threads; absent means 64 (local roots), the service passes 32 for network and
   * sync roots (Decision 9).
   */
  threads?: number | null;
  /**
   * Scaled deadlines. Test builds only: a release worker refuses the field.
   */
  timing?: Timing | null;
  /**
   * The library's stored volume identity, which the first stat is judged against
   * (Decision 17); absent for the first scan of a new library.
   */
  volume?: VolumeIdentity | null;
}
/**
 * How long the walker waits.
 *
 * - `first_stat`: the root's first stat of a walk, spin-up included (60 s); past it the
 *   root is `timeout` and the walk answers offline;
 * - `entry`: one step of a directory read (its open, a batch of names, one entry's stat;
 *   30 s); a read with no step for that long leaves the directory `unreadable` with code
 *   `TIMEOUT` and the walk goes on (its thread is left behind);
 * - `flush`: a batch holding anything is handed to the sink at least this often (5 s), so a
 *   slow walk still reports progress per directory.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Timing".
 */
export interface Timing {
  entry: number;
  firstStat: number;
  flush?: number;
}
/**
 * What a scan did (`WalkSummary`, with the identity it measured).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LibraryScanResult".
 */
export interface LibraryScanResult {
  batches: number;
  /**
   * The process's dataless policy, as the OS reports it (`off` on macOS, Decision 6).
   */
  datalessPolicy: string;
  entries: number;
  identity: VolumeIdentity2;
  incomplete: number;
  listed: number;
  skipped: number;
  /**
   * Read threads still stuck in an abandoned read when the walk ended.
   */
  stuck: number;
  timeouts: number;
  unchanged: number;
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
 */
export interface VolumeIdentity2 {
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
/**
 * A plugin in search results: its newest version that is not yanked.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Listing".
 */
export interface Listing {
  id: string;
  kind: PluginKind;
  latest: string;
  license: string;
  registryId: string;
  /**
   * Versions offered to new installs (not yanked), oldest first.
   */
  versions: string[];
}
/**
 * One rendered frame of a `media.look`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LookFrame".
 */
export interface LookFrame {
  at: Rational;
  /**
   * Relative to `out/` in a worker's result; the frame's final absolute path in the job's
   * stored result, once the kind's `onSucceeded` has moved it.
   */
  file: string;
  height: number;
  /**
   * The plan's BLAKE3 (lowercase hex).
   */
  plan: string;
  width: number;
}
/**
 * `media.look` v1: frames of `timeline` at each time in `at`, as PNGs in `out/`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LookParams".
 */
export interface LookParams {
  /**
   * Timeline times in seconds, 1 to [`LOOK_MAX_TIMES`] of them.
   */
  at: Rational[];
  /**
   * The branch of the snapshot (an id, a name or `<name>#<id>`).
   */
  branch?: string | null;
  /**
   * Where the kind's `onSucceeded` moves the frames: absolute, or relative to the project
   * folder; `workspace/shared/look/` when absent. Never inside `.finisher/` or `assets/`
   * (`EXPORT_DESTINATION`). The worker ignores it.
   */
  outDir?: string | null;
  /**
   * The stem of the files written: `out/<outName>-<i>.png`. `[A-Za-z0-9_-]`, 1 to
   * [`OUT_NAME_MAX`] characters.
   */
  outName: string;
  /**
   * One sample per time in `at` (spec §3.3: every time for `media.look`); a plan the worker
   * evaluates differently is `PLAN_MISMATCH`. A submitter leaves them out: the kind's
   * `prepare` computes them from the snapshot it writes and hands them to the worker as
   * `start.job.extra.planSamples` (and in `in/params.json`, with the rest of these params,
   * for `--replay`).
   */
  planSamples?: PlanSample[];
  /**
   * The project the look is of. Only the host reads it (the kind's `dedupe` and `prepare`);
   * the worker works from the job directory.
   */
  projectId?: string | null;
  /**
   * The branch's rev the submitter saw: part of the job's identity (`dedupe`). `prepare`
   * snapshots the head it finds, and `in/snapshot.json` says which rev that was.
   */
  rev?: number | null;
  size?: Size2;
  timeline: string;
}
/**
 * The BLAKE3 of a frame plan at one time (plan 6A Decision 4): lowercase hex of the plan's
 * compact JSON, as `fin-core`'s eval corpus hashes it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PlanSample".
 */
export interface PlanSample {
  at: Rational;
  blake3: string;
}
/**
 * The frame size; the timeline's resolution when absent. Each side 1 to
 * [`LOOK_MAX_SIDE`].
 */
export interface Size2 {
  height: number;
  width: number;
}
/**
 * What `media.look` v1 answers.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LookResult".
 */
export interface LookResult {
  adapter: GpuAdapter;
  /**
   * In `at` order.
   */
  frames: LookFrame[];
  /**
   * Every frame's, in `at` order, bottom layer first within a frame.
   */
  skipped: LookSkipped[];
  stats: LookStats;
}
/**
 * A layer, or an effect or mask of a drawn one, that a frame left out (spec §2.7).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LookSkipped".
 */
export interface LookSkipped {
  at: Rational;
  /**
   * `CODEC_UNAVAILABLE`, `ORIGINALS_MISSING`, `LAYER_UNAVAILABLE`, `MEDIA_CORRUPT`, …
   */
  code: string;
  details: unknown;
  item: string;
  kind: ItemKind;
}
/**
 * What a warm worker reused (spec §3.2 item 1: caches keyed by content).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "LookStats".
 */
export interface LookStats {
  /**
   * The GPU device and pipelines were made by an earlier job of this worker.
   */
  deviceReused: boolean;
  media?: JobMedia;
  /**
   * Sources opened (container parsed, decoder made) by this job.
   */
  sourcesOpened: number;
  /**
   * Sources an earlier job of this worker had open.
   */
  sourcesReused: number;
  /**
   * Jobs this worker ran, this one included.
   */
  workerJobs: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Manifest".
 */
export interface Manifest {
  contributes?: Contributes;
  /**
   * The host API range this plugin works with (`^1.0`).
   */
  hostApi: string;
  id: PluginId;
  kind: PluginKind;
  /**
   * An SPDX expression.
   */
  license: string;
  locales?: string[];
  /**
   * Always 1.
   */
  manifestVersion: number;
  /**
   * The package directory holding `<locale>.json` ICU catalogues.
   */
  messages?: string | null;
  models?: ModelRef[];
  permissions?: Permission[];
  /**
   * Code plugins only.
   */
  platforms?: Platform[];
  publisher: Publisher;
  /**
   * Code plugins only.
   */
  runtime?: Runtime | null;
  /**
   * Semver 2.0 without build metadata, immutable once published.
   */
  version: string;
}
/**
 * A model a code plugin needs (spec §8.2); never inside the package.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ModelRef".
 */
export interface ModelRef {
  attribution?: string | null;
  bytes: number;
  id: string;
  license: string;
  provenance?: string | null;
  /**
   * A message key in the `en` catalogue.
   */
  purpose: string;
  sha256: string;
  url: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Publisher".
 */
export interface Publisher {
  name: string;
  url?: string | null;
}
/**
 * One asset of the manifest.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ManifestAsset".
 */
export interface ManifestAsset {
  asset: string;
  fingerprint: string;
  kind: AssetKind;
  /**
   * The file on this machine (plan 6A Decision 5: the model's stored path), `null` when it
   * is missing.
   */
  original?: OriginalFile | null;
}
/**
 * Font bytes for captions (plan 6A Decision 3): the server's font cache, read-only, then the
 * project's font asset files by SHA-256.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ManifestFonts".
 */
export interface ManifestFonts {
  /**
   * `<server-data>/fonts`.
   */
  cache?: string;
  files?: FontFileRef[];
}
/**
 * What `media.capabilities` v1 answers (plan 6A Task 13): codecs, the GPU a render would use
 * (`null` when no adapter, not even a software one, can be opened) and the worker's versions.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MediaCapabilities".
 */
export interface MediaCapabilities {
  codecs: CodecCapabilities;
  gpu?: GpuAdapter | null;
  worker: WorkerInfo;
}
/**
 * The worker binary's versions.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WorkerInfo".
 */
export interface WorkerInfo {
  /**
   * [`RENDER_VERSION`](crate::RENDER_VERSION): part of every cache key (spec §2.5).
   */
  renderVersion: number;
  /**
   * The `fin-workers` crate's version.
   */
  version: string;
}
/**
 * What a media file is (spec §6.3).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "MediaInfo".
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
 * The answer of [`Store::merge`](crate::Store::merge): merge group `merge_id`'s records on
 * the target, in order (or planned and rolled back, `dry_run`); `merged`: the source txIds
 * they copy; `skipped`: the candidates that conflict and were left on the source (with
 * `skipConflicting`; they are offered again by the next merge); `excluded`: source records
 * never offered (`UNDOES_TARGET_RECORDS`); `source_state`: the source branch's state after
 * the merge; `origins`: `merged` with the records their `origin` chains lead to (a copy of a
 * copy), first seen first (7A Decision 27). A retry of a committed merge answers from the
 * log: its records and their origins, `skipped` and `excluded` empty, `dry_run` false.
 * Nothing left to merge is an empty answer, not an error.
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
  origins?: string[];
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
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginData".
 */
export interface PluginData {
  dir: string;
  /**
   * `fs pluginData readWrite` (otherwise read-only).
   */
  write: boolean;
}
/**
 * Everything a registry signs about one plugin.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginInfo".
 */
export interface PluginInfo {
  /**
   * In-scope advisories that apply to at least one listed version.
   */
  advisories: SignedAdvisory[];
  id: string;
  registryId: string;
  /**
   * Every version, yanked ones included, oldest first.
   */
  versions: VersionInfo[];
}
/**
 * An advisory with the role that signed it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SignedAdvisory".
 */
export interface SignedAdvisory {
  action: AdvisoryAction;
  id: string;
  /**
   * The plugin id, `@publisher/name`.
   */
  plugin: string;
  severity: Severity;
  /**
   * `targets` for registry-wide advisories, else the publisher.
   */
  signedBy: string;
  summary: string;
  url?: string | null;
  /**
   * A semver range of affected versions.
   */
  versions: string;
}
/**
 * One published version.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "VersionInfo".
 */
export interface VersionInfo {
  bytesUnpacked: number;
  hostApi: string;
  kind: PluginKind;
  length: number;
  license: string;
  manifestSha256: string;
  permissionsDigest: string;
  platforms: Platform[];
  sha256: string;
  /**
   * The target name, its signed length and SHA-256.
   */
  target: string;
  version: string;
  yanked: boolean;
}
/**
 * A code plugin's process (sp8 §16.1, plan 7C Decisions 9 and 21).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "PluginInput".
 */
export interface PluginInput {
  /**
   * The call's or job's directory: `in/` is read-only, `out/` writable.
   */
  callDir: string;
  /**
   * GPU device nodes; empty without the `gpu` grant. Never `/dev/dri/card*`.
   */
  gpu?: DeviceRule[];
  home: string;
  /**
   * The manifest's declared memory need (default 2 GiB, at most 16 GiB; sp8 §3.3).
   */
  memoryBytes?: number | null;
  /**
   * The model files the manifest names.
   */
  models?: string[];
  /**
   * ONNX Runtime's library.
   */
  onnxRuntime?: string | null;
  /**
   * The per-project plugin data directory, when the manifest is granted `fs pluginData`.
   */
  pluginData?: PluginData | null;
  /**
   * The runtime: finisherd's Node or the Electron framework, for `node` plugins.
   */
  runtime?: string[];
  serverData: string;
  /**
   * The plugin's package directory inside `<serverData>/plugins/store/` (read-only).
   */
  storeDir: string;
  /**
   * System libraries and frameworks (`/usr/lib`, `/lib`, `/System/Library`, `System32`).
   */
  system?: string[];
  /**
   * A private temporary directory.
   */
  tmp: string;
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
  /**
   * The agent-trace store's size cap in MiB, 64–65536 (default 2048).
   */
  tracesMaxMiB?: number | null;
  /**
   * The agent-trace store's age cap in days, 1–365 (default 90; sp7 §12.1).
   */
  tracesRetentionDays?: number | null;
  training?: Switch | null;
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
 * What a refresh found.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RefreshReport".
 */
export interface RefreshReport {
  /**
   * Whether the trusted set changed.
   */
  changed: boolean;
  rootVersion: number;
  /**
   * Roots newly trusted by this refresh.
   */
  rootsAdded: number;
  snapshotVersion: number;
  /**
   * Whole days the metadata is past its earliest expiry, when it is.
   */
  staleDays?: number | null;
  targetsVersion: number;
  timestampVersion: number;
  warnings: Warning[];
}
/**
 * A warning `{code, details}`, e.g. `REGISTRY_STALE {days, maxStaleDays, role}`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "Warning".
 */
export interface Warning {
  code: string;
  details: {
    [k: string]: unknown;
  };
}
/**
 * `registry.list`'s answer.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RegistryList".
 */
export interface RegistryList {
  problems: RegistryProblem[];
  registries: RegistryStatus[];
}
/**
 * A registry that could not be opened (its config or trusted set is damaged); the others
 * are still listed.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RegistryProblem".
 */
export interface RegistryProblem {
  code: string;
  id: string;
  message: string;
}
/**
 * One added registry (spec §5.1: `registry.json` and the trusted set).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RegistryStatus".
 */
export interface RegistryStatus {
  /**
   * Advisories in scope of the trusted set.
   */
  advisories: number;
  /**
   * `relaxed` for local folders.
   */
  freshness: string;
  id: string;
  /**
   * `local`: a folder (a git registry is a folder the user pulls).
   */
  kind: string;
  /**
   * Seconds since the epoch of the last successful refresh, `None` before the first.
   */
  lastRefreshAt: number;
  /**
   * `maxStaleDays` from the signed root (default 7, at most 90).
   */
  maxStaleDays: number;
  path: string;
  /**
   * Plugins listed (with at least one version), yanked-only ones included.
   */
  plugins: number;
  /**
   * The root this registry is trusted through (also the name of its trust directory).
   */
  rootSha256: string;
  /**
   * Whole days past the trusted metadata's earliest expiry, `None` while fresh.
   */
  staleDays: number;
  trusted: TrustedVersions;
}
/**
 * The versions of the trusted metadata roles; a role not yet fetched has none.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TrustedVersions".
 */
export interface TrustedVersions {
  root: number;
  snapshot: number;
  targets: number;
  timestamp: number;
}
/**
 * `in/manifest.json` (spec §4.2): built by `fin-node` from the snapshot's asset table,
 * never by the worker.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "RenderManifest".
 */
export interface RenderManifest {
  /**
   * Every asset the snapshot references, once.
   */
  assets: ManifestAsset[];
  fonts?: ManifestFonts1;
  /**
   * [`MANIFEST_VERSION`].
   */
  version: number;
}
/**
 * Font bytes for captions (plan 6A Decision 3): the server's font cache, read-only, then the
 * project's font asset files by SHA-256.
 */
export interface ManifestFonts1 {
  /**
   * `<server-data>/fonts`.
   */
  cache?: string;
  files?: FontFileRef[];
}
/**
 * What `fin-sandbox-exec --probe` prints.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxLauncherProbe".
 */
export interface SandboxLauncherProbe {
  namespaces: SandboxNamespaces;
}
/**
 * Every profile's status on this host (`sandboxStatus()` in fin-node).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SandboxStatus".
 */
export interface SandboxStatus {
  agent: SandboxMechanism;
  hook: SandboxMechanism;
  os: SandboxOs;
  plugin: SandboxMechanism;
  worker: SandboxMechanism;
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
 * A requested self-sandbox: a profile name and its arguments.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "SelfSandbox".
 */
export interface SelfSandbox {
  args?: {
    [k: string]: unknown;
  };
  profile: string;
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
export interface Size3 {
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
 * A package target's signed custom metadata (`custom` in its publisher's delegated role): one
 * type for the editor that signs it, the TUF client that reads it and the plan that checks it.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TargetCustom".
 */
export interface TargetCustom {
  bytesUnpacked: number;
  hostApi: string;
  id: string;
  kind: PluginKind;
  license: string;
  /**
   * Lowercase hex SHA-256 of `finisher-plugin.json`.
   */
  manifestSha256: string;
  /**
   * [`permissions_digest`] of the manifest's permissions.
   */
  permissionsDigest: string;
  platforms?: Platform[];
  version: string;
  yanked?: boolean;
}
/**
 * `test.badHello` (family `testbad`): its `hello` claims another envelope.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestBadHelloParams".
 */
export interface TestBadHelloParams {}
/**
 * `test.call`: one host call; returns the reply.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCallParams".
 */
export interface TestCallParams {
  method: string;
  params?: {
    [k: string]: unknown;
  };
}
/**
 * `test.checkpoint` (resumable): a checkpoint per step; crashes on attempt 1 at
 * `crashAt`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCheckpointParams".
 */
export interface TestCheckpointParams {
  crashAt?: number;
  stepMs?: number;
  steps: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCheckpointResult".
 */
export interface TestCheckpointResult {
  resumedFrom: number;
  steps: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCountResult".
 */
export interface TestCountResult {
  sent: number;
}
/**
 * `test.crash` (batch): `begin`/`item` per item; crashes inside a poison item (before
 * its `begin` with `between`), or inside the second item of any run of two or more with
 * `crashOnSecond`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCrashParams".
 */
export interface TestCrashParams {
  between?: boolean;
  crashOnSecond?: boolean;
  how: TestCrashHow;
  /**
   * Milliseconds per item.
   */
  itemMs?: number;
  items: string[];
  poison?: string[];
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestCrashResult".
 */
export interface TestCrashResult {
  done: string[];
}
/**
 * `test.diag`: one `diag` of kind `test.diag`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestDiagParams".
 */
export interface TestDiagParams {}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestDiagResult".
 */
export interface TestDiagResult {
  sent: boolean;
}
/**
 * `test.echo`: returns `value`, plus what `report` asks about the process.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestEchoParams".
 */
export interface TestEchoParams {
  report?: TestEchoReport[];
  value: unknown;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestEchoResult".
 */
export interface TestEchoResult {
  args?: string[];
  cgroup?: string;
  cwd?: string;
  env?: {
    [k: string]: string;
  };
  fds?: number[];
  nice?: number;
  pdeathsig?: number;
  pid: number;
  value: unknown;
}
/**
 * `test.exitAfterResult`: writes `result` and exits 0 at once.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestExitAfterResultParams".
 */
export interface TestExitAfterResultParams {}
/**
 * `test.fail`: answers `error {code, retry}` on every attempt up to `failUntil` (every
 * attempt without it), then succeeds with the attempt number.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestFailParams".
 */
export interface TestFailParams {
  code: string;
  failUntil?: number;
  /**
   * How the engine may retry after a worker `error` (Decision 31).
   */
  retry?: "never" | "backoff" | "fresh";
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestFailResult".
 */
export interface TestFailResult {
  attempt: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestFileReport".
 */
export interface TestFileReport {
  bytes: number;
  /**
   * FNV-1a 64 of the content after the run, as hex.
   */
  hash: string;
  mode: FileMode;
  name: string;
}
/**
 * `test.files`: writes a pattern into every `write` file, reads every `read` file, and
 * leaves a file in each `write` directory; fails (`retry: backoff`) on attempt
 * `failOnAttempt`.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestFilesParams".
 */
export interface TestFilesParams {
  failOnAttempt?: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestFilesResult".
 */
export interface TestFilesResult {
  files: TestFileReport[];
  outEmpty?: boolean;
}
/**
 * `test.garbage`: commits the protocol violation `what`, then waits to be killed.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestGarbageParams".
 */
export interface TestGarbageParams {
  what: TestGarbage;
}
/**
 * `test.hang`: stops `alive`, stops reading stdin (EOF unseen, the pipe fills) and blocks.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestHangParams".
 */
export interface TestHangParams {}
/**
 * `test.helper`: the job of a test helper (`helper` picks its behaviour).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestHelperParams".
 */
export interface TestHelperParams {
  delayMs?: number;
  /**
   * The test helper's behaviour (`fin-workers --helper test:<behaviour>`).
   */
  helper?: "ok" | "exitEarly" | "hang" | "spawn" | "call";
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestHelperResult".
 */
export interface TestHelperResult {
  helpers: string[];
}
/**
 * `test.oom`: allocates and touches `mib` MiB, maps `mapFile` (if given) read-only and
 * reads every page of it, writes `1` to `readyFile` (if given), then holds.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestOomParams".
 */
export interface TestOomParams {
  /**
   * Hold the memory this long before answering.
   */
  holdMs?: number;
  /**
   * A file (mapped name, or under the job directory) to map: file-backed pages.
   */
  mapFile?: string;
  mib: number;
  /**
   * A file to write once the memory is held.
   */
  readyFile?: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestOomResult".
 */
export interface TestOomResult {
  mib: number;
}
/**
 * `test.pluginMinor` (family `testplugin`): `hello` with minor 1 and no envelope, then a
 * message of an unknown type with an `id`. `key` is its warm key.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestPluginMinorParams".
 */
export interface TestPluginMinorParams {
  key?: string;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestPluginMinorResult".
 */
export interface TestPluginMinorResult {
  key?: string;
  pid: number;
  reply: unknown;
  served: number;
}
/**
 * `test.progressFlood`: `n` progress messages as fast as it can; with `stderrBytes`, `n`
 * stderr lines of that many bytes (`flood <i> ` then padding) instead.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestProgressFloodParams".
 */
export interface TestProgressFloodParams {
  n: number;
  stderrBytes?: number;
}
/**
 * `test.sleep`: sleeps `ms` (checking cancel), with optional progress.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestSleepParams".
 */
export interface TestSleepParams {
  ms: number;
  progressEveryMs?: number;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestSleepResult".
 */
export interface TestSleepResult {
  sleptMs: number;
}
/**
 * `test.spawn` (breaks the no-process rule on purpose): double-forks a sleeping
 * grandchild, writes the pids to the mapped file `out`, then sleeps.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestSpawnParams".
 */
export interface TestSpawnParams {
  detach: TestDetach;
  /**
   * How long the grandchild sleeps (default 600 s).
   */
  grandchildMs?: number;
  out: string;
}
/**
 * What `test.spawn` writes to its `out` file (JSON, then zero padding).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestSpawnPids".
 */
export interface TestSpawnPids {
  grandchild: number;
  mid: number;
  worker: number;
}
/**
 * `test.stall`: keeps `alive`, never reports progress, ends at cancel.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestStallParams".
 */
export interface TestStallParams {}
/**
 * `test.warm` (warm): returns `value` with the pid and how many jobs the process served.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestWarmParams".
 */
export interface TestWarmParams {
  delayMs?: number;
  /**
   * A job-directory file kept open past `ready` (breaks the warm contract on purpose).
   */
  holdFile?: string;
  /**
   * Paths the worker tries to read (plan 7C Task 12): each answered with the errno of
   * the attempt, `null` when it read. A path starting with `!` is renamed instead (to
   * `<path>.moved`, and back when it worked): what replacing it with a link would need.
   */
  probe?: string[];
  value: unknown;
}
/**
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "TestWarmResult".
 */
export interface TestWarmResult {
  pid: number;
  /**
   * One errno (or `null`) per `probe` path, in order.
   */
  probes?: (number | null)[];
  served: number;
  value: unknown;
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
 * `validateManifest`: the manifest when valid, else every violation.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "ValidationReport".
 */
export interface ValidationReport {
  errors: Invalid[];
  manifest?: Manifest | null;
  ok: boolean;
}
/**
 * A media worker (spec §8.10; sp6 §3.5; plan 7C Decisions 8 and 19).
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WorkerInput".
 */
export interface WorkerInput {
  /**
   * The font and media caches (read-only).
   */
  caches?: string[];
  /**
   * GPU device nodes and driver files (sub-project 6 adds them, 6B-1).
   */
  devices?: DeviceRule[];
  scope: WorkerScope;
  serverData: string;
  /**
   * A private temporary directory (read and write).
   */
  tmp: string;
}
/**
 * The branches a write may change, by branch id (`br<n>`, `main`). The default guards
 * nothing.
 *
 * This interface was referenced by `FinCore`'s JSON-Schema
 * via the `definition` "WriteGuard".
 */
export interface WriteGuard {
  /**
   * When set, the only branches a write may change (`BRANCH_NOT_ALLOWED` otherwise). Absent
   * means no allow-list; `null` is refused, so a sender's bug cannot open the guard.
   */
  allowBranches?: string[];
  /**
   * The credential the write is made for (`agent:<id>`, `mcp:<id>`), refused once it has
   * been ended ([`end_credential`]); absent for `local` and `console`. `null` is refused.
   */
  credential?: string;
  /**
   * Branches no write may change (`BRANCH_PROTECTED`).
   */
  denyBranches?: string[];
}
