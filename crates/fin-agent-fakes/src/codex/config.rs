//! The fake's own configuration (sidecar file and variables), and Codex's: the features the pin
//! knows (the committed `features.txt`), the `-c` keys `--strict-config` accepts, and the
//! `config/read` answer built from the flags the process was started with.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::argv::Argv;
use super::toml_value;

pub const PIN: &str = "0.167.0";
const FEATURES_TXT: &str = include_str!("../../../../vendor/codex/0.167.0/features.txt");
const BASE_CONFIG: &str = include_str!("data/config.json");
/// Keys the CLI accepts under `--strict-config` that `config/read` does not show.
const EXTRA_KEYS: &[&str] = &["skills.include_instructions", "skills.config"];

/// `<directory of the executable>/fake-codex.json`: `{"script", "observed", "version"}`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sidecar {
    pub script: Option<String>,
    pub observed: Option<String>,
    pub version: Option<String>,
}

pub struct Setup {
    pub script: Option<PathBuf>,
    pub observed: Option<String>,
    pub version: Option<String>,
}

/// Sidecar first, the `FAKE_CODEX_*` variables win over it. A relative script is relative to
/// the executable's directory.
pub fn setup() -> Result<Setup, String> {
    let mut s = Sidecar::default();
    let mut base = PathBuf::new();
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let f = dir.join("fake-codex.json");
        if let Ok(text) = std::fs::read_to_string(&f) {
            s = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", f.display()))?;
        }
        base = dir;
    }
    let var = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
    let script = var("FAKE_CODEX_SCRIPT").or(s.script).map(|p| base.join(p));
    Ok(Setup {
        script,
        observed: var("FAKE_CODEX_OBSERVED").or(s.observed),
        version: var("FAKE_CODEX_VERSION").or(s.version),
    })
}

pub struct Feature {
    pub name: &'static str,
    pub stage: &'static str,
    pub default: bool,
}

pub fn features() -> &'static [Feature] {
    static F: OnceLock<Vec<Feature>> = OnceLock::new();
    F.get_or_init(|| {
        FEATURES_TXT
            .lines()
            .filter_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                let default = w.last()?.parse().ok()?;
                let stage = match w.get(1..w.len() - 1)?.join(" ").as_str() {
                    "experimental" => "beta",
                    "under development" => "underDevelopment",
                    "stable" => "stable",
                    "deprecated" => "deprecated",
                    "removed" => "removed",
                    _ => return None,
                };
                Some(Feature {
                    name: w[0],
                    stage,
                    default,
                })
            })
            .collect()
    })
}

fn base_config() -> Value {
    let mut c: Value = serde_json::from_str(BASE_CONFIG).expect("data/config.json");
    let f: Map<String, Value> = features()
        .iter()
        .map(|f| (f.name.to_string(), json!(f.default)))
        .collect();
    c["features"] = Value::Object(f);
    c
}

/// Whether `-c a.b.c` names a key the CLI knows: every segment must exist in the config shape
/// until a place whose own shape is open (null, a scalar, an array, an empty table).
fn key_known(base: &Value, path: &[&str]) -> bool {
    if EXTRA_KEYS.contains(&path.join(".").as_str()) {
        return true;
    }
    let mut node = base;
    for seg in path {
        match node {
            Value::Object(m) if m.is_empty() => return true,
            Value::Object(m) => match m.get(*seg) {
                Some(n) => node = n,
                None => return false,
            },
            _ => return true,
        }
    }
    true
}

/// The start-up checks of the CLI: an unknown feature always exits 1, an unknown `-c` key only
/// under `--strict-config`.
pub fn validate(a: &Argv) -> Result<(), String> {
    let known = |f: &str| features().iter().any(|k| k.name == f);
    for f in a.enable.iter().chain(&a.disable) {
        if !known(f) {
            return Err(format!("Unknown feature flag: {f}"));
        }
    }
    if a.strict {
        let base = base_config();
        for (k, _) in &a.config {
            let path: Vec<&str> = k.split('.').collect();
            if !key_known(&base, &path) {
                return Err(format!(
                    "unknown configuration field `{k}` in -c/--config override"
                ));
            }
        }
    }
    Ok(())
}

fn set_path(root: &mut Value, path: &[&str], v: Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = root;
    for seg in parents {
        if !node.is_object() {
            *node = json!({});
        }
        node = node
            .as_object_mut()
            .unwrap()
            .entry(seg.to_string())
            .or_insert_with(|| json!({}));
    }
    if !node.is_object() {
        *node = json!({});
    }
    node.as_object_mut().unwrap().insert(last.to_string(), v);
}

/// The flags as the `sessionFlags` layer: `-c` values in order, then `--enable`, `--disable`.
fn flags_layer(a: &Argv) -> Value {
    let mut layer = json!({});
    for (k, v) in &a.config {
        let path: Vec<&str> = k.split('.').collect();
        set_path(&mut layer, &path, toml_value::parse(v));
    }
    for (list, on) in [(&a.enable, true), (&a.disable, false)] {
        for f in list {
            set_path(&mut layer, &["features", f], json!(on));
        }
    }
    layer
}

fn merge(into: &mut Value, from: &Value) {
    match (into, from) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                match a.get_mut(k) {
                    Some(slot) if slot.is_object() && v.is_object() => merge(slot, v),
                    _ => {
                        a.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

/// A stand-in for the layer's content hash (`sha256:` plus 64 hex digits; not a real SHA).
fn digest(s: &str) -> String {
    let mut out = String::from("sha256:");
    for seed in 0..4u64 {
        let mut h = DefaultHasher::new();
        seed.hash(&mut h);
        s.hash(&mut h);
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn leaves(prefix: &str, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => m.iter().for_each(|(k, x)| {
            leaves(
                &if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                },
                x,
                out,
            )
        }),
        Value::Array(a) if a.is_empty() => {}
        _ => out.push(prefix.to_string()),
    }
}

pub struct Effective {
    pub config: Value,
    flags: Value,
    home: PathBuf,
}

impl Effective {
    pub fn new(a: &Argv, home: &Path) -> Effective {
        let flags = flags_layer(a);
        let mut config = base_config();
        merge(&mut config, &flags);
        Effective {
            config,
            flags,
            home: home.to_path_buf(),
        }
    }

    pub fn feature(&self, name: &str) -> bool {
        self.config["features"][name].as_bool().unwrap_or(false)
    }

    /// `config/read`; layers (highest precedence first) only when asked for.
    pub fn read(&self, include_layers: bool) -> Value {
        let has_flags = self.flags.as_object().is_some_and(|m| !m.is_empty());
        let user_file = self.home.join("config.toml");
        let user_hash = digest(&std::fs::read_to_string(&user_file).unwrap_or_default());
        let mut origins = Map::new();
        let mut layers = vec![];
        if has_flags {
            let version = digest(&self.flags.to_string());
            let mut keys = vec![];
            leaves("", &self.flags, &mut keys);
            for k in keys {
                origins.insert(
                    k,
                    json!({"name": {"type": "sessionFlags"}, "version": version}),
                );
            }
            layers.push(
                json!({"name": {"type": "sessionFlags"}, "version": version, "config": self.flags}),
            );
        }
        layers.push(json!({"name": {"type": "user", "file": user_file, "profile": null}, "version": user_hash, "config": {}}));
        layers.push(json!({"name": {"type": "system", "file": "/etc/codex/config.toml"}, "version": digest(""), "config": {}}));
        json!({"config": self.config, "origins": origins, "layers": if include_layers { Value::Array(layers) } else { Value::Null }})
    }

    pub fn project_doc_max_bytes(&self) -> i64 {
        self.config["project_doc_max_bytes"]
            .as_i64()
            .unwrap_or(32768)
    }
}
