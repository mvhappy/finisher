//! `registry …`: `fin_registry::editor::RepoBuilder` behind a command line.

use std::path::Path;

use fin_plugin_api::PluginId;
use fin_registry::advisory::{Advisory, AdvisoryAction, Severity};
use fin_registry::editor::{PublicKey, RepoBuilder, Role, SigningKey};
use fin_registry::tuf::sha256_hex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::keys::{keys_dir, read_signing_key};
use crate::{Args, CliError, REGISTRY_HELP};

fn open(dir: &Path, a: &Args) -> Result<RepoBuilder, CliError> {
    let keys = keys_dir(a, dir)?;
    Ok(RepoBuilder::open(dir, &keys)?)
}

/// `{sha256, version}` of the newest root.
fn root_info(b: &RepoBuilder) -> Result<Value, CliError> {
    let bytes = b.root_bytes()?;
    let version = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v["signed"]["version"].as_u64())
        .unwrap_or(0);
    Ok(json!({ "sha256": sha256_hex(&bytes), "version": version }))
}

pub fn run(raw: &[String]) -> Result<Value, CliError> {
    let Some((sub, rest)) = raw.split_first() else {
        return Err(CliError::usage(format!(
            "no registry command\n{REGISTRY_HELP}"
        )));
    };
    match sub.as_str() {
        "init" => init(rest),
        "delegate" => delegate(rest),
        "publish" => publish(rest),
        "yank" => yank(rest),
        "advise" => advise(rest),
        "rotate-key" => rotate_key(rest),
        "resign-timestamp" => resign_timestamp(rest),
        other => Err(CliError::usage(format!(
            "unknown registry command {other:?}"
        ))),
    }
}

fn init(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(
        raw,
        &["--keys", "--threshold", "--max-stale-days"],
        &["--root-keys"],
    )?;
    let dir = Path::new(&a.positionals(1, "registry init <dir>")?[0]).to_path_buf();
    let keys = keys_dir(&a, &dir)?;
    let root_keys = a
        .all("--root-keys")?
        .iter()
        .map(|p| read_signing_key(p))
        .collect::<Result<Vec<SigningKey>, _>>()?;
    let threshold = a
        .number::<u32>("--threshold")?
        .ok_or_else(|| CliError::usage("--threshold is required"))?;
    let stale = a.number::<u16>("--max-stale-days")?;
    let b = RepoBuilder::init(&dir, &keys, &root_keys, threshold, stale)?;
    Ok(json!({
        "dir": dir.display().to_string(),
        "root": json!({ "sha256": sha256_hex(&b.first_root_bytes()?), "version": 1 }),
    }))
}

fn delegate(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--keys", "--publisher", "--key"], &[])?;
    let dir = Path::new(&a.positionals(1, "registry delegate <dir>")?[0]).to_path_buf();
    let mut b = open(&dir, &a)?;
    let publisher = a.req("--publisher")?;
    let key = PublicKey::read(Path::new(a.req("--key")?))?;
    b.delegate(publisher, &key)?;
    Ok(json!({ "publisher": publisher, "keyId": key.key_id() }))
}

fn publish(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--keys", "--key"], &[])?;
    let p = a.positionals(2, "registry publish <dir> <pkg.finpkg>")?;
    let (dir, pkg) = (Path::new(&p[0]), Path::new(&p[1]));
    let mut b = open(dir, &a)?;
    let key = read_signing_key(a.req("--key")?)?;
    let t = b.publish(pkg, &key)?;
    Ok(json!({
        "name": t.name,
        "role": t.role,
        "length": t.length,
        "sha256": t.sha256,
        "custom": t.custom,
    }))
}

fn yank(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--keys", "--key"], &[])?;
    let p = a.positionals(2, "registry yank <dir> <id>@<version>")?;
    let (id, version) = p[1]
        .rsplit_once('@')
        .filter(|(id, _)| !id.is_empty())
        .ok_or_else(|| CliError::usage("expected <id>@<version>, e.g. @acme/pack@1.0.0"))?;
    let version = semver::Version::parse(version)
        .map_err(|e| CliError::usage(format!("{version:?} is not a version: {e}")))?;
    let mut b = open(Path::new(&p[0]), &a)?;
    let key = read_signing_key(a.req("--key")?)?;
    b.yank(id, &version, &key)?;
    Ok(json!({ "id": id, "version": version.to_string(), "yanked": true }))
}

fn advise(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(
        raw,
        &[
            "--keys",
            "--plugin",
            "--versions",
            "--action",
            "--severity",
            "--summary",
            "--url",
            "--id",
            "--key",
        ],
        &[],
    )?;
    let dir = Path::new(&a.positionals(1, "registry advise <dir>")?[0]).to_path_buf();
    let plugin = a.req("--plugin")?;
    PluginId::parse(plugin)
        .map_err(|e| CliError::usage(format!("--plugin {plugin:?}: {}", e.reason())))?;
    let versions = a.req("--versions")?;
    semver::VersionReq::parse(versions)
        .map_err(|e| CliError::usage(format!("--versions {versions:?}: {e}")))?;
    let action = match a.req("--action")? {
        "warn" => AdvisoryAction::Warn,
        "disable" => AdvisoryAction::Disable,
        o => {
            return Err(CliError::usage(format!(
                "--action is warn or disable, not {o:?}"
            )));
        }
    };
    let severity = match a.opt("--severity").unwrap_or("high") {
        "low" => Severity::Low,
        "medium" => Severity::Medium,
        "high" => Severity::High,
        "critical" => Severity::Critical,
        o => {
            return Err(CliError::usage(format!(
                "--severity is low, medium, high or critical, not {o:?}"
            )));
        }
    };
    let id = match a.opt("--id") {
        Some(id) => id.to_string(),
        None => {
            let h = Sha256::digest(format!("{plugin}\n{versions}"));
            format!(
                "ADV-{}",
                h.iter()
                    .take(6)
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        }
    };
    let mut b = open(&dir, &a)?;
    let key = read_signing_key(a.req("--key")?)?;
    b.advise(
        Advisory {
            id: id.clone(),
            plugin: plugin.into(),
            versions: versions.into(),
            severity,
            action,
            summary: a.req("--summary")?.into(),
            url: a.opt("--url").map(String::from),
        },
        &key,
    )?;
    Ok(json!({ "id": id, "plugin": plugin, "versions": versions, "action": a.req("--action")? }))
}

fn rotate_key(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--keys", "--role", "--threshold"], &["--new"])?;
    let dir = Path::new(&a.positionals(1, "registry rotate-key <dir>")?[0]).to_path_buf();
    let mut b = open(&dir, &a)?;
    let role = a.req("--role")?;
    let new = a
        .all("--new")?
        .iter()
        .map(|p| read_signing_key(p))
        .collect::<Result<Vec<SigningKey>, _>>()?;
    let threshold = a.number::<u32>("--threshold")?.unwrap_or(1);
    b.rotate(Role::parse(role), &new, threshold)?;
    let ids: Vec<String> = new.iter().map(|k| k.public().key_id()).collect();
    Ok(json!({ "role": role, "threshold": threshold, "keyIds": ids, "root": root_info(&b)? }))
}

fn resign_timestamp(raw: &[String]) -> Result<Value, CliError> {
    let a = Args::parse(raw, &["--keys", "--expires"], &[])?;
    let dir = Path::new(&a.positionals(1, "registry resign-timestamp <dir>")?[0]).to_path_buf();
    let mut b = open(&dir, &a)?;
    let days = a.number::<u32>("--expires")?.unwrap_or(30);
    b.resign_timestamp(days)?;
    let p = dir.join("metadata/timestamp.json");
    let version = std::fs::read(&p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["signed"]["version"].as_u64())
        .unwrap_or(0);
    Ok(json!({ "timestamp": { "version": version, "expiresInDays": days } }))
}
