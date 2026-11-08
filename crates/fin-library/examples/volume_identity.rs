//! Prints what the volume identity of each path is computed from (plan 5D Task 2 Step 0;
//! Task 17's cross-platform runs): the OS fields, the key, the root's file id and the sample.
//! The output holds mount sources and paths: keep it local.
//!
//! `cargo run -p fin-library --example volume_identity -- <dir>...`

use std::path::Path;

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: volume_identity <dir>...");
        std::process::exit(2);
    }
    for p in paths {
        let path = Path::new(&p);
        println!("== {p}");
        #[cfg(target_os = "linux")]
        match fin_library::volume::linux::fields_of(path) {
            Ok(f) => println!(
                "fstype={} source={} mnt_id={} mount_root_dir={} fsid={} dev={} ino={}",
                f.mount.fstype,
                f.mount.source,
                f.mnt_id,
                String::from_utf8_lossy(&f.mount.root),
                f.fsid
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                f.dev,
                f.ino
            ),
            Err(e) => println!("fields: {e}"),
        }
        #[cfg(target_os = "macos")]
        match fin_library::volume::macos::fields_of(path) {
            Ok(f) => println!(
                "fstype={} mntfrom={} mnton={} local={} uuid={} fsid={} case_sensitive={}",
                f.fstype,
                f.mntfrom,
                String::from_utf8_lossy(&f.mnton),
                f.local,
                f.uuid
                    .map(|u| u.iter().map(|b| format!("{b:02x}")).collect::<String>())
                    .unwrap_or_else(|| "-".into()),
                f.fsid
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>(),
                f.case_sensitive
            ),
            Err(e) => println!("fields: {e}"),
        }
        #[cfg(windows)]
        match fin_library::volume::windows::fields_of(path) {
            Ok(f) => println!(
                "fs={} serial={:016x} share={:?} remote={} volume_root={} case_sensitive={} file_id={}",
                f.fs_name,
                f.serial,
                f.share,
                f.remote,
                f.volume_root,
                f.case_sensitive,
                f.file_id
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
            Err(e) => println!("fields: {e}"),
        }
        match fin_library::volume::root_stat(path) {
            Ok(s) => println!(
                "key={} kind={} dev_id={} parent_dev_id={:?} mount_root={} case_sensitive={} remote={} root_file_id={:?} entries={} sample={:?}",
                s.key,
                s.kind,
                s.dev_id,
                s.parent_dev_id,
                s.mount_root(),
                s.case_sensitive,
                s.remote,
                s.root_file_id,
                s.entries,
                s.sample
                    .iter()
                    .map(|e| (e.file_id.clone(), e.size, e.mtime_ns))
                    .collect::<Vec<_>>()
            ),
            Err(e) => println!("root_stat: {e}"),
        }
    }
}
