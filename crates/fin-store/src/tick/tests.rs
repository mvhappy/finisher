use super::*;
use crate::conflict_names::Provider;
use crate::storage_kind::{slow_dir_cache, slow_dir_cache_on};

const KINDS: [StorageKind; 3] = [
    StorageKind::Local,
    StorageKind::Network,
    StorageKind::Sync(Provider::ICloud),
];

#[test]
fn only_a_macos_network_share_has_the_slow_directory_cache() {
    for kind in KINDS {
        assert_eq!(
            slow_dir_cache_on(kind, true),
            kind == StorageKind::Network,
            "{kind}"
        );
        assert!(!slow_dir_cache_on(kind, false), "{kind}");
        assert_eq!(
            slow_dir_cache(kind),
            slow_dir_cache_on(kind, cfg!(target_os = "macos"))
        );
    }
}

#[test]
fn the_ordering_rule_waits_65s_on_a_macos_share_else_10s() {
    assert_eq!(MACOS_DIR_CACHE_WINDOW, Duration::from_secs(65));
    assert_eq!(DIR_CACHE_WINDOW, Duration::from_secs(10));
    // smbfs `dir_cache_max` defaults to 60 s: the wait must outlast it.
    assert!(MACOS_DIR_CACHE_WINDOW > Duration::from_secs(60));
    for kind in KINDS {
        let mac = if kind == StorageKind::Network {
            MACOS_DIR_CACHE_WINDOW
        } else {
            DIR_CACHE_WINDOW
        };
        assert_eq!(dir_cache_window_on(kind, true), mac, "{kind}");
        assert_eq!(dir_cache_window_on(kind, false), DIR_CACHE_WINDOW, "{kind}");
        assert_eq!(
            dir_cache_window(kind),
            dir_cache_window_on(kind, cfg!(target_os = "macos"))
        );
    }
}

#[test]
fn poll_cadence_is_2s_local_1s_network_10s_sync() {
    assert_eq!(poll_every(StorageKind::Local), Duration::from_secs(2));
    assert_eq!(poll_every(StorageKind::Network), Duration::from_secs(1));
    assert_eq!(
        poll_every(StorageKind::Sync(Provider::Dropbox)),
        Duration::from_secs(10)
    );
}
