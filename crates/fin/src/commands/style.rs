//! `fin style add|set|remove|ls` (plan 4C Task 24). Styles are the built-in presets
//! (`preset:<name>`) plus project styles `st<n>`; `add` binds `$style`, starting from `--from`
//! (a preset or a style) or the default style, with `--set p=v` paths of the style scope
//! (`fin schema param --scope style`). Like every write that may need fonts, these plan their
//! pins through `fonts.plan` (the `Builder`). `ls` reads the project and works offline.
//!
//! `--from @pub/pack:<name>` (plan 8A Task 12c) copies a caption style an enabled content pack
//! contributes: the server's `plugin.content.get` answers the style (without `id`), and the
//! copy is a `style.add` from the default style whose `set` holds every style path of the
//! registry (`params::specs()`, scope `Style`) the pack's style has a value for, read with
//! `get_style`; `--set` paths apply on top. The project keeps a copy, so it renders the same
//! after the pack is disabled or removed.

use clap::{Args, Subcommand};
use fin_core::captions::presets;
use fin_core::layout::style_key;
use std::collections::BTreeMap;

use fin_core::params::{Scope, ScopeOf, get_style, specs};
use fin_core::{CaptionStyle, CoreError, Op, ParamValue, Project, StyleAdd, StyleRemove, StyleSet};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::param::parse_sets;
use super::track::{SetArgs, set_value};
use super::{Builder, Globals, target_branch};
use crate::backend::Backend;
use crate::output::{internal, usage};

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum StyleCmd {
    Add(AddArgs),
    Set(SetArgs),
    Remove(RemoveArgs),
    Ls,
}

/// Add a project style; binds $style.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddArgs {
    #[arg(long, value_name = "s")]
    pub name: String,
    /// A preset (`preset:bold`), a project style, or an enabled content pack's style
    /// (`@pub/pack:<name>`) to start from (default: the default style).
    #[arg(long, value_name = "style")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `path=value` of the style scope, e.g. `text.size=72px`.
    #[arg(long = "set", value_name = "p=v", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set: Vec<String>,
}

/// Remove an unused project style (STYLE_IN_USE lists its users).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoveArgs {
    #[arg(value_name = "stN")]
    pub style: String,
}

pub fn name(cmd: &StyleCmd) -> &'static str {
    match cmd {
        StyleCmd::Add(_) => "style.add",
        StyleCmd::Set(_) => "style.set",
        StyleCmd::Remove(_) => "style.remove",
        StyleCmd::Ls => "style.ls",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &StyleCmd, suffix: &str) -> Result<Value, CoreError> {
    let rate = b.work.rate;
    match cmd {
        StyleCmd::Add(a) => {
            let own = parse_sets(&ScopeOf::Style, &a.set, rate)?;
            let (from, mut set, pack) = match a.from.as_deref() {
                Some(f) if f.starts_with('@') => {
                    let (set, pack) = pack_style(b.backend, f)?;
                    (None, set, Some(pack))
                }
                other => (other.map(|f| b.id(f)), BTreeMap::new(), None),
            };
            set.extend(own);
            let id = format!("$style{suffix}");
            b.push(Op::StyleAdd(StyleAdd {
                id: id.clone(),
                name: a.name.clone(),
                from,
                set: set.clone(),
            }))?;
            let font = fin_core::captions::style_of(&b.work, &b.id(&id)).map(style_key);
            let mut echo =
                json!({ "style": id, "name": a.name, "from": a.from, "set": set, "font": font });
            if let Some(pack) = pack {
                echo["pack"] = pack;
            }
            Ok(echo)
        }
        StyleCmd::Set(a) => {
            let value = set_value(ScopeOf::Style, a, rate)?;
            b.push(Op::StyleSet(StyleSet {
                style: a.target.clone(),
                path: a.path.clone(),
                value: value.clone(),
            }))?;
            let font = fin_core::captions::style_of(&b.work, &b.id(&a.target)).map(style_key);
            Ok(json!({ "style": a.target, "path": a.path, "value": value, "font": font }))
        }
        StyleCmd::Remove(a) => {
            b.push(Op::StyleRemove(StyleRemove {
                style: a.style.clone(),
            }))?;
            Ok(json!({ "style": a.style }))
        }
        StyleCmd::Ls => Err(usage(
            "style ls is not a write",
            json!({ "command": "style.ls" }),
            "run fin style ls on its own",
        )),
    }
}

/// The `set` paths that make the default style into the caption style `from`
/// (`@pub/pack:<name>`) names, and `{id, version, name}` of what was copied.
fn pack_style(
    backend: &dyn Backend,
    from: &str,
) -> Result<(BTreeMap<String, ParamValue>, Value), CoreError> {
    let Some((id, name)) = from.split_once(':').filter(|(_, n)| !n.is_empty()) else {
        return Err(usage(
            format!("--from {from:?} names no style of a pack"),
            json!({ "from": from }),
            "write a pack's style as --from @publisher/pack:<name> (fin plugin info <id> lists them)",
        ));
    };
    let answer = backend.server_call(
        "plugin.content.get",
        json!({ "id": id, "kind": "captionStyle", "name": name }),
    )?;
    let unexpected = |why: String| {
        internal(
            format!("plugin.content.get answered a style fin cannot read: {why}"),
            json!({ "id": id, "name": name }),
        )
        .with_fix("update fin and finisherd to the same version")
    };
    let bytes = serde_json::to_vec(&answer["style"]).map_err(|e| unexpected(e.to_string()))?;
    // The server checked the file; this is the same check, so nothing outside the model or
    // the registry's ranges reaches the op.
    let style = fin_core::captions::parse_style_file(&bytes).map_err(unexpected)?;
    let mut set = BTreeMap::new();
    for spec in specs().iter().filter(|s| s.scope == Scope::Style) {
        // The copy is named by `--name`; the pack's own name stays in `pack`.
        if spec.path == "name" {
            continue;
        }
        if let Some(value) = get_style(&style, spec.path)? {
            set.insert(spec.path.to_string(), value);
        }
    }
    let pack = json!({ "id": answer["id"], "version": answer["version"], "name": answer["name"] });
    Ok((set, pack))
}

/// One style, compactly: id, name, font key (and whether the project pins it), size, safe
/// zone, line limits; `users` for a project style.
fn line(project: &Project, s: &CaptionStyle, users: bool) -> Value {
    let key = style_key(s);
    let mut v = json!({
        "id": s.id, "name": s.name, "font": key,
        "pinned": project.fonts.pins.contains_key(&key),
        "size": s.text.size, "safeZone": s.safe_zone,
        "maxChars": s.max_chars, "maxLines": s.max_lines,
    });
    if users {
        let captions: Vec<String> = project
            .timelines
            .iter()
            .flat_map(|tl| tl.tracks.iter().flat_map(|t| t.items.iter()))
            .filter(
                |i| matches!(&i.body, fin_core::ItemBody::Caption { block } if block.style == s.id),
            )
            .map(|i| i.id.clone())
            .collect();
        v["captions"] = json!(captions);
    }
    v
}

/// `fin style ls`: the presets and the project's styles.
pub fn ls(backend: &dyn Backend, globals: &Globals) -> Result<Value, CoreError> {
    let branch = target_branch(backend, globals)?;
    let snap = backend.snapshot(&branch)?;
    let p = &snap.project;
    let presets: Vec<Value> = presets().iter().map(|s| line(p, s, false)).collect();
    let styles: Vec<Value> = p.styles.values().map(|s| line(p, s, true)).collect();
    Ok(json!({ "rev": snap.rev, "presets": presets, "styles": styles }))
}
