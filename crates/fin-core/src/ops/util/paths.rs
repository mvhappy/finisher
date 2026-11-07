//! The effect ids inside parameter paths (plan 4C Decision 24): `effects.<fx>.<param>` names
//! effect `fx`, in a `param.set`/`keyframe.*` path and in the keys of a keyframe map (an
//! `effect.restore`'s, and a restored item's). [`crate::ops::Op::ids`] lists them and
//! [`crate::ops::Op::try_for_each_ref`] rewrites them with the op's other ids, so a recovered
//! record re-run under fresh ids follows the effect it named, and a placeholder bound by an
//! earlier `effect.add` resolves (`effects.$b.radius`).

use std::collections::BTreeMap;

use crate::error::CoreError;
use crate::model::{Keyframe, Track};
use crate::ops::{Bindings, Op};
use crate::params::effect_path;

/// A place an op names effect ids inside parameter paths.
pub(crate) enum PathsMut<'a> {
    /// One path.
    Path(&'a mut String),
    /// The keys of a keyframe map.
    Keys(&'a mut BTreeMap<String, Vec<Keyframe>>),
}

/// Every place `op` carries parameter paths. Ops not listed carry none (one central
/// list, like the transition pass's, so ops added elsewhere need no stub).
pub(crate) fn paths_mut(op: &mut Op) -> Vec<PathsMut<'_>> {
    match op {
        Op::ParamSet(s) => vec![PathsMut::Path(&mut s.path)],
        Op::KeyframeSet(k) => vec![PathsMut::Path(&mut k.path)],
        Op::KeyframeRemove(k) => vec![PathsMut::Path(&mut k.path)],
        Op::EffectRestore(r) => vec![PathsMut::Keys(&mut r.keyframes)],
        Op::ItemRestore(r) => vec![PathsMut::Keys(&mut r.item.common.keyframes)],
        Op::TrackRestore(r) => track_keys(&mut r.track),
        Op::TimelineRestore(r) => r.timeline.tracks.iter_mut().flat_map(track_keys).collect(),
        _ => Vec::new(),
    }
}

fn track_keys(track: &mut Track) -> Vec<PathsMut<'_>> {
    track
        .items
        .iter_mut()
        .map(|i| PathsMut::Keys(&mut i.common.keyframes))
        .collect()
}

/// The effect ids inside `op`'s paths (what [`paths_mut`] reaches), in that order.
pub(crate) fn path_ids(op: &Op) -> Vec<&str> {
    let paths: Vec<&str> = match op {
        Op::ParamSet(s) => vec![&s.path],
        Op::KeyframeSet(k) => vec![&k.path],
        Op::KeyframeRemove(k) => vec![&k.path],
        Op::EffectRestore(r) => r.keyframes.keys().map(String::as_str).collect(),
        Op::ItemRestore(r) => r.item.common.keyframes.keys().map(String::as_str).collect(),
        Op::TrackRestore(r) => keys_of(std::slice::from_ref(&r.track)),
        Op::TimelineRestore(r) => keys_of(&r.timeline.tracks),
        _ => Vec::new(),
    };
    paths
        .into_iter()
        .filter_map(|p| effect_path(p).map(|(fx, _)| fx))
        .collect()
}

fn keys_of(tracks: &[Track]) -> Vec<&str> {
    tracks
        .iter()
        .flat_map(|t| &t.items)
        .flat_map(|i| i.common.keyframes.keys().map(String::as_str))
        .collect()
}

/// `path` with its effect segment passed through `f`: `Some` new path when `f` changed it.
fn rewrite<E>(
    path: &str,
    f: &mut impl FnMut(&mut String) -> Result<(), E>,
) -> Result<Option<String>, E> {
    let Some((fx, rest)) = effect_path(path) else {
        return Ok(None);
    };
    let mut seg = fx.to_string();
    f(&mut seg)?;
    Ok((seg != fx).then(|| format!("effects.{seg}.{rest}")))
}

impl PathsMut<'_> {
    /// Passes each effect segment through `f`, writing back what it changed. A map's keys are
    /// renamed together, so a rename onto another key's name (a swap) keeps both lists.
    pub(crate) fn try_each<E>(
        self,
        f: &mut impl FnMut(&mut String) -> Result<(), E>,
    ) -> Result<(), E> {
        match self {
            PathsMut::Path(p) => {
                if let Some(new) = rewrite(p, f)? {
                    *p = new;
                }
            }
            PathsMut::Keys(map) => {
                let renamed: Vec<Option<String>> = map
                    .keys()
                    .map(|k| rewrite(k, f))
                    .collect::<Result<_, _>>()?;
                if renamed.iter().any(Option::is_some) {
                    let old = std::mem::take(map);
                    *map = old
                        .into_iter()
                        .zip(renamed)
                        .map(|((k, v), new)| (new.unwrap_or(k), v))
                        .collect();
                }
            }
        }
        Ok(())
    }
}

/// `path` with a `$placeholder` effect segment resolved through `bindings` (an effect an
/// earlier op of the transaction added: `effects.$b.radius`); `UNBOUND_PLACEHOLDER` when it is
/// not bound. Other paths are returned as they are.
pub(crate) fn resolve_path(bindings: &Bindings, path: &str) -> Result<String, CoreError> {
    match effect_path(path) {
        Some((fx, rest)) if fx.starts_with('$') => {
            Ok(format!("effects.{}.{rest}", bindings.resolve(fx)?))
        }
        _ => Ok(path.to_string()),
    }
}
