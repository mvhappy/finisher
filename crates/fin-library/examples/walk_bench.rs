//! Times the walker over a real tree (plan 5D Task 4; the end-to-end budget is Task 15's:
//! a 100k-file local full scan ≤ 20 s warm, an unchanged poll of 5k directories ≤ 1.5 s).
//!
//! `cargo run --release -p fin-library --example walk_bench -- <dir> [--create] [--runs N]`
//! creates (with `--create`) 100,000 files in 5,000 directories under `<dir>` (50 top-level
//! directories of 100 subdirectories of 20 files; names in several scripts), then prints the
//! full walk and the unchanged poll, each `N` times (default 3). The first full walk after
//! `--create` or after the caches were dropped is the cold one (Linux `echo 3 >
//! /proc/sys/vm/drop_caches`, as root; macOS `purge`).

#[cfg(any(unix, windows))]
fn main() {
    bench::main();
}

#[cfg(not(any(unix, windows)))]
fn main() {
    eprintln!("walk_bench runs on Linux, macOS and Windows");
}

#[cfg(any(unix, windows))]
mod bench {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Instant;

    use fin_library::source::NativeSource;
    use fin_library::{
        CancelToken, Excludes, KnownDir, KnownDirs, Source, WalkMode, WalkOpts, walk,
    };

    const NAMES: [&str; 5] = ["clip", "Interview", "café", "日本語", "Ünïcödé"];

    fn create(root: &Path) {
        for t in 0..50 {
            for s in 0..100 {
                let d = root
                    .join(format!("top{t:02}"))
                    .join(format!("{} {s:03}", NAMES[s % 5]));
                std::fs::create_dir_all(&d).unwrap();
                for f in 0..20 {
                    std::fs::write(d.join(format!("{}_{f:02}.mov", NAMES[f % 5])), b"x").unwrap();
                }
            }
        }
    }

    fn run(root: &Path, mode: WalkMode, threads: usize) -> (f64, u64, u64, Vec<KnownDir>) {
        let src: Arc<dyn Source> = Arc::new(NativeSource::new(root));
        let mut opts = WalkOpts::new(Excludes::defaults(true));
        opts.threads = threads;
        let mut known = Vec::new();
        let t = Instant::now();
        let s = walk(
            src,
            mode,
            &opts,
            &mut |b| {
                for d in b.dirs_done.into_iter().filter(|d| d.complete) {
                    known.push(KnownDir {
                        dir: d.dir,
                        mtime_ns: d.mtime_ns,
                        entries: d.entries,
                        listed_gen: Some(1),
                        mount_root: false,
                        volume: d.volume,
                    });
                }
                Ok(())
            },
            &CancelToken::new(),
        )
        .expect("walk");
        (t.elapsed().as_secs_f64(), s.entries, s.listed, known)
    }

    pub fn main() {
        let mut args = std::env::args().skip(1);
        let root = PathBuf::from(
            args.next()
                .expect("usage: walk_bench <dir> [--create] [--runs N]"),
        );
        let rest: Vec<String> = args.collect();
        let runs = rest
            .iter()
            .position(|a| a == "--runs")
            .and_then(|i| rest.get(i + 1))
            .map_or(3, |n| n.parse().unwrap());
        if rest.iter().any(|a| a == "--create") {
            let t = Instant::now();
            create(&root);
            println!("created in {:.2} s", t.elapsed().as_secs_f64());
        }
        let mut known = Vec::new();
        for threads in [fin_library::walk::THREADS_LOCAL, 8, 1] {
            for i in 0..runs {
                let (s, entries, listed, k) = run(
                    &root,
                    WalkMode::Full {
                        generation: 1,
                        known: None,
                    },
                    threads,
                );
                println!(
                    "full  threads {threads:2} run {i}: {s:.3} s ({entries} entries, {listed} dirs)"
                );
                known = k;
            }
        }
        let known = KnownDirs::new(known);
        for i in 0..runs {
            let (s, _, listed, _) = run(
                &root,
                WalkMode::Poll {
                    known: known.clone(),
                },
                fin_library::walk::THREADS_LOCAL,
            );
            println!(
                "poll  unchanged run {i}: {s:.3} s ({} known dirs, {listed} re-listed)",
                known.len()
            );
        }
    }
}
