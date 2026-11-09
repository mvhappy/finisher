//! Contributions: a kind this host does not consume is `PLUGIN_CONTRIBUTION_UNSUPPORTED
//! {kind, hostApi}` (8A Decision 4); font catalogs and caption styles are checked through the
//! injected [`ContentChecks`]; commands' names, timeouts and argument schemas here (8B
//! Task 2).

use std::collections::BTreeSet;

use serde_json::json;

use super::{ContentChecks, Invalid, PackageFiles, ValidateCtx};
use crate::contribution::{ContributionKind, consumable};
use crate::manifest::Manifest;
use crate::protocol::MAX_COMMAND_TIMEOUT_S;

fn unsupported(kind: &str, ctx: &ValidateCtx) -> Invalid {
    Invalid::new(
        "PLUGIN_CONTRIBUTION_UNSUPPORTED",
        format!(
            "host API {} does not consume `{kind}` contributions",
            ctx.host_api
        ),
        json!({ "kind": kind, "hostApi": ctx.host_api.to_string() }),
    )
}

/// `[a-z0-9][a-z0-9-]{0,63}`.
fn style_name_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

pub(super) fn check(
    m: &Manifest,
    files: &dyn PackageFiles,
    ctx: &ValidateCtx,
    out: &mut Vec<Invalid>,
) {
    let eaten = consumable(&ctx.host_api);
    for key in m.contributes.unsupported() {
        out.push(unsupported(key, ctx));
    }
    for kind in m.contributes.kinds() {
        if !eaten.contains(&kind) {
            out.push(unsupported(kind.key(), ctx));
        }
    }
    if eaten.contains(&ContributionKind::FontCatalogs) {
        for (i, c) in m.contributes.font_catalogs.iter().enumerate() {
            let path = format!("/contributes/fontCatalogs/{i}/file");
            content(
                files,
                &c.file,
                &path,
                "fontCatalog",
                |b| ctx.content_checks.font_catalog(b),
                out,
            );
        }
    }
    if eaten.contains(&ContributionKind::CaptionStyles) {
        captions(m, files, ctx.content_checks, out);
    }
    if eaten.contains(&ContributionKind::Commands) {
        commands(m, out);
    }
}

fn commands(m: &Manifest, out: &mut Vec<Invalid>) {
    let mut names = BTreeSet::new();
    for (i, c) in m.contributes.commands.iter().enumerate() {
        let base = format!("/contributes/commands/{i}");
        if !style_name_ok(&c.name) {
            out.push(Invalid::manifest(
                &format!("{base}/name"),
                "commandName",
                format!(
                    "{:?} is not a command name [a-z0-9][a-z0-9-]{{0,63}}",
                    c.name
                ),
            ));
        }
        if !names.insert(c.name.as_str()) {
            out.push(Invalid::manifest(
                &format!("{base}/name"),
                "duplicateCommand",
                format!("command {} is listed twice", c.name),
            ));
        }
        if let Some(t) = c.timeout_s
            && !(1..=MAX_COMMAND_TIMEOUT_S).contains(&t)
        {
            out.push(Invalid::manifest(
                &format!("{base}/timeoutS"),
                "commandTimeout",
                format!("a command's timeout is 1 to {MAX_COMMAND_TIMEOUT_S} s, not {t}"),
            ));
        }
        if let Err(e) = c.args.check() {
            out.push(
                Invalid::manifest(
                    &format!("{base}/args"),
                    "argsSchema",
                    format!(
                        "command {}: arguments schema at {:?}: {}",
                        c.name, e.at, e.message
                    ),
                )
                .with("at", e.at)
                .with("problem", e.problem),
            );
        }
    }
}

fn captions(
    m: &Manifest,
    files: &dyn PackageFiles,
    checks: &dyn ContentChecks,
    out: &mut Vec<Invalid>,
) {
    let mut names = BTreeSet::new();
    for (i, s) in m.contributes.caption_styles.iter().enumerate() {
        let base = format!("/contributes/captionStyles/{i}");
        if !style_name_ok(&s.name) {
            out.push(Invalid::manifest(
                &format!("{base}/name"),
                "styleName",
                format!("{:?} is not a style name [a-z0-9][a-z0-9-]{{0,63}}", s.name),
            ));
        }
        if !names.insert(s.name.as_str()) {
            out.push(Invalid::manifest(
                &format!("{base}/name"),
                "duplicateStyle",
                format!("style {} is listed twice", s.name),
            ));
        }
        content(
            files,
            &s.file,
            &format!("{base}/file"),
            "captionStyle",
            |b| checks.caption_style(b),
            out,
        );
    }
}

/// Runs `check` on the file's bytes; a missing file is reported by `files`.
fn content(
    files: &dyn PackageFiles,
    file: &str,
    path: &str,
    reason: &str,
    check: impl Fn(&[u8]) -> Result<(), String>,
    out: &mut Vec<Invalid>,
) {
    let Some(bytes) = files.read(file) else {
        return;
    };
    if let Err(why) = check(&bytes) {
        out.push(
            Invalid::manifest(path, reason, format!("{file}: {why}"))
                .with("file", file.to_string()),
        );
    }
}
