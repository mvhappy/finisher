//! The client the `tuf-conformance` suite drives (feature `test-http`; 8A Tasks 5 and 13).
//! Protocol (tuf-conformance `CLIENT-CLI.md`):
//!
//! ```text
//! client --metadata-dir D init TRUSTED_ROOT
//! client --metadata-dir D --metadata-url U refresh
//! client --metadata-dir D --metadata-url U --target-name N… --target-base-url T --target-dir TD download
//! ```
//!
//! Exit 0 on success, 1 otherwise. The trust store lives in `D/.fin-trust/`; after every
//! command the trusted files are mirrored into `D` under non-versioned names. Freshness is
//! `strict` on the system clock and the layout is generic (no publisher rules).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_registry::transport::HttpTransport;
use fin_registry::tuf::{
    Freshness, Layout, Policy, SystemClock, TrustStore, TrustedRepo, TufError, sha256_hex,
};

#[derive(Default)]
struct Args {
    metadata_dir: Option<PathBuf>,
    metadata_url: Option<String>,
    target_names: Vec<String>,
    target_base_url: Option<String>,
    target_dir: Option<PathBuf>,
    command: Option<String>,
    root: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--metadata-dir" => a.metadata_dir = Some(val()?.into()),
            "--metadata-url" => a.metadata_url = Some(val()?),
            "--target-name" => a.target_names.push(val()?),
            "--target-base-url" => a.target_base_url = Some(val()?),
            "--target-dir" => a.target_dir = Some(val()?.into()),
            c if a.command.is_none() => a.command = Some(c.to_string()),
            r if a.root.is_none() => a.root = Some(r.into()),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    Ok(a)
}

const POLICY: Policy = Policy {
    freshness: Freshness::Strict,
    layout: Layout::Generic,
};

fn store(dir: &Path) -> Result<TrustStore, String> {
    let sha = std::fs::read_to_string(dir.join(".fin-trust/anchor")).map_err(|e| e.to_string())?;
    TrustStore::new(&dir.join(".fin-trust"), sha.trim()).map_err(|e| e.to_string())
}

fn mirror(dir: &Path, repo: &TrustedRepo) -> Result<(), String> {
    let files: BTreeMap<String, Vec<u8>> = repo.trusted_files().map_err(|e| e.to_string())?;
    for (name, bytes) in files {
        std::fs::write(dir.join(name), bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A target name as one file name.
fn file_name(target: &str) -> String {
    target
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'.' | b'-' | b'_' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn run(a: Args) -> Result<(), String> {
    let dir = a.metadata_dir.ok_or("--metadata-dir is required")?;
    let tuf = |e: TufError| e.to_string();
    match a.command.as_deref() {
        Some("init") => {
            let root = std::fs::read(a.root.ok_or("init needs TRUSTED_ROOT")?)
                .map_err(|e| e.to_string())?;
            let sha = sha256_hex(&root);
            std::fs::create_dir_all(dir.join(".fin-trust")).map_err(|e| e.to_string())?;
            let s = TrustStore::new(&dir.join(".fin-trust"), &sha).map_err(tuf)?;
            let repo = TrustedRepo::init(s, &root, &sha).map_err(tuf)?;
            std::fs::write(dir.join(".fin-trust/anchor"), &sha).map_err(|e| e.to_string())?;
            mirror(&dir, &repo)
        }
        Some(cmd @ ("refresh" | "download")) => {
            let url = a.metadata_url.ok_or("--metadata-url is required")?;
            let targets = a.target_base_url.clone().unwrap_or_else(|| url.clone());
            let t = Arc::new(HttpTransport::new(&url, &targets));
            let mut repo = TrustedRepo::open(store(&dir)?).map_err(tuf)?;
            let r = repo.refresh(t.clone(), &POLICY, &SystemClock);
            mirror(&dir, &repo)?;
            r.map_err(tuf)?;
            if cmd == "refresh" {
                return Ok(());
            }
            let out = a.target_dir.ok_or("download needs --target-dir")?;
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            for name in &a.target_names {
                let path = out.join(file_name(name));
                let info = repo.target(name).ok_or(format!("no target {name:?}"))?;
                if std::fs::read(&path).is_ok_and(|b| sha256_hex(&b) == info.sha256) {
                    continue;
                }
                repo.fetch_target(t.as_ref(), name, &path).map_err(tuf)?;
            }
            Ok(())
        }
        other => Err(format!("unknown command {other:?}")),
    }
}

fn main() {
    if let Err(e) = parse().and_then(run) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
