//! The fake's own configuration (sidecar file and variables), and Codex's: the features the pin
//! knows (the committed `features.txt`), the start-up checks of `-c` values (`config_check`),
//! and the `config/read` answer: the `sessionFlags` layer (`-c`, `--enable`, `--disable`) over
//! the `user` layer (`$CODEX_HOME/config.toml`) over the bundled defaults. Layers merge table by
//! table, as 0.167.0 does: `-c mcp_servers={}` does not remove a user's server.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::argv::Argv;
use super::config_check::{self, TABLE_FEATURES};
use super::{toml_doc, toml_value};

pub const PIN: &str = "0.167.0";
const FEATURES_TXT: &str = include_str!("../../../../vendor/codex/0.167.0/features.txt");
const BASE_CONFIG: &str = include_str!("data/config.json");

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

fn known_feature(f: &str) -> bool {
    features().iter().any(|k| k.name == f)
}

/// The start-up checks of the CLI, as 0.167.0 makes them: an unknown feature always exits 1; a
/// wrongly typed `-c` value always exits 1 (`Error: <serde message>\nin `<key>``); an unknown
/// `-c` key, or an unknown key of `$CODEX_HOME/config.toml`, only under `--strict-config`.
pub fn validate(a: &Argv, home: &Path) -> Result<(), String> {
    for f in a.enable.iter().chain(&a.disable) {
        if !known_feature(f) {
            return Err(format!("Unknown feature flag: {f}"));
        }
    }
    let base = base_config();
    if a.strict {
        for (k, _) in &a.config {
            let path: Vec<&str> = k.split('.').collect();
            if let Some(f) = config_check::unknown_field(&path, &base, known_feature) {
                return Err(format!(
                    "unknown configuration field `{f}` in -c/--config override"
                ));
            }
        }
    }
    if let Err((msg, path)) = config_check::check_layer(&flags_layer(a)) {
        return Err(format!("{msg}\nin `{path}`"));
    }
    let file = home.join("config.toml");
    if a.strict
        && let Ok(text) = std::fs::read_to_string(&file)
        && let Ok(doc) = toml_doc::parse(&text)
    {
        for (k, line, col) in &doc.keys {
            let path: Vec<&str> = k.split('.').collect();
            if let Some(f) = config_check::unknown_field(&path, &base, known_feature) {
                return Err(format!(
                    "{}:{line}:{col}: unknown configuration field `{f}`",
                    file.display()
                ));
            }
        }
    }
    Ok(())
}

/// `$CODEX_HOME/config.toml` as a layer, and the file's text (for its version hash).
/// UNOBSERVED(7E Task 0 Step 3): a file that is not TOML, or holds a wrongly typed value, does
/// not stop 0.167.0's start (seen); the fake then reads it as an empty layer, where the real
/// one may keep the good keys and send a `configWarning`.
fn user_layer(home: &Path) -> (Value, String) {
    let Ok(text) = std::fs::read_to_string(home.join("config.toml")) else {
        return (json!({}), String::new());
    };
    match toml_doc::parse(&text) {
        Ok(doc) if config_check::check_layer(&doc.value).is_ok() => (doc.value, text),
        _ => (json!({}), text),
    }
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

/// Defaults 0.167.0 fills into each `mcp_servers` entry of the effective config.
fn server_defaults(config: &mut Value) {
    let Some(servers) = config["mcp_servers"].as_object_mut() else {
        return;
    };
    for s in servers.values_mut() {
        let Some(m) = s.as_object_mut() else { continue };
        if m.contains_key("command") {
            m.entry("args").or_insert(json!([]));
        }
        m.entry("environment_id").or_insert(json!("local"));
        m.entry("enabled").or_insert(json!(true));
        m.entry("tool_timeout_sec").or_insert(Value::Null);
    }
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
    user: Value,
    user_text: String,
    home: PathBuf,
}

impl Effective {
    pub fn new(a: &Argv, home: &Path) -> Effective {
        let flags = flags_layer(a);
        let (user, user_text) = user_layer(home);
        let mut config = base_config();
        merge(&mut config, &user);
        merge(&mut config, &flags);
        server_defaults(&mut config);
        Effective {
            config,
            flags,
            user,
            user_text,
            home: home.to_path_buf(),
        }
    }

    /// An effective value as a string (`approval_policy`, `sandbox_mode`, ...).
    pub fn str(&self, key: &str) -> Option<String> {
        self.config[key].as_str().map(str::to_string)
    }

    /// Bundled (`system`) skills: on unless `skills.bundled.enabled = false`.
    pub fn bundled_skills(&self) -> bool {
        self.config["skills"]["bundled"]["enabled"] != false
    }

    /// The directory names that end skills discovery upward (`project_root_markers`).
    pub fn root_markers(&self) -> Vec<String> {
        self.config["project_root_markers"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_else(|| vec![".git".into()])
    }

    /// `mcp_servers` of the effective config, by name.
    pub fn mcp_servers(&self) -> Vec<(String, Value)> {
        self.config["mcp_servers"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    pub fn feature(&self, name: &str) -> bool {
        self.config["features"][name].as_bool().unwrap_or(false)
    }

    /// `config/read`; layers (highest precedence first) only when asked for.
    pub fn read(&self, include_layers: bool) -> Value {
        let has_flags = self.flags.as_object().is_some_and(|m| !m.is_empty());
        let user_file = self.home.join("config.toml");
        let user_name = json!({"type": "user", "file": user_file, "profile": null});
        let user_hash = digest(&self.user_text);
        let mut origins = Map::new();
        let mut keys = vec![];
        leaves("", &self.user, &mut keys);
        for k in keys {
            origins.insert(k, json!({"name": user_name, "version": user_hash}));
        }
        let mut layers = vec![];
        if has_flags {
            let version = digest(&self.flags.to_string());
            let mut keys = vec![];
            leaves("", &self.flags, &mut keys);
            for k in keys {
                // A table-shaped feature's origin names its `enabled` key (0.167.0, `--disable
                // sleep_tool`). UNOBSERVED(7E Task 0 Step 3): the same for `-c features.X=false`.
                let k = match k.strip_prefix("features.") {
                    Some(f) if TABLE_FEATURES.contains(&f) => format!("{k}.enabled"),
                    _ => k,
                };
                origins.insert(
                    k,
                    json!({"name": {"type": "sessionFlags"}, "version": version}),
                );
            }
            layers.push(
                json!({"name": {"type": "sessionFlags"}, "version": version, "config": self.flags}),
            );
        }
        layers.push(json!({"name": user_name, "version": user_hash, "config": self.user}));
        layers.push(json!({"name": {"type": "system", "file": "/etc/codex/config.toml"}, "version": digest(""), "config": {}}));
        json!({"config": self.config, "origins": origins, "layers": if include_layers { Value::Array(layers) } else { Value::Null }})
    }

    pub fn project_doc_max_bytes(&self) -> i64 {
        self.config["project_doc_max_bytes"]
            .as_i64()
            .unwrap_or(32768)
    }
}
