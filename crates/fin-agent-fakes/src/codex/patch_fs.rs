//! An accepted `fileChange` written to disk, so a test sees what an approval let through (and a
//! declined one leaves nothing). A change is a `FileUpdateChange {path, kind, diff}`: `add`
//! writes `diff` as the file's text, `delete` removes the file, `update` applies `diff` (a
//! unified diff, `@@` hunks) and, with `move_path`, writes the result there and removes the
//! original. Relative paths are the thread's `cwd`'s. Every change is computed before any is
//! written, so a hunk that does not apply leaves the disk as it was.
//! UNOBSERVED(7E Task 0 Step 3): how 0.167.0 reports a patch that does not apply (the fake fails
//! the item, `status: "failed"`).

use std::path::{Path, PathBuf};

use serde_json::Value;

enum Op {
    Write(PathBuf, String),
    Remove(PathBuf),
}

fn resolve(cwd: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn lines(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// `@@ -a[,b] +c[,d] @@` → the 1-based start line of the old side.
fn hunk_start(header: &str) -> Option<usize> {
    let old = header.strip_prefix("@@ -")?.split([' ', ',']).next()?;
    old.parse().ok()
}

/// Applies a unified diff's hunks to `text`.
pub fn apply_diff(text: &str, diff: &str) -> Result<String, String> {
    let mut out: Vec<String> = lines(text).into_iter().map(str::to_string).collect();
    let mut hunks: Vec<(usize, Vec<String>, Vec<String>)> = vec![];
    for l in diff.lines() {
        if l.starts_with("@@") {
            let start = hunk_start(l).ok_or_else(|| format!("bad hunk header: {l}"))?;
            hunks.push((start, vec![], vec![]));
            continue;
        }
        let Some((_, old, new)) = hunks.last_mut() else {
            // Headers before the first hunk (`---`, `+++`, `diff`, `index`).
            continue;
        };
        match l.chars().next() {
            Some(' ') => {
                old.push(l[1..].to_string());
                new.push(l[1..].to_string());
            }
            None => {
                old.push(String::new());
                new.push(String::new());
            }
            Some('-') => old.push(l[1..].to_string()),
            Some('+') => new.push(l[1..].to_string()),
            Some('\\') => {}
            _ => return Err(format!("bad diff line: {l}")),
        }
    }
    // Later hunks first, so earlier line numbers stay right.
    for (start, old, new) in hunks.into_iter().rev() {
        let at = start.saturating_sub(1).min(out.len());
        let fits = |i: usize| {
            out.get(i..i + old.len())
                .is_some_and(|w| w == old.as_slice())
        };
        let i = if fits(at) {
            at
        } else {
            (0..=out.len().saturating_sub(old.len()))
                .find(|&i| fits(i))
                .ok_or_else(|| format!("hunk at line {start} does not apply"))?
        };
        out.splice(i..i + old.len(), new);
    }
    let mut s = out.join("\n");
    if !s.is_empty() && (text.ends_with('\n') || text.is_empty()) {
        s.push('\n');
    }
    Ok(s)
}

fn plan(cwd: &Path, c: &Value) -> Result<Vec<Op>, String> {
    let path = c["path"].as_str().ok_or("a change without a path")?;
    let p = resolve(cwd, path);
    let diff = c["diff"].as_str().unwrap_or("");
    match c["kind"]["type"].as_str() {
        Some("add") => Ok(vec![Op::Write(p, diff.to_string())]),
        Some("delete") if p.is_file() => Ok(vec![Op::Remove(p)]),
        Some("delete") => Err(format!("{path}: no such file")),
        Some("update") => {
            let old = std::fs::read_to_string(&p).map_err(|e| format!("{path}: {e}"))?;
            let new = apply_diff(&old, diff).map_err(|e| format!("{path}: {e}"))?;
            match c["kind"]["move_path"].as_str() {
                Some(to) => Ok(vec![Op::Write(resolve(cwd, to), new), Op::Remove(p)]),
                None => Ok(vec![Op::Write(p, new)]),
            }
        }
        k => Err(format!("{path}: unknown change kind {k:?}")),
    }
}

/// Writes every change, or none (`Err` names the first that does not apply).
pub fn apply(cwd: &Path, changes: &[Value]) -> Result<(), String> {
    let mut ops = vec![];
    for c in changes {
        ops.extend(plan(cwd, c)?);
    }
    for op in ops {
        match op {
            Op::Write(p, text) => {
                if let Some(d) = p.parent() {
                    std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
                }
                std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
            }
            Op::Remove(p) => {
                std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
    }
    Ok(())
}
