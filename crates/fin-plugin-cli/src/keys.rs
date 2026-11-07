//! Key files: `keygen`, and the rule that the key ring never lives inside a registry folder.

use std::path::{Component, Path, PathBuf};

use fin_registry::editor::SigningKey;
use serde_json::{Value, json};

use crate::{Args, CliError};

/// `keygen <private.pem> [--public <public.pem>]`: a new ed25519 key; prints its id only.
pub fn keygen(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--public"], &[])?;
    let private = Path::new(&a.positionals(1, "keygen <private.pem>")?[0]).to_path_buf();
    let public = a.opt("--public").map(PathBuf::from);
    for p in std::iter::once(&private).chain(public.as_ref()) {
        if p.exists() {
            return Err(CliError::new(
                "KEY_EXISTS",
                format!("{} exists; keygen never overwrites a key", p.display()),
            ));
        }
    }
    let key = SigningKey::generate();
    key.write(&private)?;
    if let Some(p) = &public {
        std::fs::write(p, key.public().to_pem()).map_err(|e| CliError::io("write", p, e))?;
    }
    Ok(json!({ "keyId": key.public().key_id() }))
}

/// `path` made absolute and, as far as it exists, with symlinks resolved.
fn resolve(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    // Lexically first, so `a/../b` of a folder that does not exist yet is `b`.
    let mut norm = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            c => norm.push(c),
        }
    }
    let abs = norm;
    let mut tail = Vec::new();
    let mut cur = abs.as_path();
    loop {
        if let Ok(c) = cur.canonicalize() {
            return tail.iter().rev().fold(c, |p, t| p.join(t));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return abs,
        }
    }
}

/// The `--keys` folder, refused when it is inside the registry folder (it would be published).
pub fn keys_dir(a: &Args, registry: &Path) -> Result<PathBuf, CliError> {
    let keys = PathBuf::from(a.req("--keys")?);
    if resolve(&keys).starts_with(resolve(registry)) {
        return Err(CliError::usage(format!(
            "{} is inside the registry folder; private keys must never be published",
            keys.display()
        ))
        .fix("keep --keys in a folder outside the registry"));
    }
    Ok(keys)
}

pub fn read_signing_key(path: &str) -> Result<SigningKey, CliError> {
    Ok(SigningKey::read(Path::new(path))?)
}
