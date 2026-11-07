//! The walker's descriptor budget (plan 5D Task 4): open directories kept for their
//! children, running reads and reads abandoned past their deadline (which still hold their
//! descriptors) together stay below macOS's default 256.

mod walk_util;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use fin_library::source::fake::FakeSource;
use fin_library::{DirHandle, RawEntry, RawRel, RootStat, Source, VolumeIdentity};
use serde_json::json;
use walk_util::*;

/// Counts descriptors as a native source holds them: one per open handle, one more while a
/// listing reads (its fresh offset).
struct Fds {
    inner: FakeSource,
    open: Arc<AtomicUsize>,
    peak: AtomicUsize,
}

struct Guard(Arc<AtomicUsize>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct Wrapped {
    inner: DirHandle,
    _fd: Guard,
}

impl Fds {
    fn take(&self) -> Guard {
        let n = self.open.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(n, Ordering::SeqCst);
        Guard(Arc::clone(&self.open))
    }

    fn inner<'a>(&self, dir: &'a DirHandle) -> &'a DirHandle {
        &dir.os::<Wrapped>().expect("a wrapped handle").inner
    }
}

impl Source for Fds {
    fn stat_root(&self) -> std::io::Result<RootStat> {
        self.inner.stat_root()
    }
    fn open_dir(&self, rel: &RawRel) -> std::io::Result<DirHandle> {
        let inner = self.inner.open_dir(rel)?;
        let fd = self.take();
        Ok(DirHandle::new(rel.clone(), Wrapped { inner, _fd: fd }))
    }
    fn read_dir(&self, dir: &DirHandle) -> std::io::Result<Vec<RawEntry>> {
        let _fresh = self.take();
        self.inner.read_dir(self.inner(dir))
    }
    fn stat_child(&self, parent: &DirHandle, name: &[u8]) -> std::io::Result<Option<RawEntry>> {
        self.inner.stat_child(self.inner(parent), name)
    }
    fn dir_mtime(&self, dir: &DirHandle) -> std::io::Result<Option<i64>> {
        self.inner.dir_mtime(self.inner(dir))
    }
    fn volume(&self, dir: &DirHandle) -> std::io::Result<VolumeIdentity> {
        self.inner.volume(self.inner(dir))
    }
}

#[test]
fn stuck_reads_count_against_the_descriptor_budget() {
    // 200 directories with a subdirectory each (so their handles are kept); the first 60
    // hang past the deadline, the rest are slow enough to overlap.
    let dirs: Vec<serde_json::Value> = (0..200)
        .map(|i| {
            let mut s = dir("s", 1, vec![file("f", 1)]);
            s["readDelayMs"] = json!(30);
            let mut d = dir(&format!("d{i:03}"), i, vec![s, file("f", 1)]);
            d["readDelayMs"] = json!(if i < 60 { 1_500 } else { 30 });
            d
        })
        .collect();
    let fds = Arc::new(Fds {
        inner: FakeSource::from_json(&root(dirs).to_string()).unwrap(),
        open: Arc::new(AtomicUsize::new(0)),
        peak: AtomicUsize::new(0),
    });
    let mut o = opts(64);
    o.timing = timing(5_000, 150);
    let (r, _) = run(Arc::clone(&fds) as Arc<dyn Source>, full(1, None), &o);
    let s = r.unwrap();
    assert_eq!(s.timeouts, 60, "{s:?}");
    let peak = fds.peak.load(Ordering::SeqCst);
    // 64 kept + 64 tasks × 2, the root's handle being one of the tasks'.
    assert!(peak <= 192, "peak {peak} descriptors");
}
