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

use fin_fonts::{CatalogFace, FetchOptions, Fetcher, FontCache, FontCatalog, Variable, sha256_hex};
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

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The `wght` axis `[min, max]` of face `index`'s `fvar` table, rounded to integers; `None`
/// for a static face.
fn wght_axis(bytes: &[u8], index: u32) -> Option<Variable> {
    let font = if bytes.get(0..4)? == b"ttcf" {
        be32(bytes, 12 + 4 * index as usize)? as usize
    } else {
        0
    };
    let tables = be16(bytes, font + 4)? as usize;
    let fvar = (0..tables).find_map(|i| {
        let rec = font + 12 + 16 * i;
        (bytes.get(rec..rec + 4)? == b"fvar").then(|| be32(bytes, rec + 8))?
    })? as usize;
    let (axes, count, size) = (
        be16(bytes, fvar + 4)? as usize,
        be16(bytes, fvar + 8)? as usize,
        be16(bytes, fvar + 10)? as usize,
    );
    let fixed = |at: usize| be32(bytes, at).map(|v| (f64::from(v as i32) / 65536.0).round() as u16);
    (0..count).find_map(|i| {
        let a = fvar + axes + i * size;
        (bytes.get(a..a + 4)? == b"wght").then(|| {
            Some(Variable {
                wght: [fixed(a + 4)?, fixed(a + 12)?],
            })
        })?
    })
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
        let variable = wght_axis(&bytes, 0);
        let out = json!({ "path": p, "sha256": sha256_hex(&bytes), "size": bytes.len(), "faces": faces, "variable": variable });
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
        let axis = wght_axis(&bytes, style.default_face);
        if axis != style.file.variable {
            problems.push(format!(
                "{name}: the fvar wght axis is {axis:?}, the catalog says {:?}",
                style.file.variable
            ));
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
        "warm_cache: catalog {} — {} styles in {} files, cached in {} and matching the catalog",
        catalog.catalog_version,
        catalog.styles().count(),
        catalog
            .styles()
            .map(|(_, s)| &s.file.sha256)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        dir.display()
    );
    ExitCode::SUCCESS
}
