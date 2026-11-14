//! The lists the real binary builds from files it probes: skills under the workspace's
//! `.agents/skills` (and each parent up to the project root), `$HOME/.agents/skills`,
//! `$CODEX_HOME/skills` and the bundled `system` skills; hooks from `$CODEX_HOME/hooks.json`;
//! the MCP servers of the effective config; the instruction sources `thread/start` reports.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Fake;

/// The five `system` skills 0.167.0 bundles (from its `skills/list`), paths under
/// `$CODEX_HOME`.
const BUNDLED: &str = include_str!("../data/bundled_skills.json");

/// What 0.167.0 said of a server that did not speak MCP (`command = "ls"`); the fake runs none.
pub const MCP_FAILED: &str = "MCP startup failed: handshaking with MCP server failed: connection closed: initialize response: connection closed: initialize response";

fn bundled(home: &Path) -> Vec<Value> {
    let text = BUNDLED.replace("$CODEX_HOME", &home.to_string_lossy());
    serde_json::from_str(&text).expect("data/bundled_skills.json")
}

/// Writes the bundled skills under `$CODEX_HOME/skills/.system/`, as 0.167.0 does at start
/// unless `skills.bundled.enabled = false` (so provider hygiene sees them).
/// UNOBSERVED(7E Task 0 Step 3): whether it writes them at start or at the first request that
/// needs them, and their full contents (the fake writes the front matter only).
pub(super) fn install_bundled_skills(home: &Path) {
    let root = home.join("skills/.system");
    for s in bundled(home) {
        let Some(path) = s["path"].as_str() else {
            continue;
        };
        let p = PathBuf::from(path);
        if let Some(d) = p.parent()
            && std::fs::create_dir_all(d).is_ok()
        {
            let text = format!(
                "---\nname: {}\ndescription: {}\n---\n",
                s["name"].as_str().unwrap_or(""),
                s["description"].as_str().unwrap_or("")
            );
            let _ = std::fs::write(&p, text);
        }
    }
    let _ = std::fs::write(root.join(".codex-system-skills.marker"), "");
}

/// `PreToolUse` → `pre_tool_use` (the hook key's form of the event).
fn snake(event: &str) -> String {
    let mut out = String::new();
    for (i, c) in event.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn cwd_of(p: &Value) -> PathBuf {
    p.as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// `name:` and `description:` of a SKILL.md's front matter (quotes stripped).
fn front_matter(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    for l in lines {
        if l.trim() == "---" {
            break;
        }
        if let Some(v) = l.strip_prefix(key).and_then(|r| r.strip_prefix(':')) {
            return Some(v.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

fn skills_in(root: &Path, scope: &str, seen: &mut Vec<PathBuf>, out: &mut Vec<Value>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for d in dirs {
        let f = d.join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        if seen.contains(&f) {
            continue;
        }
        seen.push(f.clone());
        let dir_name = d
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(
            json!({"name": front_matter(&text, "name").unwrap_or(dir_name),
            "description": front_matter(&text, "description").unwrap_or_default(),
            "path": f, "scope": scope, "enabled": true, "pluginId": null}),
        );
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

impl Fake {
    pub(super) fn skills_list(&mut self, p: &Value) -> Value {
        let cwds: Vec<PathBuf> = match p["cwds"].as_array() {
            Some(a) if !a.is_empty() => a.iter().map(cwd_of).collect(),
            _ => vec![cwd_of(&Value::Null)],
        };
        let data: Vec<Value> = cwds
            .iter()
            .map(|cwd| {
                let (mut seen, mut skills) = (vec![], vec![]);
                // The workspace and each parent, up to the project root (a directory holding a
                // `project_root_markers` entry, `.git` by default), else up to `/`.
                let markers = self.eff.root_markers();
                for dir in cwd.ancestors() {
                    skills_in(&dir.join(".agents/skills"), "repo", &mut seen, &mut skills);
                    if markers.iter().any(|m| dir.join(m).exists()) {
                        break;
                    }
                }
                if let Some(h) = home_dir() {
                    skills_in(&h.join(".agents/skills"), "user", &mut seen, &mut skills);
                }
                skills_in(&self.home.join("skills"), "user", &mut seen, &mut skills);
                if self.eff.bundled_skills() {
                    skills.extend(bundled(&self.home));
                }
                json!({"cwd": cwd, "skills": skills, "errors": []})
            })
            .collect();
        json!({"data": data})
    }

    pub(super) fn hooks_list(&mut self, p: &Value) -> Value {
        let cwds: Vec<PathBuf> = match p["cwds"].as_array() {
            Some(a) if !a.is_empty() => a.iter().map(cwd_of).collect(),
            _ => vec![cwd_of(&Value::Null)],
        };
        let mut hooks = vec![];
        let file = self.home.join("hooks.json");
        if self.eff.feature("hooks")
            && let Ok(text) = std::fs::read_to_string(&file)
            && let Ok(doc) = serde_json::from_str::<Value>(&text)
        {
            for (event, groups) in doc["hooks"].as_object().into_iter().flatten() {
                let name = {
                    let mut c = event.chars();
                    c.next()
                        .map(|f| f.to_lowercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                };
                for (gi, g) in groups.as_array().into_iter().flatten().enumerate() {
                    for (hi, h) in g["hooks"].as_array().into_iter().flatten().enumerate() {
                        let command = h["command"].as_str().unwrap_or("");
                        hooks.push(json!({"key": format!("{}:{}:{gi}:{hi}", file.display(), snake(event)),
                            "eventName": name, "handlerType": "command", "command": command,
                            "async": false, "matcher": g["matcher"], "timeoutSec": h["timeout"].as_u64().unwrap_or(600),
                            "statusMessage": null, "sourcePath": file, "source": "user", "pluginId": null,
                            "displayOrder": hooks.len(), "enabled": true, "isManaged": false,
                            "currentHash": format!("fake:{command}"), "trustStatus": "untrusted",
                            "environmentId": null, "additionalContextLimit": null}));
                    }
                }
            }
        }
        let data: Vec<Value> = cwds
            .iter()
            .map(|c| json!({"cwd": c, "hooks": hooks, "warnings": [], "errors": []}))
            .collect();
        json!({"data": data})
    }

    /// `mcpServerStatus/list`: each server of the effective config; an enabled one failed to
    /// start (the fake runs none). UNOBSERVED(7E Task 0 Step 3): a server that starts.
    pub(super) fn mcp_status(&self) -> Value {
        let data: Vec<Value> = self
            .eff
            .mcp_servers()
            .into_iter()
            .map(|(name, s)| {
                let err = if s["enabled"] == false { Value::Null } else { json!(MCP_FAILED) };
                json!({"name": name, "runtimeStatus": null, "pluginId": null, "httpOrigin": null,
                    "serverInfo": null, "serverCapabilities": null, "toolListMeta": null, "tools": {},
                    "toolsError": err, "resources": [], "resourceTemplates": [], "authStatus": "unsupported"})
            })
            .collect();
        json!({"data": data, "nextCursor": null})
    }

    /// `$CODEX_HOME/AGENTS.md` (or its override) always; the workspace's while
    /// `project_doc_max_bytes` allows project docs.
    pub(super) fn instruction_sources(&self, cwd: &Path) -> Vec<PathBuf> {
        let pick = |dir: &Path| {
            ["AGENTS.override.md", "AGENTS.md"]
                .iter()
                .map(|n| dir.join(n))
                .find(|p| p.is_file())
        };
        let mut out: Vec<PathBuf> = pick(&self.home).into_iter().collect();
        if self.eff.project_doc_max_bytes() > 0 {
            out.extend(pick(cwd));
        }
        out
    }
}
