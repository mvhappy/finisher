//! Key files: `keygen`, and the rule that the key ring never lives inside a registry folder.

use std::path::{Path, PathBuf};

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

/// The `--keys` folder, refused when it is inside the registry folder (it would be published).
pub fn keys_dir(a: &Args, registry: &Path) -> Result<PathBuf, CliError> {
    let keys = PathBuf::from(a.req("--keys")?);
    if fin_registry::editor::keys_inside(registry, &keys) {
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
