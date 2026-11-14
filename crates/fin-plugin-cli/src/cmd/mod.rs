pub mod checks;
pub mod new;
pub mod pack;
pub mod registry;
pub mod test;
pub mod validate;

use std::path::Path;

use fin_plugin_api::{Manifest, SourceClass, ValidateCtx, host_api};
use fin_registry::package::DirFiles;
use serde_json::json;

use crate::CliError;

/// Validates the plugin folder `dir` as the host would at install, short of what only the
/// server knows (its platform and Node version, the other installed packs): the manifest and
/// its bytes. Contributed caption styles and font catalogs get the host's own checks
/// ([`checks`]).
pub fn load_valid(dir: &Path) -> Result<(Manifest, Vec<u8>), CliError> {
    let path = dir.join(fin_plugin_api::MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| CliError::io("read", &path, e))?;
    let ctx = ValidateCtx {
        host_api: host_api(),
        platform: None,
        source: SourceClass::Registry,
        node: None,
        content_checks: &checks::HostChecks,
    };
    let files = DirFiles(dir);
    let checked = fin_plugin_api::validate(&bytes, &files, &ctx).and_then(|m| {
        match checks::catalog_merge(&m, &files) {
            None => Ok(m),
            Some(problem) => Err(vec![problem]),
        }
    });
    match checked {
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
