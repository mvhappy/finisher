//! The lists the real binary builds from files it probes: skills under the workspace's
//! `.agents/skills` (and every parent), `$HOME/.agents/skills` and `$CODEX_HOME/skills`; hooks
//! from `$CODEX_HOME/hooks.json`; the instruction sources `thread/start` reports.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::Fake;

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
                // The workspace and every parent: the project root marker is not honoured, as
                // the real binary probes up to `/`.
                for dir in cwd.ancestors() {
                    skills_in(&dir.join(".agents/skills"), "repo", &mut seen, &mut skills);
                }
                if let Some(h) = home_dir() {
                    skills_in(&h.join(".agents/skills"), "user", &mut seen, &mut skills);
                }
                skills_in(&self.home.join("skills"), "user", &mut seen, &mut skills);
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
                        hooks.push(json!({"key": format!("{}:{name}:{gi}:{hi}", file.display()),
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
