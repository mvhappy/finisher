//! Fills a font cache directory with every file of the built-in catalog and checks each
//! against it: the SHA-256 and size (the fetcher verifies both) and every face's family,
//! weight, italic and script coverage as `fin_text::font_info` reads them. The CI cache
//! warmer (4C Decision 39); `FIN_FONT_CACHE` then points the real-font tests at the dir.
//!
//! ```text
//! cargo run -p fin-fonts --example warm_cache -- [<dir>]       # default: $FIN_FONT_CACHE
//! cargo run -p fin-fonts --example warm_cache -- --inspect <font file>…
//! ```
//!
//! `--inspect` prints `{sha256, size, faces}` for local files (writing a catalog entry).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use fin_fonts::{CatalogFace, FetchOptions, Fetcher, FontCache, FontCatalog, sha256_hex};
use serde_json::json;

fn faces(bytes: &[u8]) -> Result<Vec<CatalogFace>, String> {
    let info = fin_text::font_info(bytes).map_err(|e| e.message)?;
    Ok(info
        .into_iter()
        .map(|f| CatalogFace {
            index: f.index,
            family: f.family,
            weight: f.weight,
            italic: f.italic,
            scripts: f.scripts,
        })
        .collect())
}

fn inspect(paths: &[String]) -> ExitCode {
    for p in paths {
        let bytes = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{p}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let faces = faces(&bytes).unwrap_or_default();
        let out =
            json!({ "path": p, "sha256": sha256_hex(&bytes), "size": bytes.len(), "faces": faces });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    }
    ExitCode::SUCCESS
}

/// What `font_info` says differs from the catalog, if anything.
fn check(catalog: &FontCatalog, cache: &FontCache) -> Vec<String> {
    let mut problems = Vec::new();
    for (entry, style) in catalog.styles() {
        let name = format!("{}:{}", entry.id, style.id);
        let Some(bytes) = cache.get(&style.file.sha256) else {
            problems.push(format!("{name}: not in the cache"));
            continue;
        };
        if bytes.len() as u64 != style.file.size {
            problems.push(format!(
                "{name}: {} bytes, catalog {}",
                bytes.len(),
                style.file.size
            ));
        }
        match faces(&bytes) {
            Ok(read) if read == style.file.faces => {}
            Ok(read) => problems.push(format!(
                "{name}: font_info reads faces {}",
                serde_json::to_string(&read).unwrap()
            )),
            Err(e) => problems.push(format!("{name}: {e}")),
        }
        let face = style.face();
        for s in &entry.scripts {
            if !face.scripts.contains(s) {
                problems.push(format!("{name}: face {} does not cover {s}", face.index));
            }
        }
    }
    problems
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--inspect") {
        return inspect(&args[1..]);
    }
    let Some(dir) = args
        .first()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("FIN_FONT_CACHE").map(PathBuf::from))
    else {
        eprintln!("usage: warm_cache <dir> (or set FIN_FONT_CACHE)");
        return ExitCode::from(2);
    };
    let cache = match FontCache::open(&dir) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("{}", e.message);
            return ExitCode::FAILURE;
        }
    };
    let catalog = Arc::new(FontCatalog::builtin().clone());
    let fetcher = Fetcher::new(cache.clone(), catalog.clone(), FetchOptions::default());
    let mut failed = false;
    for (entry, style) in catalog.styles() {
        let sha = &style.file.sha256;
        match fetcher.fetch_now(sha) {
            Ok(()) => println!(
                "cached  {}:{} {sha} ({} bytes)",
                entry.id, style.id, style.file.size
            ),
            Err(e) => {
                failed = true;
                println!("FAILED  {}:{} {}", entry.id, style.id, e.message);
            }
        }
    }
    let problems = check(&catalog, &cache);
    for p in &problems {
        println!("MISMATCH {p}");
    }
    if failed || !problems.is_empty() {
        return ExitCode::FAILURE;
    }
    println!(
        "warm_cache: catalog {} — {} files cached in {} and match the catalog",
        catalog.catalog_version,
        catalog.styles().count(),
        dir.display()
    );
    ExitCode::SUCCESS
}
