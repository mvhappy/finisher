use std::path::Path;

use serde_json::{Value, json};

use crate::{Args, CliError};

/// `pack <dir> -o <file>`.
pub fn run(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["-o"], &[])?;
    let dir = Path::new(&a.positionals(1, "pack <dir> -o <file>")?[0]).to_path_buf();
    let out = Path::new(a.req("-o")?).to_path_buf();
    let (m, _) = super::load_valid(&dir)?;
    let info = fin_registry::package::pack(&dir, &m.executables(), &out)?;
    Ok(json!({
        "id": m.id.to_string(),
        "version": m.version.to_string(),
        "file": out.display().to_string(),
        "bytes": info.bytes,
        "sha256": info.sha256,
        "entries": info.entries,
        "treeHash": info.tree_hash,
    }))
}
