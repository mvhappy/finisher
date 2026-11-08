//! Signed advisories (spec §6.5): per publisher in `<publisher>/advisories.json` (signed by
//! the publisher's role) and registry-wide in `advisories.json` (signed by top-level
//! `targets`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum AdvisoryAction {
    /// Shown with the plugin; nothing changes.
    Warn,
    /// The plugin is disabled on refresh by any principal (`PLUGIN_REVOKED`).
    Disable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Advisory {
    pub id: String,
    /// The plugin id, `@publisher/name`.
    pub plugin: String,
    /// A semver range of affected versions.
    pub versions: String,
    pub severity: Severity,
    pub action: AdvisoryAction,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// The longest `summary` an advisory may have, in characters: one line a person reads on the
/// approval screen.
pub const MAX_SUMMARY_CHARS: usize = 500;
const MAX_ID_CHARS: usize = 128;
const MAX_URL_CHARS: usize = 512;

/// Whether `c` could rewrite what a terminal shows: a C0 or C1 control (an escape sequence,
/// a carriage return, a newline), or a bidirectional override, embedding, isolate or mark.
fn unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

impl Advisory {
    /// Metadata a person will read must be plain text of a bounded size (review M1 of Tasks
    /// 10 and 12): `Err(field)` names the first field that is too long or holds a control or
    /// bidirectional-formatting character. Run when advisories are parsed from signed
    /// metadata and when the editor signs one.
    pub fn check(&self) -> Result<(), &'static str> {
        let fields: [(&'static str, &str, usize); 5] = [
            ("id", &self.id, MAX_ID_CHARS),
            ("plugin", &self.plugin, MAX_ID_CHARS),
            ("versions", &self.versions, MAX_ID_CHARS),
            ("summary", &self.summary, MAX_SUMMARY_CHARS),
            ("url", self.url.as_deref().unwrap_or(""), MAX_URL_CHARS),
        ];
        for (name, text, max) in fields {
            if text.chars().count() > max || text.chars().any(unsafe_char) {
                return Err(name);
            }
        }
        Ok(())
    }
}

/// The content of an advisories target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvisoryFile {
    pub advisories: Vec<Advisory>,
}

/// An advisory with the role that signed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignedAdvisory {
    /// `targets` for registry-wide advisories, else the publisher.
    pub signed_by: String,
    #[serde(flatten)]
    pub advisory: Advisory,
}

impl SignedAdvisory {
    /// Whether this advisory may speak about `plugin`: registry-wide ones about any plugin,
    /// a publisher's only about its own (spec §6.5; `@a` never vouches for `@b`).
    pub fn in_scope(&self) -> bool {
        if self.signed_by == "targets" {
            return fin_plugin_api::PluginId::parse(&self.advisory.plugin).is_ok();
        }
        fin_plugin_api::PluginId::parse(&self.advisory.plugin)
            .is_ok_and(|id| id.publisher() == self.signed_by)
    }

    /// Whether it applies to `plugin`@`version`. A range that does not parse matches nothing.
    pub fn applies_to(&self, plugin: &str, version: &semver::Version) -> bool {
        self.in_scope()
            && self.advisory.plugin == plugin
            && semver::VersionReq::parse(&self.advisory.versions)
                .is_ok_and(|req| req.matches(version))
    }
}

/// The in-scope advisories that apply to `plugin`@`version`.
pub fn applicable<'a>(
    all: &'a [SignedAdvisory],
    plugin: &str,
    version: &semver::Version,
) -> Vec<&'a SignedAdvisory> {
    all.iter()
        .filter(|a| a.applies_to(plugin, version))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tuf::trusted::advisory_file;

    fn adv(summary: &str) -> Advisory {
        Advisory {
            id: "ACME-1".into(),
            plugin: "@acme/pack".into(),
            versions: "<2.0.0".into(),
            severity: Severity::High,
            action: AdvisoryAction::Warn,
            summary: summary.into(),
            url: None,
        }
    }

    /// Review M1 (Tasks 10 and 12): advisory text reaches the approval screen.
    #[test]
    fn check_rejects_controls_bidi_and_long_text() {
        assert_eq!(adv("Leaks the project folder. Update.").check(), Ok(()));
        assert_eq!(adv("é — ünïcode and 日本語 are plain text").check(), Ok(()));
        for bad in [
            "clear\u{1b}[2J the screen",
            "carriage\rreturn",
            "two\nlines",
            "c1 \u{9b}31m",
            "bell\u{7}",
            "nul\u{0}",
            "del\u{7f}",
            "bidi \u{202e}override",
            "isolate \u{2066}x",
            "mark \u{200f}x",
        ] {
            assert_eq!(adv(bad).check(), Err("summary"), "{bad:?}");
        }
        assert_eq!(adv(&"x".repeat(MAX_SUMMARY_CHARS)).check(), Ok(()));
        assert_eq!(
            adv(&"x".repeat(MAX_SUMMARY_CHARS + 1)).check(),
            Err("summary")
        );
        let mut a = adv("ok");
        a.url = Some("https://example.org/\u{1b}]0;x".into());
        assert_eq!(a.check(), Err("url"));
        a.url = None;
        a.id = "A\u{85}B".into();
        assert_eq!(a.check(), Err("id"));
    }

    #[test]
    fn parsing_signed_metadata_applies_the_same_rule() {
        let file = |summary: &str| {
            serde_json::to_vec(&AdvisoryFile {
                advisories: vec![adv(summary)],
            })
            .unwrap()
        };
        assert!(advisory_file("advisories.json", &file("fine")).is_ok());
        let e = advisory_file("advisories.json", &file("x\u{1b}[31m")).unwrap_err();
        assert_eq!(e.code, "REGISTRY_INVALID");
        assert_eq!(e.details["reason"], "advisories");
        assert_eq!(e.details["field"], "summary");
        let e = advisory_file("advisories.json", &file(&"y".repeat(10_000))).unwrap_err();
        assert_eq!(e.details["field"], "summary");
    }
}
