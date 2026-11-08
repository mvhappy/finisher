//! Permission grammar: each `perm` at most once, non-empty and duplicate-free lists, origins
//! as `https://host[:port]`, and every `hooks:modify` hook also observed (receiving an event
//! needs `hooks:observe`; modifying it needs `hooks:modify` too, spec §3.6).

use std::collections::BTreeSet;

use super::Invalid;
use crate::manifest::Manifest;
use crate::permission::Permission;

pub(super) fn check(m: &Manifest, out: &mut Vec<Invalid>) {
    let mut seen = BTreeSet::new();
    for (i, p) in m.permissions.iter().enumerate() {
        let path = format!("/permissions/{i}");
        if !seen.insert(p.key()) {
            out.push(Invalid::manifest(
                &path,
                "duplicatePermission",
                format!("{} is listed twice", p.key()),
            ));
        }
        match p {
            Permission::Net { origins } => {
                let items: Vec<&str> = origins.iter().map(|o| o.0.as_str()).collect();
                list(
                    &path,
                    "origins",
                    &items,
                    origin_ok,
                    "an https://host[:port] origin",
                    out,
                );
            }
            Permission::Secrets { names, .. } => {
                let items: Vec<&str> = names.iter().map(String::as_str).collect();
                list(
                    &path,
                    "names",
                    &items,
                    secret_ok,
                    "a secret name [a-z0-9][a-z0-9_-]{0,63}",
                    out,
                );
            }
            Permission::HooksObserve { hooks } | Permission::HooksModify { hooks } => {
                let items: Vec<&str> = hooks.iter().map(String::as_str).collect();
                list(
                    &path,
                    "hooks",
                    &items,
                    hook_ok,
                    "a hook event name [A-Z][A-Za-z0-9]{0,63}",
                    out,
                );
            }
            _ => {}
        }
    }
    let hooks_of = |want_modify: bool| -> BTreeSet<&str> {
        m.permissions
            .iter()
            .filter_map(|p| match (p, want_modify) {
                (Permission::HooksModify { hooks }, true)
                | (Permission::HooksObserve { hooks }, false) => Some(hooks),
                _ => None,
            })
            .flatten()
            .map(String::as_str)
            .collect()
    };
    let observed = hooks_of(false);
    for hook in hooks_of(true) {
        if !observed.contains(hook) {
            out.push(
                Invalid::manifest(
                    "/permissions",
                    "modifyWithoutObserve",
                    format!("hooks:modify {hook} needs hooks:observe {hook}"),
                )
                .with("hook", hook),
            );
        }
    }
}

fn list(
    path: &str,
    field: &str,
    items: &[&str],
    ok: fn(&str) -> bool,
    what: &str,
    out: &mut Vec<Invalid>,
) {
    if items.is_empty() {
        out.push(Invalid::manifest(
            &format!("{path}/{field}"),
            "emptyPermissionList",
            format!("{field} names at least one entry"),
        ));
    }
    let mut seen = BTreeSet::new();
    for (j, item) in items.iter().enumerate() {
        let p = format!("{path}/{field}/{j}");
        if !ok(item) {
            out.push(Invalid::manifest(
                &p,
                field,
                format!("{item:?} is not {what}"),
            ));
        }
        if !seen.insert(*item) {
            out.push(Invalid::manifest(
                &p,
                "duplicateEntry",
                format!("{item:?} is listed twice"),
            ));
        }
    }
}

/// `https://host[:port]`: lowercase DNS labels (two or more), an optional port 1–65535, no
/// path, query, user info or trailing slash.
pub(crate) fn origin_ok(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("https://") else {
        return false;
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty() && !p.starts_with('0') && p.parse::<u16>().is_ok_and(|n| n > 0)
    });
    let labels: Vec<&str> = host.split('.').collect();
    let label_ok = |l: &&str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    port_ok && host.len() <= 253 && labels.len() >= 2 && labels.iter().all(label_ok)
}

fn secret_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

fn hook_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_uppercase()
        && b.iter().all(u8::is_ascii_alphanumeric)
}
