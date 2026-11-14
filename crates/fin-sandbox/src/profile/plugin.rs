//! The `plugin` profile (sp8 §16.1; plan 7C Decisions 9 and 21).
//!
//! Reads: the plugin's store directory, the runtime, system libraries, ONNX Runtime, the
//! manifest's model files and the call's `in/`. Writes: the call's `out/`, the per-project
//! plugin data directory with `fs pluginData readWrite`, a private temp. The whole store is
//! protected (read-only, never linkable). No network, no pathname socket; GPU nodes only with
//! the `gpu` grant and never `/dev/dri/card*`; server data, the sensitive home paths and the
//! user's document folders (macOS: TCC-protected) denied. Limits of sp8 §3.3.

use std::path::{Path, PathBuf};

use super::{finish, inside, not_under, path, paths, personal_folders, sensitive_home};
use crate::error::PolicyError;
use crate::inputs::PluginInput;
use crate::policy::{Limits, Net, Os, Profile, ReadPolicy, SandboxPolicy, under};

pub const PROCESSES: u32 = 8;
pub const OPEN_FILES: u32 = 1024;
pub const DEFAULT_MEMORY: u64 = 2 << 30;
pub const MAX_MEMORY: u64 = 16 << 30;

pub fn plugin(i: &PluginInput) -> Result<SandboxPolicy, PolicyError> {
    plugin_on(Os::current(), i)
}

fn is_card_node(p: &Path) -> bool {
    p.parent().is_some_and(|d| d.ends_with("dev/dri"))
        && p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("card"))
}

pub fn plugin_on(os: Os, i: &PluginInput) -> Result<SandboxPolicy, PolicyError> {
    for (field, p) in [
        ("storeDir", &i.store_dir),
        ("callDir", &i.call_dir),
        ("tmp", &i.tmp),
        ("serverData", &i.server_data),
        ("home", &i.home),
    ] {
        path(field, p)?;
    }
    paths("runtime", &i.runtime)?;
    paths("system", &i.system)?;
    paths("onnxRuntime", &i.onnx_runtime)?;
    paths("models", &i.models)?;
    paths("pluginData.dir", i.plugin_data.iter().map(|d| &d.dir))?;
    paths("gpu", i.gpu.iter().map(|d| &d.path))?;
    if let Some(d) = i.gpu.iter().find(|d| is_card_node(&d.path)) {
        return Err(PolicyError::input(
            "gpu",
            format!("{} is a display node (only render nodes)", d.path.display()),
        ));
    }
    let memory = i.memory_bytes.unwrap_or(DEFAULT_MEMORY);
    if memory == 0 || memory > MAX_MEMORY {
        return Err(PolicyError::input(
            "memoryBytes",
            format!("{memory} is outside 1 B..16 GiB"),
        ));
    }

    let store = under(&i.server_data, "plugins/store");
    inside("storeDir", &i.store_dir, &store)?;
    let mut home_deny = sensitive_home(os, &i.home);
    home_deny.extend(personal_folders(os, &i.home));
    let mut deny = vec![i.server_data.clone()];
    deny.extend(home_deny.iter().cloned());

    // The runtime and system libraries are never in server data; the call's files, the
    // models and the plugin's data may be (holes), but no grant is in a personal folder.
    not_under("runtime", &i.runtime, &deny)?;
    not_under("system", &i.system, &deny)?;
    not_under("onnxRuntime", &i.onnx_runtime, &deny)?;
    not_under("models", &i.models, &home_deny)?;
    not_under("callDir", [&i.call_dir], &home_deny)?;
    not_under("tmp", [&i.tmp], &home_deny)?;
    not_under(
        "pluginData.dir",
        i.plugin_data.iter().map(|d| &d.dir),
        &home_deny,
    )?;
    let in_store = |field: &str, p: &PathBuf| -> Result<(), PolicyError> {
        if p.starts_with(&store) {
            Err(PolicyError::input(field, "the plugin store is read-only"))
        } else {
            Ok(())
        }
    };
    in_store("callDir", &i.call_dir)?;
    in_store("tmp", &i.tmp)?;

    let mut read: Vec<PathBuf> = vec![i.store_dir.clone(), i.call_dir.join("in")];
    read.extend(i.runtime.iter().cloned());
    read.extend(i.system.iter().cloned());
    read.extend(i.onnx_runtime.iter().cloned());
    read.extend(i.models.iter().cloned());
    let mut write = vec![i.call_dir.join("out"), i.tmp.clone()];
    if let Some(d) = &i.plugin_data {
        if d.write {
            write.push(d.dir.clone())
        } else {
            read.push(d.dir.clone())
        }
    }

    finish(SandboxPolicy {
        profile: Profile::Plugin,
        write,
        read: ReadPolicy::Only(read),
        protect: vec![store],
        deny_read: deny,
        unix_sockets: vec![],
        net: Net::None,
        // macOS: the (empty) allow-list denies `tccd` and the Apple Events service (sp8 §3.2).
        mach: crate::data::mach(os, Profile::Plugin),
        devices: i.gpu.clone(),
        limits: Limits {
            processes: Some(PROCESSES),
            memory: Some(memory),
            open_files: Some(OPEN_FILES),
            wall_ms: None,
        },
    })
}
