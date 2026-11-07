//! Consent per purpose (plan D, Decisions 8–12, 14): the `privacy` settings, the environment
//! overrides, the build defaults, and [`resolve`], which turns them into the consent the
//! journal records under. Effective purpose = `DO_NOT_TRACK` ? off : `FIN_<PURPOSE>` ??
//! settings ?? default. An unusable `privacy` section keeps the last good settings; before
//! the first good read it means both off (fail closed).

use std::path::Path;
use std::sync::atomic::Ordering::{Relaxed, Release};

use fin_core::events::{Basis, Purpose, Purposes};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::control::{Control, Stamp, pack};
use crate::handle::ConsentCtl;
use crate::{Consent, Journal, JournalOptions};

/// One purpose's switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Switch {
    On,
    Off,
}

/// Training's state: `undecided` until a person answers (the onboarding question). It
/// records no content, exactly like `off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrainingState {
    On,
    Off,
    Undecided,
}

/// `settings.json` `privacy` (Decision 8). Absent keys fall through to the defaults; unknown
/// keys are ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PrivacySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Switch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub training: Option<Switch>,
    /// The age cap in days, 1–365 (default 30).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 365))]
    pub retention_days: Option<u32>,
    /// The size cap in MiB over the files and blobs, 16–4096 (default 128).
    #[serde(default, rename = "maxMiB", skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 16, max = 4096))]
    pub max_mib: Option<u32>,
}

impl PrivacySettings {
    /// The section as `settings.json` holds it: absent or `null` is no settings; anything
    /// else must be an object of the right types and ranges.
    pub fn parse(v: Option<&Value>) -> Result<PrivacySettings, String> {
        let v = match v {
            None | Some(Value::Null) => return Ok(PrivacySettings::default()),
            Some(v @ Value::Object(_)) => v,
            Some(_) => return Err("privacy is not an object".into()),
        };
        let s: PrivacySettings =
            serde_json::from_value(v.clone()).map_err(|e| format!("privacy: {e}"))?;
        if s.retention_days.is_some_and(|d| !(1..=365).contains(&d)) {
            return Err("privacy.retentionDays is not 1–365".into());
        }
        if s.max_mib.is_some_and(|m| !(16..=4096).contains(&m)) {
            return Err("privacy.maxMiB is not 16–4096".into());
        }
        Ok(s)
    }
}

/// `finisherd`'s own environment, read once at start: `DO_NOT_TRACK`, `FIN_DIAGNOSTICS`,
/// `FIN_TRAINING`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverrides {
    pub do_not_track: bool,
    pub diagnostics: Option<Switch>,
    pub training: Option<Switch>,
    /// A value that was not understood (it counts as `off`).
    pub warnings: Vec<String>,
}

impl EnvOverrides {
    pub fn from_env() -> EnvOverrides {
        EnvOverrides::from_vars(|k| std::env::var(k).ok())
    }

    /// From any variable source (tests pass a map). `DO_NOT_TRACK` is set by any value but
    /// empty, `0` or `false`; `FIN_<PURPOSE>` is `on` or `off`, and anything else is `off`
    /// with a warning (fail closed).
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> EnvOverrides {
        let mut warnings = Vec::new();
        let dnt = get("DO_NOT_TRACK").is_some_and(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false")
        });
        let mut switch = |name: &str| {
            let v = get(name)?;
            match v.trim().to_ascii_lowercase().as_str() {
                "on" => Some(Switch::On),
                "off" => Some(Switch::Off),
                _ => {
                    warnings.push(format!("{name}={v:?} is not on or off: read as off"));
                    Some(Switch::Off)
                }
            }
        };
        let diagnostics = switch("FIN_DIAGNOSTICS");
        let training = switch("FIN_TRAINING");
        EnvOverrides {
            do_not_track: dnt,
            diagnostics,
            training,
            warnings,
        }
    }
}

/// The build defaults (the owner's decisions of 2026-11-07): basic diagnostics on
/// everywhere, training undecided. Training is never on by default
/// (`content_needs_explicit_consent`), so content always has basis `user` or `env`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    pub diagnostics: Switch,
    /// `None` is undecided.
    pub training: Option<Switch>,
}

pub const DEFAULTS: Defaults = Defaults {
    diagnostics: Switch::On,
    training: None,
};

/// The effective consent as `fin diagnostics show` and `diagnostics.consent` report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConsentState {
    pub diagnostics: Switch,
    pub training: TrainingState,
    // The stamp's basis. Not a doc comment: it would inline `Basis` in the generated types.
    pub basis: Basis,
    /// The consent revision: monotonic, persisted in `events/state.json`.
    pub rev: u64,
    // The purposes an environment override decides (both under `DO_NOT_TRACK`).
    pub env: Purposes,
    pub do_not_track: bool,
}

impl ConsentState {
    pub fn purposes(&self) -> Purposes {
        let mut p = Purposes::NONE;
        if self.diagnostics == Switch::On {
            p = p.with(Purpose::Diagnostics);
        }
        if self.training == TrainingState::On {
            p = p.with(Purpose::Training);
        }
        p
    }

    pub fn consent(&self) -> Consent {
        Consent {
            purposes: self.purposes(),
            rev: self.rev,
            basis: self.basis,
        }
    }

    /// The same decision, whatever the revision.
    pub(crate) fn same(&self, other: &ConsentState) -> bool {
        ConsentState { rev: 0, ..*self } == ConsentState { rev: 0, ..*other }
    }

    /// A low-level [`Consent`] as a state (training on or off, no environment).
    pub(crate) fn of(c: Consent) -> ConsentState {
        ConsentState {
            diagnostics: if c.purposes.contains(Purpose::Diagnostics) {
                Switch::On
            } else {
                Switch::Off
            },
            training: if c.purposes.has_training() {
                TrainingState::On
            } else {
                TrainingState::Off
            },
            basis: c.basis,
            rev: c.rev,
            env: Purposes::NONE,
            do_not_track: false,
        }
    }
}

/// The stamp's basis. Training covers diagnostics-class fields too (Decision 10), so with
/// training on a diagnostics `default` does not weaken it: content never has basis `default`
/// (Decision 14). Otherwise the weaker of the purposes that are on, or of both when none is.
fn stamp_basis(diag: (Switch, Basis), training: (TrainingState, Basis)) -> Basis {
    let (d_on, t_on) = (diag.0 == Switch::On, training.0 == TrainingState::On);
    match (d_on, t_on) {
        (true, true) if diag.1 == Basis::Default => training.1,
        (true, true) => diag.1.weaker(training.1),
        (false, true) => training.1,
        (true, false) => diag.1,
        (false, false) => diag.1.weaker(training.1),
    }
}

/// [`resolve`] with the full state ([`ConsentState`], `rev` 0: the journal numbers it).
pub fn resolve_state(
    privacy: Option<&Value>,
    env: &EnvOverrides,
    last_good: Option<&PrivacySettings>,
) -> (ConsentState, Option<PrivacySettings>, Option<String>) {
    let (settings, warning) = match PrivacySettings::parse(privacy) {
        Ok(s) => (Some(s), None),
        Err(m) => (
            last_good.cloned(),
            Some(match last_good {
                Some(_) => format!("{m}; keeping the last good privacy settings"),
                None => format!("{m}; recording nothing until it is fixed"),
            }),
        ),
    };
    let mut env_p = Purposes::NONE;
    let diag = if env.do_not_track {
        env_p = env_p.with(Purpose::Diagnostics);
        (Switch::Off, Basis::Env)
    } else if let Some(v) = env.diagnostics {
        env_p = env_p.with(Purpose::Diagnostics);
        (v, Basis::Env)
    } else {
        match &settings {
            None => (Switch::Off, Basis::Default),
            Some(PrivacySettings {
                diagnostics: Some(v),
                ..
            }) => (*v, Basis::User),
            Some(_) => (DEFAULTS.diagnostics, Basis::Default),
        }
    };
    let on = |s: Switch| match s {
        Switch::On => TrainingState::On,
        Switch::Off => TrainingState::Off,
    };
    let training = if env.do_not_track {
        env_p = env_p.with(Purpose::Training);
        (TrainingState::Off, Basis::Env)
    } else if let Some(v) = env.training {
        env_p = env_p.with(Purpose::Training);
        (on(v), Basis::Env)
    } else {
        match &settings {
            None => (TrainingState::Off, Basis::Default),
            Some(PrivacySettings {
                training: Some(v), ..
            }) => (on(*v), Basis::User),
            Some(_) => (
                DEFAULTS.training.map_or(TrainingState::Undecided, on),
                Basis::Default,
            ),
        }
    };
    let state = ConsentState {
        diagnostics: diag.0,
        training: training.0,
        basis: stamp_basis(diag, training),
        rev: 0,
        env: env_p,
        do_not_track: env.do_not_track,
    };
    (state, settings, warning)
}

/// The consent in force from the raw `privacy` section, the environment and the last good
/// settings: the consent (`rev` 0; the journal numbers it), the new last good settings, and
/// a warning when the section was unusable.
pub fn resolve(
    privacy: Option<&Value>,
    env: &EnvOverrides,
    last_good: Option<&PrivacySettings>,
) -> (Consent, Option<PrivacySettings>, Option<String>) {
    let (s, good, warning) = resolve_state(privacy, env, last_good);
    (s.consent(), good, warning)
}

/// The caps from the settings (`retentionDays`, `maxMiB`), else the options' (ms, bytes).
pub(crate) fn caps_of(s: Option<&PrivacySettings>, age_ms: u64, total: u64) -> (u64, u64) {
    let age = s
        .and_then(|s| s.retention_days)
        .map_or(age_ms, |d| u64::from(d) * 86_400_000);
    let total = s
        .and_then(|s| s.max_mib)
        .map_or(total, |m| u64::from(m) << 20);
    (age, total)
}

impl Journal {
    /// Resolves the raw `privacy` section with `env` (read once at start) and opens the
    /// journal under the result (Decision 12: nothing is ever written under the build
    /// default when the user said off). The settings' caps replace the options'. Returns the
    /// warning of an unusable section (it is then off: there is no last good one yet).
    pub fn open_with_settings(
        dir: &Path,
        privacy: Option<&Value>,
        env: EnvOverrides,
        opts: JournalOptions,
    ) -> (Journal, Option<String>) {
        let (state, last_good, warning) = resolve_state(privacy, &env, None);
        let ctl = ConsentCtl {
            env,
            last_good,
            state,
            base_age_ms: opts.max_age.as_millis() as u64,
            base_total: opts.max_total_bytes,
        };
        (Journal::open_ctl(dir, ctl, opts), warning)
    }

    /// Applies a new raw `privacy` section (the settings watch). An unusable one keeps the
    /// last good settings and changes nothing (no new consent, no `rev` bump); its warning is
    /// returned. A change stores the new gate at once, so callers stop building payloads for
    /// a withdrawn purpose, then journals `consent.changed` under the old consent on the
    /// control channel, which never drops.
    pub fn apply_settings(&self, privacy: Option<&Value>) -> Option<String> {
        let h = self.h.as_ref()?;
        let mut c = h.sh.lock_consent();
        let (mut next, good, warning) = resolve_state(privacy, &c.env, c.last_good.as_ref());
        c.last_good = good;
        let (age, total) = caps_of(c.last_good.as_ref(), c.base_age_ms, c.base_total);
        let age_changed = h.sh.caps_age.swap(age, Relaxed) != age;
        let total_changed = h.sh.caps_total.swap(total, Relaxed) != total;
        let caps_changed = age_changed || total_changed;
        if caps_changed {
            h.control(Control::Caps);
        }
        if next.same(&c.state) {
            return warning;
        }
        next.rev = c.state.rev + 1;
        if !next.purposes().is_empty() {
            h.sh.ensure_key(&h.dir);
        }
        let from = std::mem::replace(&mut c.state, next);
        let stamp = Stamp {
            purposes: next.purposes(),
            rev: next.rev,
            basis: next.basis,
        };
        h.sh.word.store(pack(stamp), Release);
        h.control(Control::Consent { from, to: next });
        warning
    }

    /// The consent in force.
    pub fn consent(&self) -> ConsentState {
        match &self.h {
            Some(h) => h.sh.lock_consent().state,
            None => ConsentState::of(Consent::OFF),
        }
    }
}
