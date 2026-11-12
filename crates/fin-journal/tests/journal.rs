//! The journal's basic contract (plan D, Task D2): off touches nothing, entries round-trip,
//! torn tails are repaired, bad lines are skipped and counted, an unusable directory or a
//! writer panic suspends instead of failing, `record_sync` is on disk or times out, the file
//! modes, and no network crate in the tree.

mod support;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fin_core::events::payloads::SessionEnd;
use fin_core::events::{
    Actor, ActorKind, AnyPayload, BranchId, ConsentStamp, Ctx, IdRef, ProjectId, Purposes, Source,
    decode_payload, kind,
};
use fin_journal::{CollectPolicy, Consent, Filter, Journal, JournalOptions, JournalState, Metric};
use support::*;

const PROJECT: &str = "01jb0000000000000000000000";
const TX: &str = "01JC0000000000000000000001";

fn ctx() -> Ctx {
    Ctx {
        project: ProjectId::parse(PROJECT),
        branch: BranchId::parse("br1"),
        tx_id: IdRef::classify(TX).ok(),
        author: None,
        actor: Some(Actor {
            kind: ActorKind::Agent,
            session: IdRef::classify("01JC0000000000000000000002").ok(),
            call: None,
        }),
    }
}

#[test]
fn disabled_journal_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::disabled();
    assert_eq!(j.gate(), None);
    j.record(Source::FinNode, None, commit(1));
    j.observe(Metric::Rpc, fin_core::events::Name::OTHER, 5, None);
    assert!(!j.record_sync(
        Source::FinNode,
        None,
        session_end(),
        Duration::from_millis(50)
    ));
    assert!(j.flush().is_ok());
    assert_eq!(j.status().state, JournalState::Off);
    assert_eq!(j.read(&Filter::default()).count(), 0);
    j.close();

    let c = clock();
    let off = Journal::open(&events(&dir), Consent::OFF, opts(&c));
    assert_eq!(off.gate(), None);
    for i in 0..1000 {
        off.record(Source::FinNode, Some(ctx()), commit(i));
        off.observe(Metric::Eval, fin_core::events::Name::OTHER, i, None);
    }
    assert!(!off.record_sync(
        Source::FinNode,
        None,
        session_end(),
        Duration::from_millis(50)
    ));
    c.advance(Duration::from_secs(3600));
    off.flush().unwrap();
    let s = off.status();
    assert_eq!(
        (s.state, s.files, s.bytes, s.dropped),
        (JournalState::Off, 0, 0, 0)
    );
    off.close();
    assert!(!events(&dir).exists(), "no events directory while off");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn record_then_read_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.set_project_policy(PROJECT, Some(CollectPolicy::Default));
    assert_eq!(j.gate(), Some(Purposes::DIAGNOSTICS));
    let p = commit(812);
    j.record(Source::FinNode, Some(ctx()), p.clone());
    c.advance(Duration::from_millis(5));
    j.record(Source::Finisherd, None, session_end());
    let all = read_all(&j);
    assert_eq!(all.len(), 2);
    let e = &all[0];
    assert_eq!(e.v, 1);
    assert_eq!(
        (e.kind.as_str(), e.kv, e.source),
        ("log.commit", 2, Source::FinNode)
    );
    assert_eq!(e.ts, T0 + 3_600_000);
    assert_eq!(Some(e.session.as_str()), j.session());
    assert!(
        IdRef::classify(e.id.as_str()).is_ok(),
        "a canonical ULID id: {}",
        e.id
    );
    assert_eq!(
        e.consent,
        ConsentStamp {
            rev: 1,
            purposes: Purposes::DIAGNOSTICS,
            basis: fin_core::events::Basis::Default
        }
    );
    assert_eq!(e.ctx, Some(ctx()));
    let back = decode_payload(kind("log.commit", 2).unwrap(), &e.payload).unwrap();
    assert_eq!(back, p);
    assert_eq!(all[1].kind.as_str(), "session.end");
    assert!(all[1].ctx.is_none());
    assert!(all[0].id < all[1].id, "ids increase in write order");
    let s = j.status();
    assert_eq!((s.state, s.files, s.dropped), (JournalState::On, 1, 0));
    assert_eq!(
        (s.oldest, s.newest),
        (Some(T0 + 3_600_000), Some(T0 + 3_600_005))
    );
    j.close();
    // Reopened, the same entries read back.
    let j = Journal::open(&events(&dir), Consent::OFF, opts(&c));
    assert_eq!(read_all(&j), all);
}

#[test]
fn record_value_validates_and_keeps_the_senders_session() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let session = "01JC00000000000000000000FF";
    let cli = serde_json::json!({
        "name": "item.trim", "flags": ["--dry-run"], "micros": 5, "exit": 0,
        "online": true, "dryRun": false,
    });
    j.record_value(Source::Fin, session, "cli.command", 1, None, cli.clone())
        .unwrap();
    let err = |r: Result<(), fin_core::CoreError>| r.unwrap_err().code;
    assert_eq!(
        err(j.record_value(Source::Fin, session, "cli.nope", 1, None, cli.clone())),
        "EVENT_KIND_UNKNOWN"
    );
    assert_eq!(
        err(j.record_value(Source::Fin, session, "cli.command", 2, None, cli.clone())),
        "EVENT_KIND_UNKNOWN"
    );
    assert_eq!(
        err(j.record_value(Source::Ui, session, "cli.command", 1, None, cli.clone())),
        "EVENT_INVALID"
    );
    assert_eq!(
        err(j.record_value(
            Source::Fin,
            "not-a-ulid",
            "cli.command",
            1,
            None,
            cli.clone()
        )),
        "EVENT_INVALID"
    );
    assert_eq!(
        err(j.record_value(
            Source::Fin,
            session,
            "cli.command",
            1,
            None,
            serde_json::json!({})
        )),
        "EVENT_INVALID"
    );
    let all = read_all(&j);
    assert_eq!(all.len(), 1);
    assert_eq!(
        (all[0].session.as_str(), all[0].source),
        (session, Source::Fin)
    );
    // Off, nothing is decoded and nothing is refused.
    let off = Journal::open(&dir.path().join("off"), Consent::OFF, opts(&c));
    off.record_value(Source::Ui, "x", "nope", 9, None, serde_json::json!(null))
        .unwrap();
}

#[test]
fn record_value_keys_wire_kinds_with_the_install_key() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    // No override: the per-install key, made for the journal (D3).
    let install = JournalOptions {
        key: None,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), diagnostics(), install);
    let session = "01JC00000000000000000000FF";
    let cli = serde_json::json!({
        "name": "item.trim", "micros": 5, "exit": 0, "online": true, "dryRun": false,
        "txIds": ["idem-1"],
    });
    j.record_value(Source::Fin, session, "cli.command", 1, None, cli)
        .unwrap();
    // A kind that is not a wire form needs no key.
    j.record_value(
        Source::FinNode,
        session,
        "session.end",
        1,
        None,
        serde_json::json!({ "uptimeS": 3 }),
    )
    .unwrap();
    let all = read_all(&j);
    assert_eq!(all.len(), 2);
    let want = j.id_ref("idem-1").unwrap();
    assert_eq!(all[0].payload["txIds"][0], want.as_str());
    // The raw id never reached the disk unkeyed.
    let text = files(&events(&dir))
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect::<String>();
    assert!(!text.contains("idem-1"));
}

#[test]
fn record_value_keys_the_wire_form() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let cli = serde_json::json!({
        "name": "item.trim", "micros": 5, "exit": 0, "online": true, "dryRun": false,
        "txIds": ["idem-1"],
    });
    j.record_value(
        Source::Fin,
        "01JC00000000000000000000FF",
        "cli.command",
        1,
        None,
        cli,
    )
    .unwrap();
    let all = read_all(&j);
    let ids = all[0].payload["txIds"].as_array().unwrap();
    assert!(ids[0].as_str().unwrap().starts_with("k1:"), "{ids:?}");
    assert!(!serde_json::to_string(&all[0]).unwrap().contains("idem-1"));
}

#[test]
fn torn_tail_is_truncated_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.record(Source::FinNode, None, commit(2));
    j.close();
    let file = files(&events(&dir)).pop().unwrap();
    let whole = std::fs::read(&file).unwrap();
    let mut torn = whole.clone();
    torn.extend_from_slice(br#"{"v":1,"id":"01JC","ts":17"#);
    std::fs::write(&file, &torn).unwrap();

    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.flush().unwrap();
    assert_eq!(
        std::fs::read(&file).unwrap(),
        whole,
        "cut back to the last newline"
    );
    j.record(Source::FinNode, None, commit(3));
    let mut it = j.read(&Filter::default());
    let micros: Vec<u64> = it
        .by_ref()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [1, 2, 3]);
    assert_eq!(it.skipped(), 0);
}

/// An entry of another envelope version (a later build wrote it) is unknown here: the read,
/// the export (stored and upload view), a redacted page and the file's last `ts` pass it over.
#[test]
fn other_envelope_versions_are_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.record(Source::FinNode, None, commit(2));
    j.close();
    let file = files(&events(&dir)).pop().unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let first: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    let mut later: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    later["v"] = serde_json::json!(2);
    later["ts"] = serde_json::json!(T0 + 30 * DAY.as_millis() as u64);
    lines[1] = later.to_string();
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();

    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let mut it = j.read(&Filter::default());
    let got: Vec<u64> = it
        .by_ref()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(got, [1]);
    assert_eq!((it.skipped(), it.unknown()), (0, 1));
    assert_eq!(j.export(&Filter::default(), None).count(), 1);
    let upload = j.export(&Filter::default(), Some(Purposes::DIAGNOSTICS));
    assert_eq!(upload.count(), 1);
    assert_eq!(
        j.status_now().newest,
        first["ts"].as_u64(),
        "the later build's ts is not this file's last"
    );
    let page = j.export_page(&Filter::default(), None, None, 100).unwrap();
    let mut text = serde_json::to_value(&page).unwrap();
    text["lines"][0]["v"] = serde_json::json!(2);
    let mut page: fin_journal::ExportPage = serde_json::from_value(text).unwrap();
    page.redact();
    assert!(page.lines.is_empty(), "{page:?}");
}

#[test]
fn reader_skips_bad_lines_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.set_project_policy(PROJECT, Some(CollectPolicy::Default));
    j.record(Source::FinNode, Some(ctx()), commit(1));
    j.record(Source::FinNode, None, commit(2));
    j.close();
    let file = files(&events(&dir)).pop().unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let unknown = lines[1].replace("\"kind\":\"log.commit\"", "\"kind\":\"log.future\"");
    lines.insert(1, "not json".into());
    lines.insert(2, unknown);
    lines.push("{\"torn".into());
    std::fs::write(&file, lines.join("\n")).unwrap();

    // Off: read only, nothing repaired.
    let j = Journal::open(&events(&dir), Consent::OFF, opts(&c));
    let mut it = j.read(&Filter::default());
    let got: Vec<u64> = it
        .by_ref()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(got, [1, 2]);
    assert_eq!((it.skipped(), it.unknown()), (2, 1));
    // Filters: by project, by kind, by time, and the newest n.
    let f = Filter {
        project: Some(PROJECT.into()),
        ..Filter::default()
    };
    assert_eq!(j.read(&f).count(), 1);
    let f = Filter {
        kinds: vec!["session.end".into()],
        ..Filter::default()
    };
    assert_eq!(j.read(&f).count(), 0);
    let f = Filter {
        limit: Some(1),
        ..Filter::default()
    };
    let newest: Vec<_> = j.read(&f).map(|e| e.payload["micros"].clone()).collect();
    assert_eq!(newest, [serde_json::json!(2)]);
}

#[test]
fn unwritable_dir_suspends_and_retries() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    // A file where the directory should be: on every OS.
    std::fs::write(events(&dir), b"in the way").unwrap();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    let t = Instant::now();
    for i in 0..100 {
        j.record(Source::FinNode, None, commit(i));
    }
    assert!(t.elapsed() < Duration::from_millis(100));
    assert_eq!(j.flush().unwrap_err().code, "JOURNAL_UNAVAILABLE");
    let s = j.status();
    assert_eq!(s.state, JournalState::Suspended);
    assert_eq!(s.code.as_deref(), Some("JOURNAL_UNWRITABLE"));
    assert!(s.dropped >= 100);
    std::fs::remove_file(events(&dir)).unwrap();
    // Still suspended until the retry is due.
    j.record(Source::FinNode, None, commit(500));
    assert!(j.flush().is_err());
    c.advance(Duration::from_secs(61));
    j.record(Source::FinNode, None, commit(501));
    j.flush().unwrap();
    assert_eq!(j.status().state, JournalState::On);
    let all = read_all(&j);
    let micros: Vec<_> = of_kind(&all, "log.commit")
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [501]);
    assert_eq!(
        reported_drops(&all),
        101,
        "the drops while suspended are reported"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o500)).unwrap();
        if std::fs::write(ro.join("probe"), b"").is_ok() {
            return; // running as root: permissions do not bind
        }
        let j = Journal::open(&ro, diagnostics(), opts(&c));
        j.record(Source::FinNode, None, commit(1));
        assert!(j.flush().is_err());
        assert_eq!(j.status().code.as_deref(), Some("JOURNAL_UNWRITABLE"));
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o700)).unwrap();
        c.advance(Duration::from_secs(61));
        j.record(Source::FinNode, None, commit(2));
        j.flush().unwrap();
        assert_eq!(of_kind(&read_all(&j), "log.commit").len(), 1);
    }
}

#[test]
fn disk_full_suspends_and_never_fails_the_caller() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    j.faults().unwrap().fail_writes(true);
    j.record(Source::FinNode, None, commit(2));
    assert!(j.flush().is_err());
    assert_eq!(j.status().code.as_deref(), Some("JOURNAL_DISK_FULL"));
    j.faults().unwrap().fail_writes(false);
    c.advance(Duration::from_secs(61));
    j.record(Source::FinNode, None, commit(3));
    let all = read_all(&j);
    let micros: Vec<_> = of_kind(&all, "log.commit")
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [1, 3]);
}

/// The panic tests share the process-wide panic hook.
static HOOK: Mutex<()> = Mutex::new(());

#[test]
fn writer_panic_suspends_not_crashes() {
    let _g = HOOK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    j.faults().unwrap().panic_next();
    j.record(Source::FinNode, None, commit(2));
    let s = j.status();
    assert_eq!(
        (s.state, s.code.as_deref()),
        (JournalState::Suspended, Some("JOURNAL_PANIC"))
    );
    j.record(Source::FinNode, None, commit(3)); // the caller is unaffected
    assert!(j.flush().is_err(), "still suspended");
    c.advance(Duration::from_secs(61));
    j.record(Source::FinNode, None, commit(4));
    j.flush().unwrap();
    assert_eq!(j.status().state, JournalState::On);
    let micros: Vec<_> = of_kind(&read_all(&j), "log.commit")
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [1, 4]);
}

#[test]
fn panic_hook_on_writer_thread_does_not_wait() {
    let _g = HOOK.lock().unwrap_or_else(|e| e.into_inner());
    static ON_WRITER: AtomicBool = AtomicBool::new(false);
    static HOOK_MICROS: AtomicU64 = AtomicU64::new(u64::MAX);
    static TARGET: Mutex<Option<Journal>> = Mutex::new(None);
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    *TARGET.lock().unwrap() = Some(j.clone());
    let prev = Arc::new(std::panic::take_hook());
    let chained = prev.clone();
    std::panic::set_hook(Box::new(move |info| {
        let t = Instant::now();
        if Journal::on_writer_thread() {
            ON_WRITER.store(true, Ordering::SeqCst);
        } else if let Some(j) = TARGET.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            j.record_sync(
                Source::FinNode,
                None,
                session_end(),
                Duration::from_millis(250),
            );
        }
        HOOK_MICROS.store(t.elapsed().as_micros() as u64, Ordering::SeqCst);
        chained(info);
    }));
    j.faults().unwrap().panic_next();
    j.record(Source::FinNode, None, commit(1));
    let suspended = j.status().state == JournalState::Suspended;
    let _ = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| prev(info)));
    *TARGET.lock().unwrap() = None;
    assert!(suspended);
    assert!(
        ON_WRITER.load(Ordering::SeqCst),
        "the hook ran on the writer thread"
    );
    assert!(
        HOOK_MICROS.load(Ordering::SeqCst) < 50_000,
        "and returned at once"
    );
    assert!(!Journal::on_writer_thread());
}

#[test]
fn record_sync_is_on_disk_when_it_returns() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    let p: AnyPayload = SessionEnd { uptime_s: 99 }.into();
    assert!(j.record_sync(Source::Finisherd, None, p, Duration::from_secs(1)));
    // Read the file directly, without the journal's flush.
    let on_disk = raw_lines(&events(&dir));
    let kinds: Vec<_> = on_disk.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["log.commit", "session.end"]);
    assert_eq!(on_disk[1].payload["uptimeS"], 99);
}

#[test]
fn record_sync_times_out_with_a_stalled_writer() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    j.faults().unwrap().stall_writes(Duration::from_secs(2));
    // The writer is stuck in a 2 s write before it reaches the synchronous record.
    j.record(Source::FinNode, None, commit(2));
    let stuck = {
        let j = j.clone();
        std::thread::spawn(move || j.flush())
    };
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    let ok = j.record_sync(
        Source::FinNode,
        None,
        session_end(),
        Duration::from_millis(250),
    );
    let waited = t.elapsed();
    j.faults().unwrap().stall_writes(Duration::ZERO);
    assert!(!ok);
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_millis(1000),
        "{waited:?}"
    );
    stuck.join().unwrap().unwrap();
    let all = read_all(&j);
    assert!(
        of_kind(&all, "session.end").is_empty(),
        "a cancelled record is never written"
    );
    assert_eq!(reported_drops(&all), 1, "and is counted as dropped");
}

#[cfg(unix)]
#[test]
fn files_are_0600_dir_0700() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&events(&dir)), 0o700);
    let files = files(&events(&dir));
    assert_eq!(files.len(), 1);
    assert_eq!(mode(&files[0]), 0o600);
}

#[test]
fn no_network_crates_in_tree() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let out = std::process::Command::new(cargo)
        .args([
            "tree",
            "--offline",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--manifest-path",
            manifest,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8(out.stdout).unwrap();
    let crates: Vec<&str> = tree
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(crates.contains(&"fin-journal") && crates.contains(&"fin-core"));
    for banned in [
        "ureq",
        "rustls",
        "fin-fonts",
        "erased-serde",
        "fin-store",
        "fin-node",
    ] {
        assert!(
            !crates.contains(&banned),
            "{banned} is in fin-journal's tree:\n{tree}"
        );
    }
    let ours: Vec<_> = crates.iter().filter(|c| c.starts_with("fin-")).collect();
    assert!(
        ours.iter().all(|c| ["fin-journal", "fin-core"].contains(c)),
        "{ours:?}"
    );
}
