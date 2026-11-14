//! Consent per purpose (plan D, Task D3; Decisions 6–12, 14): precedence, defaults, the
//! last good settings, the revision, and the install key.

mod support;

use std::collections::HashMap;

use fin_core::events::{Basis, Purposes, Source};
use fin_journal::{
    ConsentState, DEFAULTS, EnvOverrides, Journal, Switch, TrainingState, resolve, resolve_state,
};
use serde_json::{Value, json};
use support::*;

fn env(vars: &[(&str, &str)]) -> EnvOverrides {
    let m: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    EnvOverrides::from_vars(|k| m.get(k).cloned())
}

fn state(privacy: Value, vars: &[(&str, &str)]) -> ConsentState {
    resolve_state(Some(&privacy), &env(vars), None).0
}

/// Settings, env, expected diagnostics, training, basis and env-decided purposes.
type Row = (
    Value,
    &'static [(&'static str, &'static str)],
    Switch,
    TrainingState,
    Basis,
    Purposes,
);

#[test]
fn resolve_precedence() {
    use Switch::{Off, On};
    use TrainingState as T;
    let table: &[Row] = &[
        // Defaults: diagnostics on, training undecided.
        (
            json!({}),
            &[],
            On,
            T::Undecided,
            Basis::Default,
            Purposes::NONE,
        ),
        (
            json!(null),
            &[],
            On,
            T::Undecided,
            Basis::Default,
            Purposes::NONE,
        ),
        // Settings beat the defaults.
        // Both off: the weaker basis of the two.
        (
            json!({"diagnostics": "off"}),
            &[],
            Off,
            T::Undecided,
            Basis::Default,
            Purposes::NONE,
        ),
        (
            json!({"training": "on"}),
            &[],
            On,
            T::On,
            Basis::User,
            Purposes::NONE,
        ),
        (
            json!({"training": "off"}),
            &[],
            On,
            T::Off,
            Basis::Default,
            Purposes::NONE,
        ),
        // The environment beats the settings, either way.
        (
            json!({"diagnostics": "on", "training": "on"}),
            &[("FIN_TRAINING", "off")],
            On,
            T::Off,
            Basis::User,
            Purposes::TRAINING,
        ),
        (
            json!({"diagnostics": "off"}),
            &[("FIN_DIAGNOSTICS", "on")],
            On,
            T::Undecided,
            Basis::Env,
            Purposes::DIAGNOSTICS,
        ),
        (
            json!({}),
            &[("FIN_TRAINING", "on")],
            On,
            T::On,
            Basis::Env,
            Purposes::TRAINING,
        ),
        // A value the env does not understand is off.
        (
            json!({}),
            &[("FIN_DIAGNOSTICS", "yes")],
            Off,
            T::Undecided,
            Basis::Default,
            Purposes::DIAGNOSTICS,
        ),
        // DO_NOT_TRACK beats everything: off wins.
        (
            json!({"diagnostics": "on", "training": "on"}),
            &[("DO_NOT_TRACK", "1"), ("FIN_TRAINING", "on")],
            Off,
            T::Off,
            Basis::Env,
            Purposes::ALL,
        ),
        // `DO_NOT_TRACK=0` is not set.
        (
            json!({}),
            &[("DO_NOT_TRACK", "0")],
            On,
            T::Undecided,
            Basis::Default,
            Purposes::NONE,
        ),
    ];
    for (i, (privacy, vars, d, t, basis, env_p)) in table.iter().enumerate() {
        let s = state(privacy.clone(), vars);
        assert_eq!((s.diagnostics, s.training), (*d, *t), "row {i}: {s:?}");
        assert_eq!((s.basis, s.env), (*basis, *env_p), "row {i}: {s:?}");
    }
    // The low-level form agrees.
    let (c, good, warn) = resolve(Some(&json!({"training": "on"})), &env(&[]), None);
    assert_eq!(c.purposes, Purposes::ALL);
    assert_eq!(c.basis, Basis::User);
    assert!(good.is_some() && warn.is_none());
}

#[test]
fn content_needs_explicit_consent() {
    assert_ne!(
        DEFAULTS.training,
        Some(Switch::On),
        "training is never on by default"
    );
    let s = state(json!({}), &[]);
    assert_eq!(s.training, TrainingState::Undecided);
    assert!(!s.purposes().has_training());
}

#[test]
fn undecided_records_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let (j, w) = Journal::open_with_settings(&events(&dir), Some(&json!({})), env(&[]), opts(&c));
    assert!(w.is_none());
    assert_eq!(j.consent().training, TrainingState::Undecided);
    j.record(Source::FinNode, None, commit_with_intent(3));
    let all = read_all(&j);
    assert_eq!(all.len(), 1);
    assert!(all[0].payload.get("intent").is_none());
    assert_eq!(all[0].consent.purposes, Purposes::DIAGNOSTICS);
}

#[test]
fn content_entries_never_basis_default() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    // Diagnostics by default, training by the user: content carries the user's basis.
    let (j, _) = Journal::open_with_settings(
        &events(&dir),
        Some(&json!({"training": "on"})),
        env(&[]),
        opts(&c),
    );
    j.record(Source::FinNode, None, commit_with_intent(2));
    j.record(Source::FinNode, None, commit(1));
    for e in read_all(&j) {
        if e.consent.purposes.has_training() {
            assert_ne!(e.consent.basis, Basis::Default, "{e:?}");
        }
    }
    // Training from the environment: basis env.
    let dir = tempfile::tempdir().unwrap();
    let (j, _) = Journal::open_with_settings(
        &events(&dir),
        None,
        env(&[("FIN_TRAINING", "on")]),
        opts(&c),
    );
    j.record(Source::FinNode, None, commit_with_intent(2));
    let all = read_all(&j);
    assert!(all[0].payload.get("intent").is_some());
    assert_eq!(all[0].consent.basis, Basis::Env);
}

#[test]
fn open_with_settings_off_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let (j, _) = Journal::open_with_settings(
        &events(&dir),
        Some(&json!({"diagnostics": "off"})),
        env(&[]),
        opts(&c),
    );
    assert_eq!(j.gate(), None);
    j.record(Source::FinNode, None, commit(1));
    assert!(j.hash("x").is_none());
    j.flush().unwrap();
    j.close();
    assert!(
        !events(&dir).exists(),
        "no events directory, no key, no state"
    );
}

#[test]
fn unusable_privacy_before_first_good_read_means_off() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    for bad in [
        json!("on"),
        json!({"diagnostics": 1}),
        json!({"retentionDays": 0}),
        json!({"maxMiB": 8}),
    ] {
        let (j, w) = Journal::open_with_settings(&events(&dir), Some(&bad), env(&[]), opts(&c));
        assert!(w.is_some(), "{bad}: a warning");
        assert_eq!(j.gate(), None, "{bad}: fail closed");
        j.close();
    }
    assert!(!events(&dir).exists());
}

#[test]
fn unusable_privacy_after_good_read_keeps_last_good() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let good = json!({"training": "on", "retentionDays": 7});
    let (j, _) = Journal::open_with_settings(&events(&dir), Some(&good), env(&[]), opts(&c));
    let before = j.consent();
    let w = j.apply_settings(Some(&json!({"training": "maybe"})));
    assert!(w.unwrap().contains("last good"));
    assert_eq!(j.consent(), before);
    assert_eq!(j.gate(), Some(Purposes::ALL));
    // A good read after it applies.
    assert!(
        j.apply_settings(Some(&json!({"training": "off"})))
            .is_none()
    );
    assert_eq!(j.consent().training, TrainingState::Off);
}

#[test]
fn bad_read_does_not_bump_rev() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let (j, _) = Journal::open_with_settings(&events(&dir), Some(&json!({})), env(&[]), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    let rev = j.consent().rev;
    j.apply_settings(Some(&json!([])));
    j.apply_settings(Some(&json!({"diagnostics": "sometimes"})));
    assert_eq!(j.consent().rev, rev);
    // Re-reading the same settings is no change either.
    j.apply_settings(Some(&json!({})));
    assert_eq!(j.consent().rev, rev);
    let all = read_all(&j);
    assert!(of_kind(&all, "consent.changed").is_empty());
}

#[test]
fn consent_rev_is_monotonic_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let open =
        |p: Value| Journal::open_with_settings(&events(&dir), Some(&p), env(&[]), opts(&c)).0;
    let j = open(json!({}));
    j.record(Source::FinNode, None, commit(1));
    let r0 = j.consent().rev;
    j.apply_settings(Some(&json!({"training": "on"})));
    let r1 = j.consent().rev;
    assert!(r1 > r0);
    j.close();
    // The same settings: the same revision.
    let j = open(json!({"training": "on"}));
    assert_eq!(j.consent().rev, r1);
    j.close();
    // Different settings at start: the next revision, never an earlier one.
    let j = open(json!({}));
    let r2 = j.consent().rev;
    assert!(r2 > r1, "{r2} > {r1}");
    j.record(Source::FinNode, None, commit(2));
    j.close();
    let j = open(json!({}));
    assert_eq!(j.consent().rev, r2);
    // The stamps follow.
    let revs: Vec<u64> = read_all(&j).iter().map(|e| e.consent.rev).collect();
    assert!(revs.windows(2).all(|w| w[0] <= w[1]), "{revs:?}");
}

#[test]
fn keyed_hash_is_stable_and_not_plain_blake3() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let a = j.hash("someone@host").unwrap();
    j.record(Source::FinNode, None, commit(1));
    j.close();
    assert!(events(&dir).join("key").exists());
    let plain = blake3::hash(b"someone@host");
    assert!(!a.as_str().contains(&plain.to_hex()[..32]));
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    assert_eq!(j.hash("someone@host").unwrap(), a, "stable across reopen");
    assert_ne!(j.hash("someone@else").unwrap(), a);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(events(&dir).join("key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Another install has another key.
    let other = tempfile::tempdir().unwrap();
    let k = Journal::open(&events(&other), diagnostics(), opts(&c));
    assert_ne!(k.hash("someone@host").unwrap(), a);
}

#[test]
fn no_key_created_while_off() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), fin_journal::Consent::OFF, opts(&c));
    assert!(j.hash("x").is_none());
    assert!(j.id_ref("x").is_none());
    j.close();
    assert!(!events(&dir).exists());
    // Turned on, the key is made with the first write, not before.
    let (j, _) = Journal::open_with_settings(
        &events(&dir),
        Some(&json!({"diagnostics": "off"})),
        env(&[]),
        opts(&c),
    );
    j.apply_settings(Some(&json!({})));
    assert!(j.hash("x").is_some());
    assert!(!events(&dir).join("key").exists());
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    assert!(events(&dir).join("key").exists());
}

#[test]
fn non_ulid_ids_are_hashed() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let ulid = "01JC0000000000000000000001";
    assert_eq!(j.id_ref(ulid).unwrap().as_str(), ulid);
    for raw in ["idem-key-1", "alice-laptop-session", &ulid.to_lowercase()] {
        let r = j.id_ref(raw).unwrap();
        assert!(r.is_hashed(), "{raw} -> {r:?}");
        assert!(!r.as_str().contains(raw));
    }
}

#[test]
fn server_scrub_names_the_machine() {
    let data = std::path::Path::new("/srv/fin-data");
    let s = fin_journal::server_scrub(data, Some("test-host".into()));
    assert_eq!(s.server_data.as_deref(), Some("/srv/fin-data"));
    assert_eq!(s.host.as_deref(), Some("test-host"));
    assert!(s.tmp.is_some());
    let text = fin_core::events::scrub::scrub("failed at /srv/fin-data/x on test-host", &s);
    assert!(
        !text.contains("fin-data") && !text.contains("test-host"),
        "{text}"
    );
}
