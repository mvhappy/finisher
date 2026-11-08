//! Identity, per-kind fields, runtimes and compatibility with the running host.

use std::collections::BTreeSet;

use semver::{Op, VersionReq};
use serde_json::json;

use super::{Invalid, SourceClass, ValidateCtx};
use crate::manifest::{Manifest, PluginKind, Runtime};

pub(super) fn check(m: &Manifest, ctx: &ValidateCtx, out: &mut Vec<Invalid>) {
    identity(m, ctx, out);
    match m.kind {
        PluginKind::Content => content_kind(m, out),
        PluginKind::Code => code_kind(m, out),
    }
    models(m, out);
    compatibility(m, ctx, out);
}

fn identity(m: &Manifest, ctx: &ValidateCtx, out: &mut Vec<Invalid>) {
    if m.id.is_reserved() && ctx.source != SourceClass::Bundled {
        out.push(Invalid::manifest(
            "/id",
            "reservedPublisher",
            format!(
                "publisher @{} is reserved for bundled plugins",
                m.id.publisher()
            ),
        ));
    }
    if m.id.is_dev() && ctx.source != SourceClass::Dev {
        out.push(Invalid::manifest(
            "/id",
            "devPublisher",
            "@dev plugins load only in developer mode (--plugin-dir)",
        ));
    }
    if !m.version.build.is_empty() {
        out.push(Invalid::manifest(
            "/version",
            "buildMetadata",
            format!("version {} has build metadata", m.version),
        ));
    }
    let name = m.publisher.name.trim();
    if name.is_empty() || m.publisher.name.chars().count() > 128 {
        out.push(Invalid::manifest(
            "/publisher/name",
            "publisherName",
            "a publisher name is 1 to 128 characters",
        ));
    }
    if let Some(url) = &m.publisher.url
        && !https_url(url)
    {
        out.push(Invalid::manifest(
            "/publisher/url",
            "https",
            format!("{url:?} is not an https URL"),
        ));
    }
}

/// `https://<host>…` with a non-empty host and no whitespace or user info.
pub(super) fn https_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty() && !authority.contains('@') && !url.chars().any(char::is_whitespace)
}

fn content_kind(m: &Manifest, out: &mut Vec<Invalid>) {
    let mut refuse = |path: &str, reason: &str, what: &str| {
        out.push(Invalid::manifest(
            path,
            reason,
            format!("a content pack has no {what}"),
        ));
    };
    if !m.permissions.is_empty() {
        refuse("/permissions", "contentPermissions", "permissions");
    }
    if m.runtime.is_some() {
        refuse("/runtime", "contentRuntime", "runtime");
    }
    if !m.platforms.is_empty() {
        refuse("/platforms", "contentPlatforms", "platforms");
    }
    if !m.models.is_empty() {
        refuse("/models", "contentModels", "models");
    }
    if m.contributes.is_empty() {
        out.push(Invalid::manifest(
            "/contributes",
            "contentEmpty",
            "a content pack contributes something",
        ));
    }
}

fn code_kind(m: &Manifest, out: &mut Vec<Invalid>) {
    if m.platforms.is_empty() {
        out.push(Invalid::manifest(
            "/platforms",
            "codePlatforms",
            "a code plugin lists its platforms",
        ));
    }
    let set: BTreeSet<_> = m.platforms.iter().copied().collect();
    if set.len() != m.platforms.len() {
        out.push(Invalid::manifest(
            "/platforms",
            "duplicatePlatform",
            "a platform is listed twice",
        ));
    }
    match &m.runtime {
        None => out.push(Invalid::manifest(
            "/runtime",
            "codeRuntime",
            "a code plugin names its runtime",
        )),
        Some(Runtime::Native { exec }) => {
            let keys: BTreeSet<_> = exec.keys().copied().collect();
            if keys != set {
                out.push(Invalid::manifest(
                    "/runtime/exec",
                    "execPlatforms",
                    "runtime.exec has exactly one executable per listed platform",
                ));
            }
        }
        Some(Runtime::Node { main, node }) => {
            if !main.ends_with(".mjs") {
                out.push(Invalid::manifest(
                    "/runtime/main",
                    "nodeMain",
                    format!("{main:?} is not one bundled .mjs file"),
                ));
            }
            if !has_minimum(node) {
                out.push(Invalid::manifest(
                    "/runtime/node",
                    "nodeMinVersion",
                    format!("node range {node} names no minimum Node version"),
                ));
            }
        }
    }
}

/// Whether a range bounds Node from below (`>=24.22`, `^24`, `24.*`); `*` and `<30` do not.
fn has_minimum(req: &VersionReq) -> bool {
    req.comparators
        .iter()
        .any(|c| !matches!(c.op, Op::Less | Op::LessEq) && !(c.op == Op::Wildcard && c.major == 0))
}

fn hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn models(m: &Manifest, out: &mut Vec<Invalid>) {
    for (i, model) in m.models.iter().enumerate() {
        let path = format!("/models/{i}");
        let id_ok = !model.id.is_empty()
            && model.id.len() <= 128
            && model
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_.".contains(&b));
        let mut bad = |field: &str, why: &str| {
            out.push(Invalid::manifest(
                &format!("{path}/{field}"),
                "model",
                format!("model {}: {why}", model.id),
            ));
        };
        if !id_ok {
            bad("id", "an id is 1 to 128 of [a-z0-9-_.]");
        }
        if !https_url(&model.url) {
            bad("url", "the URL is not https");
        }
        if !hex64(&model.sha256) {
            bad("sha256", "sha256 is 64 lowercase hex digits");
        }
        if model.bytes == 0 {
            bad("bytes", "bytes is the file's size");
        }
    }
}

fn compatibility(m: &Manifest, ctx: &ValidateCtx, out: &mut Vec<Invalid>) {
    if !m.host_api.matches(&ctx.host_api) {
        out.push(Invalid::new(
            "PLUGIN_HOST_API",
            format!(
                "the plugin needs host API {}; this host is {}",
                m.host_api, ctx.host_api
            ),
            json!({ "range": m.host_api.to_string(), "host": ctx.host_api.to_string() }),
        ));
    }
    if m.kind == PluginKind::Code
        && let Some(p) = ctx.platform
        && !m.platforms.is_empty()
        && !m.platforms.contains(&p)
    {
        let platforms: Vec<&str> = m.platforms.iter().map(|p| p.as_str()).collect();
        out.push(Invalid::new(
            "PLUGIN_PLATFORM",
            format!(
                "the plugin runs on {}, not {}",
                platforms.join(", "),
                p.as_str()
            ),
            json!({ "platforms": platforms }),
        ));
    }
    if let (Some(Runtime::Node { node, .. }), Some(v)) = (&m.runtime, &ctx.node)
        && !node.matches(v)
    {
        out.push(Invalid::new(
            "PLUGIN_NODE_VERSION",
            format!("the plugin needs Node {node}; this host runs {v}"),
            json!({ "range": node.to_string(), "node": v.to_string() }),
        ));
    }
}
