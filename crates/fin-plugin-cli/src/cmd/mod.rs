pub mod new;
pub mod pack;
pub mod registry;
pub mod validate;

use std::path::Path;

use fin_plugin_api::{Manifest, SourceClass, StructuralChecks, ValidateCtx, host_api};
use fin_registry::package::DirFiles;
use serde_json::json;

use crate::CliError;

/// Validates the plugin folder `dir` as a registry would; the manifest and its bytes.
pub fn load_valid(dir: &Path) -> Result<(Manifest, Vec<u8>), CliError> {
    let path = dir.join(fin_plugin_api::MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| CliError::io("read", &path, e))?;
    let ctx = ValidateCtx {
        host_api: host_api(),
        platform: None,
        source: SourceClass::Registry,
        node: None,
        content_checks: &StructuralChecks,
    };
    match fin_plugin_api::validate(&bytes, &DirFiles(dir), &ctx) {
        Ok(m) => Ok((m, bytes)),
        Err(problems) => {
            let first = problems
                .first()
                .map(|p| p.message.clone())
                .unwrap_or_default();
            let mut e = CliError::new(
                "PLUGIN_MANIFEST_INVALID",
                format!("{} problem(s); first: {first}", problems.len()),
            );
            e.details.insert("problems".into(), json!(problems));
            Err(e)
        }
    }
}
