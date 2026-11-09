//! Events of every import pass (open, takeover, rebuild), and ticks that leave the heavy steps
//! (daily backup, compaction) to the caller (`TickOptions`).

mod integration_support;

use std::path::PathBuf;
use std::time::Duration;

use fin_store::lease::MonoClock;
use fin_store::{DueWork, Store, StoreEvent, TickOptions, TickReport};
use integration_support::*;

const LIGHT: TickOptions = TickOptions {
    backup: false,
    compaction: false,
};

fn light_poll(e: &Env, s: &mut Store) -> TickReport {
    s.poll_now_for_test();
    s.tick_with(e.clock.now(), LIGHT).unwrap()
}

fn applied(events: &[StoreEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Applied(c) => Some(c.tx.tx_id.clone()),
            _ => None,
        })
        .collect()
}

fn backups(s: &Store) -> Vec<PathBuf> {
    let dir = s.db_path().parent().unwrap().join("backups");
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    v.retain(|p| p.to_string_lossy().contains("daily-"));
    v
}

#[test]
fn records_imported_at_open_are_announced_by_the_first_tick() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    // B has no database: open builds one (a rebuild), announced as such.
    let mut b = e.open("b");
    let first = e.poll(&mut b).events;
    assert!(matches!(first[0], StoreEvent::Rebuilt { .. }), "{first:?}");
    drop(b);
    commit(&mut a, "t1", vec![insert(0, "a1")]);
    commit(&mut a, "t2", vec![insert(1, "a1")]);
    e.flush(&mut a);
    // Reopened, B imports inside open.
    let mut b = e.open("b");
    assert_eq!(b.head(fin_store::MAIN), 3, "open imports");
    assert_eq!(applied(&e.poll(&mut b).events), ["t1", "t2"]);
    assert!(applied(&e.poll(&mut b).events).is_empty(), "once");
}

#[test]
fn records_imported_by_a_takeover_are_announced() {
    let e = env();
    drop(e.create("a"));
    edit_project_file(&e.dir, |f| {
        f.insert("storageOverride".into(), "sync:dropbox".into());
    });
    let mut a = e.open("a");
    let mut b = e.open("b");
    e.poll(&mut b);
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    drop(a);
    b.takeover(true).unwrap();
    assert_eq!(b.head(fin_store::MAIN), 1, "the takeover imports");
    assert_eq!(applied(&e.poll(&mut b).events), ["t1"]);
}

#[test]
fn a_rebuild_is_announced_before_what_it_imports() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    e.poll(&mut a);
    let before = a.incarnation();
    a.rebuild().unwrap();
    let events = e.poll(&mut a).events;
    let rebuilt: Vec<&String> = events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Rebuilt { incarnation } => Some(incarnation),
            _ => None,
        })
        .collect();
    assert_eq!(rebuilt, [&a.incarnation()]);
    assert_ne!(a.incarnation(), before);
    assert!(
        matches!(events[0], StoreEvent::Rebuilt { .. }),
        "{events:?}"
    );
}

#[test]
fn a_light_tick_leaves_the_daily_backup_to_the_caller() {
    let e = env();
    let mut a = e.create("a");
    light_poll(&e, &mut a);
    assert!(backups(&a).is_empty());
    assert_eq!(
        a.due_work(),
        DueWork {
            backup: true,
            compaction: false
        }
    );
    a.run_due_backup();
    assert_eq!(backups(&a).len(), 1);
    assert_eq!(a.due_work(), DueWork::default());
    light_poll(&e, &mut a);
    assert_eq!(a.due_work(), DueWork::default(), "taken today");
}

#[test]
fn a_light_tick_leaves_compaction_to_the_caller() {
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(1, true);
    a.set_compaction_grace_for_test(Duration::from_secs(5));
    for i in 0..3 {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        assert_eq!(e.flush(&mut a), 1);
    }
    // Phase 1 now; phase 2 once the grace is over.
    assert!(!a.compact(true).unwrap().manifests.is_empty());
    light_poll(&e, &mut a);
    assert!(!a.due_work().compaction, "in its grace period");
    e.clock.advance(Duration::from_secs(6));
    let r = light_poll(&e, &mut a);
    assert_eq!(r.compact.deleted, 0, "the light tick deletes nothing");
    assert!(a.due_work().compaction);
    let done = a.run_due_compaction().unwrap();
    assert_eq!(done.deleted, 3, "{done:?}");
    assert!(!a.due_work().compaction);
    let json = serde_json::to_value(&done).unwrap();
    assert_eq!(json["deleted"], 3);
    assert!(json["packsWritten"].is_number());
}

#[test]
fn a_due_compaction_runs_one_step_at_a_time() {
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(1, true);
    a.set_compaction_grace_for_test(Duration::from_secs(5));
    commit(&mut a, "t0", vec![asset("/m/0.mov")]);
    let feature = a
        .create_branch(fin_store::BranchCreate {
            tx_id: "b1".into(),
            author: "test".into(),
            name: Some("feature".into()),
            from: fin_store::MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap()
        .branch
        .id;
    e.flush(&mut a);
    for (i, branch) in [fin_store::MAIN, &feature]
        .iter()
        .cycle()
        .take(6)
        .enumerate()
    {
        let tx = fin_core::Tx {
            tx_id: format!("t{}", i + 1),
            author: "test".into(),
            branch: (*branch).to_string(),
            base_rev: a.head(branch),
            schema_version: fin_core::SCHEMA_VERSION,
            args_hash: None,
            via: None,
            ops: vec![asset(&format!("/m/{}.mov", i + 1))],
        };
        a.commit(tx).unwrap();
        assert_eq!(e.flush(&mut a), 1);
    }
    // A manifest per branch; both past their grace at once.
    assert_eq!(a.compact(true).unwrap().manifests.len(), 2);
    e.clock.advance(Duration::from_secs(6));
    for left in [2, 1] {
        light_poll(&e, &mut a);
        assert!(a.due_work().compaction, "{left} manifests left");
        let step = a.run_due_compaction_step().unwrap();
        assert_eq!(step.finished.len(), 1, "{step:?}");
    }
    light_poll(&e, &mut a);
    assert!(!a.due_work().compaction, "all done");
}
