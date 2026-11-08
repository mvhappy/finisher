use std::path::Path;

use fin_plugin_api::{MANIFEST_FILE, PluginId};
use serde_json::{Value, json};

use crate::{Args, CliError};

/// The scaffold's caption style: a `CaptionStyle` without its `id`, which the host checks when
/// the pack installs (a style file is not free-form JSON).
const DEFAULT_STYLE: &str = r##"{
  "name": "Default",
  "text": { "family": "Inter", "weight": 700, "italic": false, "size": 64.0, "lineHeight": 1.2, "letterSpacing": 0.0, "align": "center" },
  "color": "#ffffff",
  "position": { "anchor": "bottom", "offset": 0.0 },
  "safeZone": "generic-9x16",
  "maxChars": 32,
  "maxLines": 2,
  "gap": "1/2"
}
"##;

/// A plugin name from a folder name: lowercase letters, digits and dashes.
fn plugin_name(dir: &Path) -> String {
    let raw = dir.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let s: String = raw
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if PluginId::parse(&format!("@example/{s}")).is_ok() {
        s
    } else {
        "my-plugin".into()
    }
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) -> Result<(), CliError> {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CliError::io("create", parent, e))?;
    }
    std::fs::write(&p, bytes).map_err(|e| CliError::io("write", &p, e))
}

/// `new content|code <dir>`: a minimal valid plugin.
pub fn run(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &[], &[])?;
    let p = a.positionals(2, "new content|code <dir>")?;
    let kind = p[0].as_str();
    let dir = Path::new(&p[1]);
    if kind != "content" && kind != "code" {
        return Err(CliError::usage(format!(
            "kind is content or code, not {kind:?}"
        )));
    }
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(CliError::new(
            "DIR_NOT_EMPTY",
            format!("{} is not empty", dir.display()),
        ));
    }
    let id = format!("@example/{}", plugin_name(dir));
    let mut manifest = json!({
        "manifestVersion": 1,
        "id": id,
        "version": "0.1.0",
        "kind": kind,
        "hostApi": "^1.0",
        "license": "MIT",
        "publisher": { "name": "Example" },
    });
    if kind == "content" {
        manifest["contributes"] =
            json!({ "captionStyles": [{ "name": "default", "file": "styles/default.json" }] });
        write(dir, "styles/default.json", DEFAULT_STYLE.as_bytes())?;
    } else {
        manifest["platforms"] = json!(fin_plugin_api::Platform::ALL);
        manifest["runtime"] =
            json!({ "type": "node", "main": "dist/plugin.mjs", "node": ">=26.12" });
        write(dir, "dist/plugin.mjs", b"// The plugin entry point.\n")?;
    }
    let mut text = serde_json::to_string_pretty(&manifest).expect("a manifest serialises");
    text.push('\n');
    write(dir, MANIFEST_FILE, text.as_bytes())?;
    Ok(json!({ "ok": true, "id": id, "kind": kind, "dir": dir.display().to_string() }))
}
