//! Primitive edit operations, one module per op (schema, validate, apply, invert, rebase).
//!
//! Client ops: `asset.add`, `clip.insert`, `clip.trim`, `clip.delete`. Internal ops exist only
//! as inverses (`asset.remove`, `clip.remove`, `clip.restore`); clients sending them get
//! `INVALID_OP`.
//!
//! Every clip op records exactly one `Shift` in the diff (possibly `by: 0`) and asset ops record
//! none. `rebase` relies on that to pair a committed transaction's shifts with its ops.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{AudioInfo, Clip, Project, VideoInfo};
use crate::rebase::Conflict;
use crate::time::Rational;
use crate::tx::Diff;

mod asset_add;
mod asset_remove;
mod clip_delete;
mod clip_insert;
mod clip_remove;
mod clip_restore;
mod clip_trim;
pub(crate) mod util;

/// A primitive edit operation, tagged by `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all_fields = "camelCase")]
pub enum Op {
    /// Adds a media asset. `id` is a `$name` placeholder bound to a new `a<n>` id.
    #[serde(rename = "asset.add")]
    AssetAdd {
        id: String,
        path: String,
        // Asset length in seconds; must be positive.
        duration: Rational,
        video: Option<VideoInfo>,
        audio: Option<AudioInfo>,
        fingerprint: String,
    },
    /// Inserts a clip at `at` on `track`, rippling later clips by `duration`. `id` is a `$name`
    /// placeholder; `asset` is an asset id or a placeholder bound earlier in the transaction.
    #[serde(rename = "clip.insert")]
    ClipInsert {
        id: String,
        track: String,
        // Timeline position in seconds: a clip boundary, a gap, or at/after the track end.
        at: Rational,
        asset: String,
        // Offset into the asset, in seconds.
        source_in: Rational,
        // Clip length in seconds; at least one frame.
        duration: Rational,
    },
    /// Moves one edge of a clip by `delta` seconds, rippling later clips on the track.
    #[serde(rename = "clip.trim")]
    ClipTrim {
        clip: String,
        edge: Edge,
        // Seconds; positive moves the edge later.
        delta: Rational,
    },
    /// Removes a clip and closes the gap (ripple).
    #[serde(rename = "clip.delete")]
    ClipDelete { clip: String },
    /// Internal (inverse of `asset.add`): removes an unused asset.
    #[serde(rename = "asset.remove")]
    AssetRemove { id: String },
    /// Internal (inverse of `clip.insert`): removes a clip, rippling when `ripple`.
    #[serde(rename = "clip.remove")]
    ClipRemove { id: String, ripple: bool },
    /// Internal (inverse of `clip.delete`/`clip.remove`): puts `clip` back at its `start`,
    /// rippling later clips by its duration when `ripple`.
    #[serde(rename = "clip.restore")]
    ClipRestore {
        clip: Clip,
        track: String,
        #[serde(default = "yes")]
        ripple: bool,
    },
}

fn yes() -> bool {
    true
}

/// Which edge of a clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Start,
    End,
}

impl Op {
    /// The wire name (`"clip.insert"`, ...).
    pub fn name(&self) -> &'static str {
        match self {
            Op::AssetAdd { .. } => "asset.add",
            Op::ClipInsert { .. } => "clip.insert",
            Op::ClipTrim { .. } => "clip.trim",
            Op::ClipDelete { .. } => "clip.delete",
            Op::AssetRemove { .. } => "asset.remove",
            Op::ClipRemove { .. } => "clip.remove",
            Op::ClipRestore { .. } => "clip.restore",
        }
    }

    /// Internal ops are produced as inverses and never accepted from clients.
    pub fn is_internal(&self) -> bool {
        matches!(self, Op::AssetRemove { .. } | Op::ClipRemove { .. } | Op::ClipRestore { .. })
    }

    /// Clip ops record exactly one `Shift` when applied; asset ops record none.
    pub(crate) fn records_shift(&self) -> bool {
        !matches!(self, Op::AssetAdd { .. } | Op::AssetRemove { .. })
    }

    /// Replaces `$name` references with their bound ids (unbound ones are left as they are).
    pub(crate) fn resolved(&self, bindings: &BTreeMap<String, String>) -> Op {
        let r = |s: &String| bindings.get(s).cloned().unwrap_or_else(|| s.clone());
        let mut op = self.clone();
        match &mut op {
            Op::AssetAdd { id, .. } | Op::AssetRemove { id } | Op::ClipRemove { id, .. } => *id = r(id),
            Op::ClipInsert { id, track, asset, .. } => {
                *id = r(id);
                *track = r(track);
                *asset = r(asset);
            }
            Op::ClipTrim { clip, .. } | Op::ClipDelete { clip } => *clip = r(clip),
            Op::ClipRestore { clip, track, .. } => {
                clip.id = r(&clip.id);
                clip.asset = r(&clip.asset);
                *track = r(track);
            }
        }
        op
    }
}

/// `$name` placeholders bound during one transaction, in op order.
pub(crate) struct Bindings {
    pub(crate) map: BTreeMap<String, String>,
    internal: bool,
}

impl Bindings {
    pub(crate) fn new(internal: bool) -> Self {
        Bindings { map: BTreeMap::new(), internal }
    }

    /// A reference: `$name` resolves through the bindings, anything else is a literal id.
    pub(crate) fn resolve(&self, reference: &str) -> Result<String, CoreError> {
        if !reference.starts_with('$') {
            return Ok(reference.to_string());
        }
        self.map.get(reference).cloned().ok_or_else(|| {
            CoreError::new("UNBOUND_PLACEHOLDER", format!("placeholder {reference} is not bound by an earlier op"))
                .with_details(json!({ "placeholder": reference }))
                .with_fix("create the entity earlier in the same transaction, or use an existing id")
        })
    }

    /// Checks that `id` can name a new entity without allocating anything.
    pub(crate) fn check_new(&self, project: &Project, id: &str, kind: IdKind) -> Result<(), CoreError> {
        if let Some(name) = id.strip_prefix('$') {
            if name.is_empty() {
                return Err(CoreError::new("INVALID_OP", "placeholder needs a name after `$`")
                    .with_details(json!({ "id": id }))
                    .with_fix("use a placeholder like $clip1"));
            }
            if self.map.contains_key(id) {
                return Err(CoreError::new("DUPLICATE_PLACEHOLDER", format!("placeholder {id} is bound twice"))
                    .with_details(json!({ "placeholder": id }))
                    .with_fix("give every new entity in the transaction its own placeholder"));
            }
            return Ok(());
        }
        if !self.internal {
            return Err(CoreError::new("INVALID_OP", "new entities are named with `$name` placeholders")
                .with_details(json!({ "id": id }))
                .with_fix(format!("use a placeholder such as ${id}; the server allocates the id")));
        }
        if util::exists(project, kind, id) {
            return Err(CoreError::new("DUPLICATE_ID", format!("id {id} already exists"))
                .with_details(json!({ "id": id })));
        }
        Ok(())
    }

    /// Names a new entity: binds a placeholder to a freshly allocated id, or keeps a literal id
    /// (internal mode only). Call `check_new` first; this is the only allocating step.
    pub(crate) fn create(
        &mut self,
        project: &Project,
        id: &str,
        kind: IdKind,
        ids: &mut dyn IdAlloc,
    ) -> Result<String, CoreError> {
        self.check_new(project, id, kind)?;
        if !id.starts_with('$') {
            return Ok(id.to_string());
        }
        let new = ids.next(kind);
        if util::exists(project, kind, &new) {
            return Err(CoreError::new("DUPLICATE_ID", format!("allocated id {new} already exists"))
                .with_details(json!({ "id": new })));
        }
        self.map.insert(id.to_string(), new.clone());
        Ok(new)
    }
}

/// Applies one op to `project`, appending to `diff`; returns the op's inverse ops (in order).
pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut Diff,
) -> Result<Vec<Op>, CoreError> {
    match op {
        Op::AssetAdd { .. } => asset_add::apply(project, op, bindings, ids, diff),
        Op::ClipInsert { .. } => clip_insert::apply(project, op, bindings, ids, diff),
        Op::ClipTrim { .. } => clip_trim::apply(project, op, bindings, ids, diff),
        Op::ClipDelete { .. } => clip_delete::apply(project, op, bindings, ids, diff),
        Op::AssetRemove { .. } => asset_remove::apply(project, op, bindings, ids, diff),
        Op::ClipRemove { .. } => clip_remove::apply(project, op, bindings, ids, diff),
        Op::ClipRestore { .. } => clip_restore::apply(project, op, bindings, ids, diff),
    }
}

/// Rebases `op` over one committed op `over` whose own diff (its single shift, if any) is
/// `over_diff`. `over` must already have its placeholders resolved.
pub(crate) fn rebase(op: &Op, over: &Op, over_diff: &Diff) -> Result<Op, Conflict> {
    match op {
        Op::AssetAdd { .. } => asset_add::rebase(op, over, over_diff),
        Op::ClipInsert { .. } => clip_insert::rebase(op, over, over_diff),
        Op::ClipTrim { .. } => clip_trim::rebase(op, over, over_diff),
        Op::ClipDelete { .. } => clip_delete::rebase(op, over, over_diff),
        Op::AssetRemove { .. } => asset_remove::rebase(op, over, over_diff),
        Op::ClipRemove { .. } => clip_remove::rebase(op, over, over_diff),
        Op::ClipRestore { .. } => clip_restore::rebase(op, over, over_diff),
    }
}
