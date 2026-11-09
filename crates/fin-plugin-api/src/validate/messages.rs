//! Message catalogues (arch §17.1): `messages` names a directory of `<locale>.json` files,
//! each a flat JSON object of message key → ICU message. `locales` includes `en`, and every
//! key the manifest references (a model's `purpose`, a caption style's `title`, a command's
//! `title` and its argument `description`s) is in the `en` catalogue.

use std::collections::BTreeMap;

use super::files::{catalogue_path, is_package_path};
use super::{Invalid, PackageFiles};
use crate::manifest::Manifest;

/// `[a-z][A-Za-z0-9]*` segments joined by `.`, at most 128 bytes.
pub(crate) fn key_ok(k: &str) -> bool {
    k.len() <= 128
        && k.split('.').all(|seg| {
            let b = seg.as_bytes();
            !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(u8::is_ascii_alphanumeric)
        })
}

/// `en`, `pt-BR`, `zh-Hant`: a 2–3 letter language and optional 2–8 character subtags.
fn locale_ok(l: &str) -> bool {
    let mut parts = l.split('-');
    let lang = parts.next().unwrap_or("");
    (2..=3).contains(&lang.len())
        && lang.bytes().all(|b| b.is_ascii_lowercase())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// The message keys `m` references, as (JSON pointer, key).
fn referenced_keys(m: &Manifest) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = m
        .models
        .iter()
        .enumerate()
        .map(|(i, model)| (format!("/models/{i}/purpose"), model.purpose.clone()))
        .collect();
    for (i, s) in m.contributes.caption_styles.iter().enumerate() {
        if let Some(t) = &s.title {
            v.push((format!("/contributes/captionStyles/{i}/title"), t.clone()));
        }
    }
    for (i, c) in m.contributes.commands.iter().enumerate() {
        let base = format!("/contributes/commands/{i}");
        v.push((format!("{base}/title"), c.title.clone()));
        // A schema outside the subset is reported by `content`; its keys are not read.
        if c.args.check().is_ok() {
            for (at, key) in c.args.descriptions() {
                v.push((format!("{base}/args{at}"), key));
            }
        }
    }
    v
}

pub(super) fn check(m: &Manifest, files: &dyn PackageFiles, out: &mut Vec<Invalid>) {
    let keys = referenced_keys(m);
    for (i, l) in m.locales.iter().enumerate() {
        if !locale_ok(l) {
            out.push(Invalid::manifest(
                &format!("/locales/{i}"),
                "locale",
                format!("{l:?} is not a locale tag"),
            ));
        }
    }
    let Some(dir) = &m.messages else {
        if !m.locales.is_empty() || !keys.is_empty() {
            out.push(Invalid::manifest(
                "/messages",
                "messagesMissing",
                "locales and message keys need a messages directory",
            ));
        }
        return;
    };
    if !m.locales.iter().any(|l| l == "en") {
        out.push(Invalid::manifest(
            "/locales",
            "localeEnMissing",
            "locales includes en",
        ));
        return;
    }
    if !is_package_path(dir.trim_end_matches('/')) {
        return; // reported by `files`
    }
    for (i, l) in m.locales.iter().enumerate() {
        let path = catalogue_path(dir, l);
        let Some(bytes) = files.read(&path) else {
            continue; // reported by `files`
        };
        let parsed = serde_json::from_slice::<BTreeMap<String, String>>(&bytes);
        let catalogue = match parsed {
            Ok(c) => c,
            Err(e) => {
                out.push(Invalid::manifest(
                    &format!("/locales/{i}"),
                    "messagesCatalogue",
                    format!("{path} is not a JSON object of strings: {e}"),
                ));
                continue;
            }
        };
        for k in catalogue.keys().filter(|k| !key_ok(k)) {
            out.push(
                Invalid::manifest(
                    &format!("/locales/{i}"),
                    "messageKey",
                    format!("{path}: {k:?} is not a message key"),
                )
                .with("key", k.clone()),
            );
        }
        if l != "en" {
            continue;
        }
        for (ptr, k) in &keys {
            if !key_ok(k) {
                out.push(Invalid::manifest(
                    ptr,
                    "messageKey",
                    format!("{k:?} is not a message key"),
                ));
            } else if !catalogue.contains_key(k) {
                out.push(
                    Invalid::manifest(ptr, "messageKeyMissing", format!("{k} is not in {path}"))
                        .with("key", k.clone()),
                );
            }
        }
    }
}
