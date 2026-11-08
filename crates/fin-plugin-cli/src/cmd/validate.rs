use std::path::Path;

use serde_json::{Value, json};

use crate::{Args, CliError};

/// `validate <dir>`.
pub fn run(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &[], &[])?;
    let dir = &a.positionals(1, "validate <dir>")?[0];
    let (m, _) = super::load_valid(Path::new(dir))?;
    Ok(
        json!({ "ok": true, "id": m.id.to_string(), "version": m.version.to_string(), "kind": m.kind }),
    )
}
