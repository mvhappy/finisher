//! `fin library set` (plan 5D Task 14): the flags and the `library.set` payload they make; see
//! [`super::library`] for the command.

use clap::Args;
use fin_core::CoreError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::output::usage;

const FULLS: &[&str] = &["daily", "weekly", "manual"];

#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetArgs {
    /// The library's id or name.
    pub lib: String,
    /// A new name (unique per server, case-insensitively).
    #[arg(long, value_name = "n")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Index only files matching this glob (repeatable; replaces the list).
    #[arg(long, value_name = "glob")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    /// Index every file again (clears the include list).
    #[arg(long = "no-include")]
    #[serde(default)]
    pub no_include: bool,
    /// Skip names matching this glob (repeatable; replaces the list).
    #[arg(long, value_name = "glob")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Skip nothing beyond the defaults (clears the exclude list).
    #[arg(long = "no-exclude")]
    #[serde(default)]
    pub no_exclude: bool,
    /// The kinds to probe: `all` or a comma-separated list (media, image, svg, …).
    #[arg(long, value_name = "k")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    /// How often to look for changes: `demand` or a number of seconds.
    #[arg(long, value_name = "s")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<String>,
    /// How often to scan everything again: daily, weekly or manual.
    #[arg(long, value_name = "f")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<String>,
    /// Stop scanning and probing this library.
    #[arg(long, conflicts_with = "resume")]
    #[serde(default)]
    pub pause: bool,
    /// Start again after a pause.
    #[arg(long)]
    #[serde(default)]
    pub resume: bool,
    /// Human-only (HUMAN_ONLY, exit 7): read files a sync service keeps in the cloud.
    #[arg(long, value_name = "on|off")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hydrate: Option<String>,
    /// Human-only (HUMAN_ONLY, exit 7): a read bandwidth limit in MiB/s, or `off`.
    #[arg(long, value_name = "MiB/s|off")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<String>,
    /// Human-only (HUMAN_ONLY, exit 7): the paths other machines know the root by.
    #[arg(long, value_name = "path")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alias: Vec<String>,
    /// Human-only (HUMAN_ONLY, exit 7): forget every alias.
    #[arg(long = "no-aliases")]
    #[serde(default)]
    pub no_aliases: bool,
    /// Human-only (HUMAN_ONLY, exit 7): accept the volume now under the root.
    #[arg(long = "accept-volume")]
    #[serde(default)]
    pub accept_volume: bool,
    /// Human-only (HUMAN_ONLY, exit 7): a nested mount here is gone for good.
    #[arg(long = "accept-unmounted", value_name = "dir")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accept_unmounted: Option<String>,
    /// Human-only (HUMAN_ONLY, exit 7): the root is empty for good.
    #[arg(long = "accept-empty-root")]
    #[serde(default)]
    pub accept_empty_root: bool,
}

/// `scan {poll?, full?}` of `fin library set`'s flags.
fn scan_patch(a: &SetArgs) -> Result<Value, CoreError> {
    let mut scan = json!({});
    if let Some(p) = &a.poll {
        scan["poll"] = if p == "demand" {
            json!("demand")
        } else {
            match p.parse::<u64>() {
                Ok(s) if s > 0 => json!(s),
                _ => {
                    return Err(usage(
                        format!("--poll {p:?} is neither demand nor a number of seconds"),
                        json!({ "poll": p }),
                        "use --poll demand or --poll 300",
                    ));
                }
            }
        };
    }
    if let Some(f) = &a.full {
        if !FULLS.contains(&f.as_str()) {
            return Err(usage(
                format!("--full {f:?} is not a schedule"),
                json!({ "full": f, "schedules": FULLS }),
                format!("use one of {}", FULLS.join(", ")),
            ));
        }
        scan["full"] = json!(f);
    }
    Ok(scan)
}

fn conflict(a: &str, b: &str) -> CoreError {
    usage(
        format!("{a} and {b} cannot be given together"),
        json!({ "flags": [a, b] }),
        "give one of them",
    )
}

/// The `library.set` payload of `a` (what changes, checked before anything is sent).
pub fn set_payload(a: &SetArgs) -> Result<Value, CoreError> {
    let mut p = json!({ "id": a.lib });
    if let Some(n) = &a.name {
        p["name"] = json!(n);
    }
    match (a.include.is_empty(), a.no_include) {
        (false, true) => return Err(conflict("--include", "--no-include")),
        (false, false) => p["include"] = json!(a.include),
        (true, true) => p["include"] = Value::Null,
        (true, false) => {}
    }
    match (a.exclude.is_empty(), a.no_exclude) {
        (false, true) => return Err(conflict("--exclude", "--no-exclude")),
        (false, false) => p["exclude"] = json!(a.exclude),
        (true, true) => p["exclude"] = json!([]),
        (true, false) => {}
    }
    if let Some(k) = &a.kinds {
        p["kinds"] = if k == "all" {
            json!("all")
        } else {
            json!(k.split(',').filter(|s| !s.is_empty()).collect::<Vec<_>>())
        };
    }
    let scan = scan_patch(a)?;
    if scan.as_object().is_some_and(|o| !o.is_empty()) {
        p["scan"] = scan;
    }
    if a.pause {
        p["paused"] = json!(true);
    } else if a.resume {
        p["paused"] = json!(false);
    }
    // The fields only a person decides: sent as asked, refused by the server (HUMAN_ONLY).
    if let Some(h) = &a.hydrate {
        p["hydrate"] = match h.as_str() {
            "on" => json!(true),
            "off" => json!(false),
            _ => {
                return Err(usage(
                    format!("--hydrate {h:?} is neither on nor off"),
                    json!({ "hydrate": h }),
                    "use --hydrate on or --hydrate off",
                ));
            }
        };
    }
    if let Some(b) = &a.bandwidth {
        p["bandwidthMiBs"] = match b.as_str() {
            "off" => Value::Null,
            _ => match b.parse::<f64>() {
                Ok(n) if n.is_finite() && n > 0.0 => json!(n),
                _ => {
                    return Err(usage(
                        format!("--bandwidth {b:?} is neither off nor a number of MiB/s"),
                        json!({ "bandwidth": b }),
                        "use --bandwidth 20 or --bandwidth off",
                    ));
                }
            },
        };
    }
    match (a.alias.is_empty(), a.no_aliases) {
        (false, true) => return Err(conflict("--alias", "--no-aliases")),
        (false, false) => p["aliases"] = json!(a.alias),
        (true, true) => p["aliases"] = json!([]),
        (true, false) => {}
    }
    if a.accept_volume {
        p["acceptVolume"] = json!(true);
    }
    if let Some(d) = &a.accept_unmounted {
        p["acceptUnmounted"] = json!({ "dir": d });
    }
    if a.accept_empty_root {
        p["acceptEmptyRoot"] = json!(true);
    }
    if p.as_object().is_some_and(|o| o.len() == 1) {
        return Err(usage(
            "fin library set needs something to change",
            json!({ "lib": a.lib }),
            "give --name, --include, --exclude, --kinds, --poll, --full, --pause or --resume",
        ));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    type Edit = Box<dyn FnOnce(&mut SetArgs)>;

    fn set(f: impl FnOnce(&mut SetArgs)) -> SetArgs {
        let mut a = SetArgs {
            lib: "raw".into(),
            name: None,
            include: vec![],
            no_include: false,
            exclude: vec![],
            no_exclude: false,
            kinds: None,
            poll: None,
            full: None,
            pause: false,
            resume: false,
            hydrate: None,
            bandwidth: None,
            alias: vec![],
            no_aliases: false,
            accept_volume: false,
            accept_unmounted: None,
            accept_empty_root: false,
        };
        f(&mut a);
        a
    }

    #[test]
    fn set_sends_what_was_asked() {
        let p = set_payload(&set(|a| {
            a.name = Some("Footage".into());
            a.include = vec!["*.mov".into()];
            a.no_exclude = true;
            a.kinds = Some("media,image".into());
            a.poll = Some("300".into());
            a.full = Some("weekly".into());
            a.pause = true;
        }))
        .unwrap();
        assert_eq!(
            p,
            json!({
                "id": "raw", "name": "Footage", "include": ["*.mov"], "exclude": [],
                "kinds": ["media", "image"], "scan": { "poll": 300, "full": "weekly" },
                "paused": true,
            })
        );
        let clear = set_payload(&set(|a| {
            a.no_include = true;
            a.kinds = Some("all".into());
            a.poll = Some("demand".into());
            a.resume = true;
        }))
        .unwrap();
        assert_eq!(
            clear,
            json!({ "id": "raw", "include": null, "kinds": "all", "scan": { "poll": "demand" }, "paused": false })
        );
    }

    #[test]
    fn human_only_fields_are_sent_for_the_server_to_refuse() {
        let p = set_payload(&set(|a| {
            a.hydrate = Some("on".into());
            a.bandwidth = Some("off".into());
            a.alias = vec!["/mnt/x".into()];
            a.accept_volume = true;
            a.accept_unmounted = Some("mount".into());
            a.accept_empty_root = true;
        }))
        .unwrap();
        assert_eq!(
            p,
            json!({
                "id": "raw", "hydrate": true, "bandwidthMiBs": null, "aliases": ["/mnt/x"],
                "acceptVolume": true, "acceptUnmounted": { "dir": "mount" }, "acceptEmptyRoot": true,
            })
        );
        assert_eq!(
            set_payload(&set(|a| a.no_aliases = true)).unwrap()["aliases"],
            json!([])
        );
    }

    #[test]
    fn set_checks_its_flags_before_sending() {
        let bad: [Edit; 8] = [
            Box::new(|_| {}),
            Box::new(|a| a.poll = Some("soon".into())),
            Box::new(|a| a.poll = Some("0".into())),
            Box::new(|a| a.full = Some("hourly".into())),
            Box::new(|a| a.hydrate = Some("maybe".into())),
            Box::new(|a| a.bandwidth = Some("fast".into())),
            Box::new(|a| {
                a.include = vec!["*.mov".into()];
                a.no_include = true;
            }),
            Box::new(|a| {
                a.alias = vec!["/x".into()];
                a.no_aliases = true;
            }),
        ];
        for f in bad {
            assert_eq!(set_payload(&set(f)).unwrap_err().code, "USAGE");
        }
    }
}
