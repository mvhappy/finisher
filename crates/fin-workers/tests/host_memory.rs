//! Per-group memory limits and the sampled figures (5C-1 plan, Task 4; Decision 20). With
//! process groups the host samples every limited group and kills it when the sum of its
//! processes passes the limit; delegated cgroups leave it to the kernel (`memory.max`).

#![cfg(unix)]

mod host_support;

use std::time::{Duration, Instant};

use fin_jobhost::*;
use host_support::*;
use serde_json::json;

const MIB: u64 = 1 << 20;

/// Waits for the file `test.oom` writes once its memory is held.
fn wait_file(h: &TestHost, name: &str) {
    let until = Instant::now() + WAIT;
    while !h.dir.path().join(name).exists() {
        assert!(Instant::now() < until, "{name} was not written");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn sampling() -> Timing {
    Timing {
        rss_sample_ms: 200,
        ..timing()
    }
}

#[test]
fn group_sum_over_limit_is_oom() {
    let mut h = host_with(|o| o.timing = sampling());
    let g = limited_group(&h, 64 * MIB);
    let (a, b) = (h.worker(g), h.worker(g));
    h.hello(a);
    h.hello(b);
    // 48 MiB each: either alone is under the limit, the sum is not.
    for (p, ready) in [(a, "a.ready"), (b, "b.ready")] {
        let params = json!({ "mib": 48, "holdMs": 60_000, "readyFile": ready });
        h.start(p, "test.oom", params);
    }
    let t0 = Instant::now();
    let (ea, eb) = (h.exit(a), h.exit(b));
    assert_eq!(ea.reason, ExitReason::Oom, "{ea:?}");
    assert_eq!(eb.reason, ExitReason::Oom, "{eb:?}");
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    if !h.host.caps().cgroup_memory {
        let used = ea.memory_peak.expect("the sampled figure");
        assert!(used > 64 * MIB, "{used}");
    }
}

#[test]
fn under_limit_runs() {
    let mut h = host_with(|o| o.timing = sampling());
    let g = limited_group(&h, 64 * MIB);
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.oom", json!({ "mib": 16, "holdMs": 1_000 }));
    assert_eq!(h.result(p)["mib"], 16);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Exited);
    if !h.host.caps().cgroup_memory {
        let used = exit.memory_peak.expect("sampled");
        assert!((16 * MIB..64 * MIB).contains(&used), "{used}");
    }
}

/// Linux: the figure is `RssAnon + RssShmem`; a 256 MiB file on a disk, mapped read-only and
/// read in full, is reclaimable and not the job's. (The same file on tmpfs, where this repo's
/// test runner keeps `TMPDIR`, counts as shmem: its pages cannot be reclaimed.)
#[cfg(target_os = "linux")]
#[test]
fn sampled_figure_is_anon_plus_shmem() {
    let mut h = host_with(|o| o.timing = sampling());
    if h.host.caps().cgroup_memory {
        return eprintln!("cgroup mode: the kernel's memory.current is the figure");
    }
    let disk = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let file = disk.path().join("big.bin");
    std::fs::write(&file, vec![7u8; (256 * MIB) as usize]).unwrap();
    let g = limited_group(&h, 64 * MIB);
    let p = h.worker(g);
    h.hello(p);
    let params = json!({ "mib": 8, "holdMs": 60_000, "mapFile": file, "readyFile": "r" });
    h.start(p, "test.oom", params);
    wait_file(&h, "r");
    std::thread::sleep(Duration::from_millis(1_000));
    h.kill(g, KillReason::Test);
    let exit = h.exit(p);
    assert_eq!(
        exit.reason,
        ExitReason::Killed(KillReason::Test),
        "not an OOM"
    );
    let used = exit.memory_peak.expect("sampled");
    assert!((8 * MIB..64 * MIB).contains(&used), "{used}");
}

/// macOS: the figure is `ri_phys_footprint`, which counts touched anonymous memory.
#[cfg(target_os = "macos")]
#[test]
fn macos_uses_phys_footprint() {
    let mut h = host_with(|o| o.timing = sampling());
    let g = limited_group(&h, 1024 * MIB);
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.oom",
        json!({ "mib": 64, "holdMs": 60_000, "readyFile": "r" }),
    );
    wait_file(&h, "r");
    std::thread::sleep(Duration::from_millis(1_000));
    h.kill(g, KillReason::Test);
    let used = h.exit(p).memory_peak.expect("sampled");
    assert!((64 * MIB..512 * MIB).contains(&used), "{used}");
}

#[test]
fn memory_rule_resolution() {
    let total = 16 * 1024 * MIB;
    let rule = |fixed, frac, cap| MemoryRule {
        fixed_mib: fixed,
        ram_fraction: frac,
        cap_mib: cap,
    };
    assert_eq!(
        rule(Some(512), Some(0.5), Some(1)).resolve(total),
        Some(512 * MIB)
    );
    assert_eq!(
        rule(None, Some(0.25), None).resolve(total),
        Some(4096 * MIB)
    );
    assert_eq!(
        rule(None, Some(0.25), Some(2048)).resolve(total),
        Some(2048 * MIB)
    );
    assert_eq!(
        rule(None, Some(0.1), Some(8192)).resolve(total),
        Some(total / 10)
    );
    assert_eq!(rule(None, None, Some(300)).resolve(total), Some(300 * MIB));
    assert_eq!(rule(None, Some(3.0), None).resolve(total), Some(total));
    assert_eq!(rule(None, Some(f64::NAN), None).resolve(total), None);
    assert_eq!(MemoryRule::default().resolve(total), None);
}
