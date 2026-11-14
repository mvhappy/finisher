//! `@example/rust-tools`: an example native plugin on `fin-plugin-sdk` (plan 8B Task 12).
//!
//! - `trim-silence` (`writes`): reads the project at the job's `rev` and proposes an
//!   `item.delete` for every item of the main timeline's video and audio tracks that lies wholly
//!   inside a marker span labelled `label` (default `silence`), sorted by (track order, start);
//!   `mode` is `lift` (default) or `ripple`. It never edits markers, captions or other
//!   timelines. No such item answers an empty transaction. `ripple` closes each gap on its own
//!   track only (fin-core's `item.delete`), so tracks whose silent items differ end up shifted
//!   by different amounts: an example of the op, not a sync-safe edit. (A linked item's delete
//!   also takes its group's members, so a group with two silent members would name one twice;
//!   the example assumes unlinked items.)
//! - `fetch-title` (read-only): `GET https://api.example.org<path>` (default `/`) through the
//!   host's broker and returns `{status, title}`, `title` being the trimmed text of the first
//!   `<title>` element, or `null`.

use fin_plugin_sdk::fin_core::model::{Project, TrackKind};
use fin_plugin_sdk::fin_core::{DeleteMode, ItemDelete, Op, Rational};
use fin_plugin_sdk::prelude::*;

/// The one origin the manifest's `net` permission names.
const ORIGIN: &str = "https://api.example.org";

#[derive(Default)]
struct RustTools;

impl Plugin for RustTools {
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError> {
        match call.name.as_str() {
            "trim-silence" => {
                let label = arg_str(&call, "label").unwrap_or("silence");
                let mode = match arg_str(&call, "mode") {
                    Some("ripple") => DeleteMode::Ripple,
                    _ => DeleteMode::Lift,
                };
                let project = host.snapshot()?;
                let ops = silent_items(&project, label)
                    .into_iter()
                    .map(|item| Op::ItemDelete(ItemDelete { item, mode }))
                    .collect::<Vec<_>>();
                host.log(Level::Info, &format!("trim-silence: {} items", ops.len()));
                Ok(CommandOut::Tx { ops })
            }
            "fetch-title" => {
                let path = arg_str(&call, "path").unwrap_or("/");
                let response = host.fetch(http_get(format!("{ORIGIN}{path}")))?;
                let title = title_of(&response.text());
                Ok(CommandOut::Data {
                    value: json!({ "status": response.status, "title": title }),
                })
            }
            _ => Err(call.unknown()),
        }
    }
}

fn arg_str<'a>(call: &'a CommandCall, name: &str) -> Option<&'a str> {
    call.args.get(name).and_then(Value::as_str)
}

/// The ids of the main timeline's video and audio items wholly inside a span marker labelled
/// `label`.
fn silent_items(project: &Project, label: &str) -> Vec<String> {
    let Some(timeline) = project.timelines.first() else {
        return Vec::new();
    };
    let spans: Vec<(Rational, Rational)> = timeline
        .markers
        .iter()
        .filter(|m| m.label == label)
        .filter_map(|m| Some((m.at, m.at.checked_add(m.duration?).ok()?)))
        .collect();
    let mut found = Vec::new();
    for (index, track) in timeline.tracks.iter().enumerate() {
        if track.kind == TrackKind::Caption {
            continue;
        }
        for item in &track.items {
            let Ok(end) = item.end() else { continue };
            if spans.iter().any(|&(a, b)| a <= item.start && end <= b) {
                found.push((index, item.start, item.id.clone()));
            }
        }
    }
    // Explicitly by (track, start): the snapshot's stored order is not relied on.
    found.sort_by_key(|a| (a.0, a.1));
    found.into_iter().map(|(_, _, id)| id).collect()
}

/// The trimmed text of the first `<title>` element (ASCII case-insensitive tag), if any.
fn title_of(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = html[start..end].trim();
    (!title.is_empty()).then(|| title.to_owned())
}

fin_plugin_sdk::main!(RustTools);
