use super::*;

/// `.finisher/segments/main/x.txt` (a foreign file, so the listing has something to show).
fn layout() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join(".finisher").join("segments").join("main");
    fs::create_dir_all(&main).unwrap();
    fs::write(main.join("x.txt"), b"x").unwrap();
    tmp
}

fn temps_left(dir: &Path) -> usize {
    let mut n = 0;
    for e in fs::read_dir(dir).unwrap().flatten() {
        if e.file_type().unwrap().is_dir() {
            n += temps_left(&e.path());
        } else if e.file_name().to_str().is_some_and(crate::names::is_temp) {
            n += 1;
        }
    }
    n
}

#[test]
fn a_refreshing_scan_touches_segments_and_each_branch_and_leaves_nothing() {
    let tmp = layout();
    let plain = Scanner::new().scan(tmp.path()).unwrap();
    let before = refreshes();
    let fresh = Scanner::new().walk(tmp.path(), true).unwrap();
    // `segments/` (branch discovery) and `segments/main`; not snapshots or compaction.
    assert_eq!(refreshes() - before, 2);
    assert_eq!(fresh, plain);
    assert_eq!(temps_left(tmp.path()), 0);
}

#[test]
fn scan_for_refreshes_only_on_a_macos_network_share() {
    let tmp = layout();
    use crate::conflict_names::Provider;
    for kind in [
        StorageKind::Local,
        StorageKind::Network,
        StorageKind::Sync(Provider::ICloud),
    ] {
        let before = refreshes();
        Scanner::new().scan_for(tmp.path(), kind).unwrap();
        let expected = if crate::storage_kind::slow_dir_cache(kind) {
            2
        } else {
            0
        };
        assert_eq!(refreshes() - before, expected, "{kind}");
    }
}

#[test]
fn a_refresh_that_cannot_write_is_harmless() {
    // A missing directory is not created.
    let tmp = tempfile::tempdir().unwrap();
    let gone = tmp.path().join("gone");
    refresh_listing(&gone);
    assert!(!gone.exists());
    // A read-only branch directory: the scan still lists it, unchanged.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let tmp = layout();
        let main = tmp.path().join(".finisher").join("segments").join("main");
        let plain = Scanner::new().scan(tmp.path()).unwrap();
        fs::set_permissions(&main, fs::Permissions::from_mode(0o555)).unwrap();
        let fresh = Scanner::new().walk(tmp.path(), true);
        fs::set_permissions(&main, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(fresh.unwrap(), plain);
        assert_eq!(temps_left(tmp.path()), 0);
    }
}
