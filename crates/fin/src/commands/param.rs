//! `fin param set <item> <path> (<value>|--reset)` (plan 4C Task 24): one registry path of an
//! item (`fin schema param --scope item` lists them), its value typed by the path's spec
//! ([`crate::values`]). Also the helpers the keyframe, effect, caption and style commands share:
//! an item path's spec and rate, and `--set p=v` lists.

use std::collections::BTreeMap;

use clap::Subcommand;
use fin_core::params::{ParamSpec, ScopeOf, find};
use fin_core::{CoreError, Op, ParamSet, ParamValue, Project, Rational};
use serde::Serialize;
use serde_json::{Value, json};

use super::Builder;
use super::track::{SetArgs, value_or_reset};
use crate::addr;
use crate::output::usage;
use crate::values;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum ParamCmd {
    Set(SetArgs),
}

/// The spec of `path` on item `item` (a working id) and its timeline's rate: graphics look
/// slots up in their template, effects (`effects.<fx>.<param>`) by the effect's kind.
pub fn item_spec(
    project: &Project,
    item: &str,
    path: &str,
) -> Result<(&'static ParamSpec, Rational), CoreError> {
    let (tl, _, it) = addr::item(project, item)?;
    let template = match &it.body {
        fin_core::ItemBody::Graphic { asset, .. } => {
            project.assets.get(asset).and_then(|a| a.lottie.as_ref())
        }
        _ => None,
    };
    let spec = find(&ScopeOf::Item { item: it, template }, path)?;
    Ok((spec, tl.rate))
}

/// `--set p=v` entries parsed by `scope`'s specs.
pub fn parse_sets(
    scope: &ScopeOf<'_>,
    sets: &[String],
    rate: Rational,
) -> Result<BTreeMap<String, ParamValue>, CoreError> {
    let mut out = BTreeMap::new();
    for entry in sets {
        let Some((path, value)) = entry.split_once('=') else {
            return Err(usage(
                format!("--set {entry:?} is not path=value"),
                json!({ "set": entry }),
                "write each setting as --set <path>=<value>, e.g. --set radius=4",
            ));
        };
        let spec = find(scope, path)?;
        out.insert(path.to_string(), values::parse(spec, path, value, rate)?);
    }
    Ok(out)
}

pub fn name(cmd: &ParamCmd) -> &'static str {
    match cmd {
        ParamCmd::Set(_) => "param.set",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &ParamCmd) -> Result<Value, CoreError> {
    let ParamCmd::Set(a) = cmd;
    let item = b.id(&a.target);
    let (spec, rate) = item_spec(&b.work, &item, &a.path)?;
    let value = match (&a.value, a.reset) {
        (Some(v), false) => Some(values::parse(spec, &a.path, v, rate)?),
        (None, true) => None,
        _ => return Err(value_or_reset(&a.path)),
    };
    b.push(Op::ParamSet(ParamSet {
        item: a.target.clone(),
        path: a.path.clone(),
        value: value.clone(),
    }))?;
    Ok(json!({ "item": a.target, "path": a.path, "value": value, "unit": spec.unit }))
}
