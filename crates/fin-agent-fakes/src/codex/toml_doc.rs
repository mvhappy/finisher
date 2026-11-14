//! A minimal reader for `$CODEX_HOME/config.toml` (the `user` layer): `[table]` headers,
//! `key = value` lines with bare, quoted or dotted keys, `#` comments, and values `toml_value`
//! reads (an array or inline table may span lines). Arrays of tables (`[[x]]`), multi-line
//! strings and dates are not read: such a file is "not TOML" to the fake.

use serde_json::{Map, Value};

use super::toml_value;

/// A parsed file: its value, and where each key was written (dotted path, line, column; both
/// 1-based) for `--strict-config`'s message.
pub struct Doc {
    pub value: Value,
    pub keys: Vec<(String, usize, usize)>,
}

/// A dotted key: bare segments, or `"quoted"` / `'literal'` ones.
fn key_path(raw: &str) -> Option<Vec<String>> {
    let mut out = vec![];
    let mut rest = raw.trim();
    loop {
        let (seg, after) = match rest.chars().next()? {
            q @ ('"' | '\'') => {
                let end = rest[1..].find(q)? + 1;
                (rest[1..end].to_string(), &rest[end + 1..])
            }
            _ => {
                let end = rest.find(['.', ' ', '\t']).unwrap_or(rest.len());
                let seg = &rest[..end];
                if seg.is_empty()
                    || !seg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return None;
                }
                (seg.to_string(), &rest[end..])
            }
        };
        out.push(seg);
        let after = after.trim_start();
        match after.strip_prefix('.') {
            Some(r) => rest = r.trim_start(),
            None if after.is_empty() => return Some(out),
            None => return None,
        }
    }
}

/// The part of a line before `=` that is not inside quotes.
fn split_eq(line: &str) -> Option<(&str, &str)> {
    let mut quote = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '=') => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}

fn insert(root: &mut Map<String, Value>, path: &[String], v: Value) -> Result<(), String> {
    let (last, parents) = path.split_last().ok_or("empty key")?;
    let mut node = root;
    for seg in parents {
        let slot = node
            .entry(seg.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        node = slot
            .as_object_mut()
            .ok_or_else(|| format!("`{seg}` is not a table"))?;
    }
    if node.contains_key(last) {
        return Err(format!("duplicate key `{last}`"));
    }
    node.insert(last.clone(), v);
    Ok(())
}

/// The rest of a line after a value: blank or a comment.
fn only_comment(rest: &str) -> bool {
    let r = rest.trim_start();
    r.is_empty() || r.starts_with('#')
}

pub fn parse(text: &str) -> Result<Doc, String> {
    let mut root = Map::new();
    let mut keys = vec![];
    let mut table: Vec<String> = vec![];
    let lines: Vec<&str> = text.lines().collect();
    let mut n = 0;
    while n < lines.len() {
        let line = lines[n];
        let body = line.trim_start();
        let col = line.len() - body.len() + 1;
        n += 1;
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if body.starts_with("[[") {
            return Err(format!(
                "line {n}: arrays of tables are not read by fake-codex"
            ));
        }
        if let Some(h) = body.strip_prefix('[') {
            let end = h
                .find(']')
                .ok_or_else(|| format!("line {n}: unclosed table header"))?;
            if !only_comment(&h[end + 1..]) {
                return Err(format!("line {n}: text after a table header"));
            }
            table = key_path(&h[..end]).ok_or_else(|| format!("line {n}: bad table name"))?;
            let mut node = &mut root;
            for seg in &table {
                node = node
                    .entry(seg.clone())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| format!("line {n}: `{seg}` is not a table"))?;
            }
            continue;
        }
        let (k, rest) =
            split_eq(body).ok_or_else(|| format!("line {n}: expected `key = value`"))?;
        let path = key_path(k).ok_or_else(|| format!("line {n}: bad key"))?;
        // A value may go on over the next lines (a multi-line array or inline table).
        let (line_no, mut raw) = (n, rest.to_string());
        let value = loop {
            if let Some((v, used)) = toml_value::parse_prefix(raw.trim_start())
                && only_comment(&raw.trim_start().chars().skip(used).collect::<String>())
            {
                break v;
            }
            if n >= lines.len() || n - line_no > 1000 {
                return Err(format!("line {line_no}: not a TOML value"));
            }
            raw.push('\n');
            raw.push_str(lines[n].split('#').next().unwrap_or(""));
            n += 1;
        };
        let full: Vec<String> = table.iter().cloned().chain(path).collect();
        keys.push((full.join("."), line_no, col));
        insert(&mut root, &full, value).map_err(|e| format!("line {line_no}: {e}"))?;
    }
    Ok(Doc {
        value: Value::Object(root),
        keys,
    })
}
