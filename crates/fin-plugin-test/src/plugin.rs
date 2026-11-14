//! The plugin under test: its manifest, validated as the host validates it at install, and the
//! program its runtime runs on this host (spec §3.2): a `native` plugin's executable for this
//! platform, or a `node` plugin's bundled `.mjs` under Node's permission flags.

use std::path::{Path, PathBuf};
use std::process::Command;

use fin_plugin_api::{
    Manifest, PackageFiles, Platform, PluginKind, Runtime, SourceClass, StructuralChecks,
    ValidateCtx, host_api, validate,
};
use serde_json::{Value, json};

use crate::HarnessError;

/// How the plugin's process starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// The executable, run with no arguments but a warm process's `--warm`.
    Native { exe: PathBuf },
    /// `node <permission flags> <main> [--warm]`.
    Node { node: PathBuf, main: PathBuf },
}

impl Program {
    pub fn runtime(&self) -> &'static str {
        match self {
            Program::Native { .. } => "native",
            Program::Node { .. } => "node",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub manifest: Manifest,
    /// The package folder, absolute (the context's `dirs.plugin`).
    pub dir: PathBuf,
    pub program: Program,
}

impl Plugin {
    /// `<id>@<version>`.
    pub fn name(&self) -> String {
        format!("{}@{}", self.manifest.id, self.manifest.version)
    }
}

/// The package's files, with the runtime's executables present whatever the folder holds:
/// a source tree has none of its build outputs, and the one this host runs is checked when it
/// is resolved.
struct Folder<'a> {
    dir: &'a Path,
    executables: Vec<String>,
}

impl PackageFiles for Folder<'_> {
    fn exists(&self, path: &str) -> bool {
        self.executables.iter().any(|e| e == path) || self.dir.join(path).is_file()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join(path)).ok()
    }
}

/// `path` made absolute with links resolved (Node's permission flags and the host's grants
/// name canonical paths); on Windows without the `\\?\` prefix `canonicalize` adds.
pub fn absolute(path: &Path) -> Result<PathBuf, HarnessError> {
    let io = |e: std::io::Error| {
        HarnessError::new("IO_ERROR", format!("resolve {}: {e}", path.display()))
            .with("op", "resolve")
            .with("path", path.display().to_string())
    };
    if cfg!(windows) {
        std::path::absolute(path).map_err(io)
    } else {
        path.canonicalize().map_err(io)
    }
}

/// This host's platform in the manifest's spelling, if a manifest can name it.
pub fn host_platform() -> Option<Platform> {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    serde_json::from_value(json!(format!("{os}-{arch}"))).ok()
}

/// The executables the raw manifest's runtime names (before it is validated).
fn raw_executables(bytes: &[u8]) -> Vec<String> {
    let v: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    v.pointer("/runtime/exec")
        .and_then(Value::as_object)
        .map(|m| {
            m.values()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Reads and validates `dir`'s manifest and resolves its program. `exec` replaces a native
/// plugin's executable for this platform; `node` is the Node a `node` plugin runs on.
pub fn load(dir: &Path, exec: Option<&Path>, node: Option<&Path>) -> Result<Plugin, HarnessError> {
    let dir = absolute(dir)?;
    let path = dir.join(fin_plugin_api::MANIFEST_FILE);
    let bytes = std::fs::read(&path).map_err(|e| {
        HarnessError::new("IO_ERROR", format!("read {}: {e}", path.display()))
            .with("op", "read")
            .with("path", path.display().to_string())
    })?;
    let ctx = ValidateCtx {
        host_api: host_api(),
        platform: None,
        source: SourceClass::Registry,
        node: None,
        content_checks: &StructuralChecks,
    };
    let files = Folder {
        dir: &dir,
        executables: raw_executables(&bytes),
    };
    let manifest = validate(&bytes, &files, &ctx).map_err(|problems| {
        let first = problems
            .first()
            .map(|p| p.message.clone())
            .unwrap_or_default();
        HarnessError::new(
            "PLUGIN_MANIFEST_INVALID",
            format!("{} problem(s); first: {first}", problems.len()),
        )
        .with("problems", json!(problems))
    })?;
    if manifest.kind != PluginKind::Code {
        return Err(HarnessError::new(
            "PLUGIN_RUNTIME_UNAVAILABLE",
            format!(
                "{} is a content plugin: it runs no code to test",
                manifest.id
            ),
        )
        .with("reason", "notCode"));
    }
    let program = match &manifest.runtime {
        Some(Runtime::Native { exec: by_platform }) => native(&dir, by_platform, exec)?,
        Some(Runtime::Node { main, node: need }) => {
            let main = dir.join(main);
            if !main.is_file() {
                return Err(HarnessError::new(
                    "PLUGIN_RUNTIME_UNAVAILABLE",
                    format!("the runtime's main file {} is missing", main.display()),
                )
                .with("reason", "noMain")
                .fix("build the plugin's bundle first"));
            }
            let node = node.map_or_else(|| PathBuf::from("node"), Path::to_path_buf);
            check_node(&node, need)?;
            Program::Node { node, main }
        }
        None => {
            return Err(HarnessError::new(
                "PLUGIN_RUNTIME_UNAVAILABLE",
                "the manifest names no runtime",
            )
            .with("reason", "noRuntime"));
        }
    };
    Ok(Plugin {
        manifest,
        dir,
        program,
    })
}

fn native(
    dir: &Path,
    by_platform: &std::collections::BTreeMap<Platform, String>,
    exec: Option<&Path>,
) -> Result<Program, HarnessError> {
    if let Some(exe) = exec {
        if !exe.is_file() {
            return Err(HarnessError::new(
                "PLUGIN_RUNTIME_UNAVAILABLE",
                format!("--exec {} is not a file", exe.display()),
            )
            .with("reason", "noExecutable"));
        }
        return Ok(Program::Native {
            exe: absolute(exe)?,
        });
    }
    let platform = host_platform();
    let Some(rel) = platform.and_then(|p| by_platform.get(&p)) else {
        let name = platform.map_or("unknown", Platform::as_str);
        return Err(HarnessError::new(
            "PLUGIN_PLATFORM",
            format!("the plugin has no executable for {name}"),
        )
        .with("platform", name));
    };
    let exe = dir.join(rel);
    if !exe.is_file() {
        return Err(HarnessError::new(
            "PLUGIN_RUNTIME_UNAVAILABLE",
            format!("the executable {rel} is not in the plugin's folder"),
        )
        .with("reason", "noExecutable")
        .with("exec", rel.as_str())
        .fix("build it and pass its path with --exec, or test the packed folder"));
    }
    Ok(Program::Native { exe })
}

/// Node runs and its version satisfies the manifest's `runtime.node`.
fn check_node(node: &Path, need: &semver::VersionReq) -> Result<(), HarnessError> {
    let unavailable = |reason: &str, message: String| {
        HarnessError::new("PLUGIN_RUNTIME_UNAVAILABLE", message)
            .with("runtime", "node")
            .with("reason", reason)
            .with("need", need.to_string())
    };
    let out = Command::new(node).arg("--version").output().map_err(|e| {
        unavailable("notFound", format!("{} does not run: {e}", node.display()))
            .fix("install Node or pass its path with --node")
    })?;
    let text = String::from_utf8_lossy(&out.stdout);
    let version = semver::Version::parse(text.trim().trim_start_matches('v')).map_err(|_| {
        unavailable(
            "version",
            format!("cannot read Node's version from {text:?}"),
        )
    })?;
    if !need.matches(&version) {
        return Err(unavailable(
            "version",
            format!("Node {version} does not satisfy the plugin's {need}"),
        )
        .with("found", version.to_string()));
    }
    Ok(())
}
