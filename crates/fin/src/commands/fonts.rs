//! `fin fonts ls|fetch|status|pin|repin|unpin` (plan 4C Decision 44, Task 24). The catalog and
//! the font cache are the server's: `ls` (`--pinned`: this project's pins) merges `fonts.catalog` with `fonts.status` (whose
//! `files[].keys` say what open projects pin), `fetch` blocks on `fonts.fetch` (progress on
//! stderr, `FONT_FETCH_FAILED` exit 5), `status` is `fonts.status`. `pin`, `repin` and `unpin`
//! are writes: a catalog pin is read off the server's catalog exactly as `fonts.plan` makes
//! one (`fin_fonts::plan`); `--asset` pins a face of an imported font file and works offline.

use clap::{ArgGroup, Args, Subcommand};
use fin_core::layout::key_users;
use fin_core::{CoreError, FontPin, FontPinOp, FontRepin, FontSource, FontUnpin, Op, font_key};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Builder, Globals, target_branch};
use crate::addr;
use crate::backend::Backend;
use crate::output::usage;

#[derive(Debug, Clone, Subcommand)]
pub enum FontsCmd {
    Ls(LsArgs),
    Fetch(FetchArgs),
    Status,
    Pin(PinArgs),
    Repin(RepinArgs),
    Unpin(UnpinArgs),
}

/// The writes among `fin fonts`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum FontEdit {
    Pin(PinArgs),
    Repin(RepinArgs),
    Unpin(UnpinArgs),
}

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[command(group(ArgGroup::new("which").args(["catalog", "cached", "pinned"])))]
#[serde(deny_unknown_fields)]
pub struct LsArgs {
    /// Catalog entries only.
    #[arg(long)]
    #[serde(default)]
    pub catalog: bool,
    /// Cached files only.
    #[arg(long)]
    #[serde(default)]
    pub cached: bool,
    /// This project's pins, with their state and users (Decision 44's `--project`, named
    /// `--pinned` because `--project <dir>` is the global flag).
    #[arg(long)]
    #[serde(default)]
    pub pinned: bool,
    /// Add hashes, sizes, licences and source URLs.
    #[arg(long)]
    #[serde(default)]
    pub detail: bool,
}

/// Fetch a font key (`Family|400|normal`), a catalog entry, a file's SHA-256, or every file
/// the project pins (`pinned`).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchArgs {
    #[arg(
        value_name = "key|entry|sha256",
        required_unless_present = "pinned",
        conflicts_with = "pinned"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[arg(long)]
    #[serde(default)]
    pub pinned: bool,
}

/// Pin a face: a catalog family's style, or a face of a font asset (`asset`).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PinArgs {
    #[arg(value_name = "family", required_unless_present = "asset")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    /// Default 400 (an asset: its family's regular face, else its first).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[arg(long)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[arg(long, value_name = "aN")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset: Option<String>,
    /// Place in the project's fallback order.
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<u32>,
}

/// Point a catalog pin at the file of this server's catalog (`catalog-version` must be it).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RepinArgs {
    #[arg(value_name = "key")]
    pub key: String,
    #[arg(long = "catalog-version", value_name = "v")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_version: Option<String>,
}

/// Remove a pin no style or caption uses (FONT_IN_USE lists its users).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnpinArgs {
    #[arg(value_name = "key")]
    pub key: String,
}

pub fn edit_name(cmd: &FontEdit) -> &'static str {
    match cmd {
        FontEdit::Pin(_) => "fonts.pin",
        FontEdit::Repin(_) => "fonts.repin",
        FontEdit::Unpin(_) => "fonts.unpin",
    }
}

fn not_in_catalog(message: String, details: Value) -> CoreError {
    CoreError::new("FONT_NOT_IN_CATALOG", message)
        .with_details(details)
        .with_fix("pick a family and weight the catalog has (fin fonts ls --catalog), or import the font (fin asset add, then fin fonts pin --asset)")
}

/// The pin of `style` of `entry` in `catalog` (JSON from `fonts.catalog`), as
/// `fin_fonts::plan` makes it: the style's default face, the entry's scripts (sorted).
fn catalog_pin(catalog: &Value, entry: &Value, style: &Value) -> Result<FontPin, CoreError> {
    let mut scripts: Vec<String> =
        serde_json::from_value(entry["scripts"].clone()).unwrap_or_default();
    scripts.sort();
    let file = &style["file"];
    let pin = json!({
        "family": entry["family"], "weight": style["weight"], "italic": style["italic"],
        "face": style.get("defaultFace").cloned().unwrap_or(json!(0)),
        "sha256": file["sha256"], "size": file["size"], "format": file["format"],
        "scripts": scripts,
        "source": { "type": "catalog", "entry": entry["id"], "style": style["id"],
                    "catalog": catalog["catalogVersion"] },
    });
    serde_json::from_value(pin).map_err(|e| {
        crate::output::internal(
            format!("fonts.catalog answered an unexpected style: {e}"),
            json!({ "entry": entry["id"], "style": style["id"] }),
        )
    })
}

fn entries(catalog: &Value) -> &[Value] {
    catalog["entries"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn styles(entry: &Value) -> &[Value] {
    entry["styles"].as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// The catalog pin of `family` at `weight`/`italic`.
fn pin_from_catalog(
    catalog: &Value,
    family: &str,
    weight: u16,
    italic: bool,
) -> Result<FontPin, CoreError> {
    let entry = entries(catalog).iter().find(|e| e["family"] == family);
    let style = entry.and_then(|e| {
        styles(e)
            .iter()
            .find(|s| s["weight"] == weight && s["italic"] == italic)
    });
    match (entry, style) {
        (Some(e), Some(s)) => catalog_pin(catalog, e, s),
        _ => {
            let key = font_key(family, weight, italic);
            let families: Vec<&Value> = entries(catalog).iter().map(|e| &e["family"]).collect();
            let weights: Vec<&Value> = entry.map_or(Vec::new(), |e| {
                styles(e).iter().map(|s| &s["weight"]).collect()
            });
            Err(not_in_catalog(
                format!("the font catalog has no {key}"),
                json!({ "key": key, "families": families, "weights": weights }),
            ))
        }
    }
}

/// The pin of a face of font asset `asset` (an id the work copy holds).
fn pin_from_asset(b: &Builder<'_>, a: &PinArgs, asset: &str) -> Result<FontPin, CoreError> {
    let shown = a.asset.as_deref().unwrap_or(asset);
    let found =
        b.work.assets.get(asset).ok_or_else(|| {
            addr::not_found("asset", shown, b.work.assets.keys().cloned().collect())
        })?;
    let Some(file) = &found.font else {
        return Err(
            CoreError::new("INVALID_OP", format!("asset {shown} is not a font"))
                .with_details(json!({ "asset": shown, "kind": found.kind }))
                .with_fix("name a font asset (fin asset add <file.ttf|otf|ttc>)"),
        );
    };
    let faces: Vec<_> = file
        .faces
        .iter()
        .filter(|f| a.family.as_deref().is_none_or(|fam| f.family == fam))
        .filter(|f| a.weight.is_none_or(|w| f.weight == w) && f.italic == a.italic)
        .collect();
    let face = faces
        .iter()
        .find(|f| f.weight == 400)
        .or(faces.first())
        .ok_or_else(|| {
            let all: Vec<String> = file
                .faces
                .iter()
                .map(|f| font_key(&f.family, f.weight, f.italic))
                .collect();
            CoreError::new("NOT_FOUND", format!("asset {shown} has no such face"))
                .with_details(json!({ "asset": shown, "kind": "face", "alternatives": all }))
                .with_fix("name a family, --weight and --italic of one of the listed faces")
        })?;
    Ok(FontPin {
        family: face.family.clone(),
        weight: face.weight,
        italic: face.italic,
        face: face.index,
        sha256: file.sha256.clone(),
        size: file.size,
        format: file.format,
        scripts: face.scripts.clone(),
        source: FontSource::Asset {
            asset: shown.to_string(),
        },
    })
}

fn echo_pin(key: &str, pin: &FontPin) -> Value {
    json!({ "key": key, "sha256": pin.sha256, "source": pin.source })
}

pub fn plan(b: &mut Builder<'_>, cmd: &FontEdit) -> Result<Value, CoreError> {
    match cmd {
        FontEdit::Pin(a) => {
            let pin = match &a.asset {
                Some(asset) => pin_from_asset(b, a, &b.id(asset))?,
                None => {
                    let catalog = b.backend.server_call("fonts.catalog", json!({}))?;
                    let family = a.family.as_deref().unwrap_or_default();
                    pin_from_catalog(&catalog, family, a.weight.unwrap_or(400), a.italic)?
                }
            };
            let key = font_key(&pin.family, pin.weight, pin.italic);
            let mut echo = echo_pin(&key, &pin);
            echo["fallback"] = json!(a.fallback);
            b.push(Op::FontPin(FontPinOp {
                key,
                pin,
                fallback: a.fallback,
            }))?;
            Ok(echo)
        }
        FontEdit::Repin(a) => {
            let old = b.work.fonts.pins.get(&a.key).cloned().ok_or_else(|| {
                addr::not_found(
                    "font pin",
                    &a.key,
                    b.work.fonts.pins.keys().cloned().collect(),
                )
            })?;
            let FontSource::Catalog {
                entry,
                style,
                catalog: was,
            } = &old.source
            else {
                return Err(CoreError::new(
                    "INVALID_OP",
                    format!("{} is pinned to a font asset, not the catalog", a.key),
                )
                .with_details(json!({ "key": a.key, "source": old.source }))
                .with_fix("pin another face with fin fonts pin --asset <a> (after fin fonts unpin, once nothing uses it)"));
            };
            let catalog = b.backend.server_call("fonts.catalog", json!({}))?;
            let version = catalog["catalogVersion"].as_str().unwrap_or_default();
            if let Some(v) = &a.catalog_version
                && v != version
            {
                return Err(not_in_catalog(
                    format!("this server's font catalog is {version}, not {v}"),
                    json!({ "key": a.key, "catalogVersion": version, "requested": v }),
                ));
            }
            let e = entries(&catalog).iter().find(|e| e["id"] == entry.as_str());
            let s = e.and_then(|e| styles(e).iter().find(|s| s["id"] == style.as_str()));
            let (Some(e), Some(s)) = (e, s) else {
                return Err(not_in_catalog(
                    format!("catalog {version} has no style {style} of entry {entry}"),
                    json!({ "key": a.key, "entry": entry, "style": style }),
                ));
            };
            let pin = catalog_pin(&catalog, e, s)?;
            let echo = json!({ "key": a.key, "from": { "sha256": old.sha256, "catalog": was },
                               "to": { "sha256": pin.sha256, "catalog": version } });
            b.push(Op::FontRepin(FontRepin {
                key: a.key.clone(),
                pin,
            }))?;
            Ok(echo)
        }
        FontEdit::Unpin(a) => {
            b.push(Op::FontUnpin(FontUnpin {
                key: a.key.clone(),
                fallback_only: false,
            }))?;
            Ok(json!({ "key": a.key }))
        }
    }
}

/// `sha256 → file status` of a `fonts.status` answer.
fn file_of<'s>(status: &'s Value, sha: &Value) -> Option<&'s Value> {
    status["files"]
        .as_array()?
        .iter()
        .find(|f| &f["sha256"] == sha)
}

fn state_of(file: Option<&Value>) -> Value {
    file.map_or(Value::Null, |f| f["state"]["state"].clone())
}

/// One catalog entry, compactly.
fn entry_line(e: &Value, status: &Value, detail: bool, cached_only: bool) -> Option<Value> {
    let mut lines = Vec::new();
    for s in styles(e) {
        let file = file_of(status, &s["file"]["sha256"]);
        let cached = state_of(file) == "cached";
        if cached_only && !cached {
            continue;
        }
        let pinned = file.is_some_and(|f| f["keys"].as_array().is_some_and(|k| !k.is_empty()));
        let mut v =
            json!({ "style": s["id"], "weight": s["weight"], "cached": cached, "pinned": pinned });
        if s["italic"] == true {
            v["italic"] = json!(true);
        }
        if detail {
            v["sha256"] = s["file"]["sha256"].clone();
            v["size"] = s["file"]["size"].clone();
            v["state"] = file.map_or(Value::Null, |f| f["state"].clone());
            v["urls"] = json!(s["sources"].as_array().map_or(Vec::new(), |src| {
                src.iter().map(|x| x["url"].clone()).collect()
            }));
        }
        lines.push(v);
    }
    if cached_only && lines.is_empty() {
        return None;
    }
    let mut v = json!({ "entry": e["id"], "family": e["family"], "styles": lines });
    if detail {
        v["licence"] = e["licence"]["spdx"].clone();
        v["scripts"] = e["scripts"].clone();
    }
    Some(v)
}

/// `fin fonts ls`.
fn ls(backend: &dyn Backend, globals: &Globals, a: &LsArgs) -> Result<Value, CoreError> {
    if a.pinned {
        return ls_project(backend, globals, a.detail);
    }
    let catalog = backend.server_call("fonts.catalog", json!({}))?;
    let status = backend.server_call("fonts.status", json!({}))?;
    let list: Vec<Value> = entries(&catalog)
        .iter()
        .filter_map(|e| entry_line(e, &status, a.detail, a.cached))
        .collect();
    let mut doc = json!({ "catalogVersion": catalog["catalogVersion"], "entries": list });
    if !a.catalog {
        // Files open projects pin that the catalog does not list (font assets).
        let files = status["files"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        let other: Vec<Value> = files
            .iter()
            .filter(|f| f.get("entry").is_none())
            .filter(|f| !a.cached || f["state"]["state"] == "cached")
            .map(|f| json!({ "keys": f["keys"], "sha256": f["sha256"], "state": f["state"]["state"] }))
            .collect();
        if !other.is_empty() {
            doc["other"] = json!(other);
        }
    }
    Ok(doc)
}

/// `fin fonts ls --project`: this branch's pins in fallback order first, with the cache state
/// when a server answers, and who uses each.
fn ls_project(backend: &dyn Backend, globals: &Globals, detail: bool) -> Result<Value, CoreError> {
    let branch = target_branch(backend, globals)?;
    let snap = backend.snapshot(&branch)?;
    let fonts = &snap.project.fonts;
    let status = if backend.dry_run_only() {
        None
    } else {
        Some(backend.server_call("fonts.status", json!({}))?)
    };
    let pins: Vec<Value> = fonts
        .pins
        .iter()
        .map(|(key, pin)| {
            let (styles, captions) = key_users(&snap.project, key);
            let mut v = json!({
                "key": key, "source": pin.source,
                "fallback": fonts.fallback.iter().position(|k| k == key),
                "styles": styles, "captions": captions,
            });
            if let Some(s) = &status {
                v["state"] = state_of(file_of(s, &json!(pin.sha256)));
            }
            if detail {
                v["sha256"] = json!(pin.sha256);
                v["size"] = json!(pin.size);
                v["scripts"] = json!(pin.scripts);
            }
            v
        })
        .collect();
    Ok(json!({ "rev": snap.rev, "pins": pins, "fallback": fonts.fallback }))
}

/// What `fonts.fetch` is asked for: a key holds `|`, a SHA-256 is 64 hex digits, anything
/// else is a catalog entry id.
fn fetch_payload(backend: &dyn Backend, a: &FetchArgs) -> Result<Value, CoreError> {
    if a.pinned {
        return Ok(json!({ "pinned": backend.project_id()? }));
    }
    let t = a.target.clone().unwrap_or_default();
    Ok(if t.contains('|') {
        json!({ "keys": [t] })
    } else if t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()) {
        json!({ "sha256": [t.to_ascii_lowercase()] })
    } else {
        json!({ "entries": [t] })
    })
}

/// The reads: `ls`, `fetch`, `status`. Writes go through [`plan`].
pub fn read(backend: &dyn Backend, globals: &Globals, cmd: &FontsCmd) -> Result<Value, CoreError> {
    match cmd {
        FontsCmd::Ls(a) => ls(backend, globals, a),
        FontsCmd::Status => backend.server_call("fonts.status", json!({})),
        FontsCmd::Fetch(a) => {
            let payload = fetch_payload(backend, a)?;
            backend.fonts_fetch(payload).map_err(|mut e| {
                if e.code == "FONT_FETCH_FAILED" {
                    let fix = e.fix.take().unwrap_or_else(|| {
                        "check the network (behind a proxy, set HTTPS_PROXY), then run fin fonts fetch again".into()
                    });
                    e.fix = Some(format!(
                        "{fix}; to work without fetching, set FIN_FONTS_OFFLINE=1 on the server (captions stay unmeasured)"
                    ));
                }
                e
            })
        }
        FontsCmd::Pin(_) | FontsCmd::Repin(_) | FontsCmd::Unpin(_) => Err(usage(
            "fonts pin, repin and unpin are writes",
            json!({}),
            "run them as fin fonts pin|repin|unpin",
        )),
    }
}
