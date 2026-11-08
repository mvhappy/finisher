//! How a probe opens its file: never blocking, only a regular file, and for `probe_beneath`
//! only beneath the root without a symlink.
use std::path::PathBuf;

use fin_probe::fixtures::write_png;
use fin_probe::{probe, probe_beneath};

/// A canonical temporary root with `clip.png` inside and `secret.png` beside it.
fn setup() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(t.path()).unwrap();
    let root = base.join("root");
    std::fs::create_dir_all(root.join("workspace")).unwrap();
    write_png(&root.join("workspace").join("clip.png"), 64, 36);
    write_png(&base.join("secret.png"), 32, 18);
    (t, root)
}

#[test]
fn a_folder_is_not_a_file() {
    let (_t, root) = setup();
    let err = probe(&root.join("workspace")).unwrap_err();
    assert_eq!(
        (err.code.as_str(), &err.details["reason"]),
        ("ASSET_UNREADABLE", &"NotAFile".into())
    );
    let err = probe_beneath(&root, &root.join("workspace")).unwrap_err();
    assert_eq!(err.details["reason"], "NotAFile");
}

#[test]
fn a_regular_file_beneath_the_root_probes_as_usual() {
    let (_t, root) = setup();
    let clip = root.join("workspace").join("clip.png");
    let got = probe_beneath(&root, &clip).unwrap();
    assert_eq!(got, probe(&clip).unwrap());
    assert_eq!(got.image.unwrap().width, 64);
    let err = probe_beneath(&root, &root.join("workspace").join("missing.png")).unwrap_err();
    assert_eq!(err.code, "ASSET_NOT_FOUND");
}

#[test]
fn a_path_outside_the_root_is_not_allowed() {
    let (_t, root) = setup();
    let secret = root.parent().unwrap().join("secret.png");
    for p in [secret.clone(), root.join("..").join("secret.png")] {
        let err = probe_beneath(&root, &p).unwrap_err();
        assert_eq!(err.code, "PATH_NOT_ALLOWED", "{p:?}");
        assert_eq!(err.details["reason"], "outsideRoot");
        assert!(err.details.get("fingerprint").is_none());
    }
    // Unrestricted, it is just a file.
    assert!(probe(&secret).is_ok());
}

/// FIFOs and symlinks (Windows: neither in a plain folder without privileges).
#[cfg(unix)]
mod unix {
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    use fin_probe::{fingerprint, probe, probe_beneath};

    use super::setup;

    /// `f` on another thread, failing the test if it does not finish promptly (a regression would
    /// block in `open` for ever).
    fn promptly<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(10))
            .expect("the probe blocked")
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let (_t, root) = setup();
        let fifo = root.join("workspace").join("pipe.mp4");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (f, r) = (fifo.clone(), root.clone());
        let plain = promptly(move || probe(&f));
        let f = fifo.clone();
        let beneath = promptly(move || probe_beneath(&r, &f));
        for err in [plain.unwrap_err(), beneath.unwrap_err()] {
            assert_eq!(err.code, "ASSET_UNREADABLE");
            assert_eq!(err.details["reason"], "NotAFile");
            assert_eq!(err.details["path"], fifo.to_str().unwrap());
        }
        let f = fifo.clone();
        let hashed = promptly(move || fingerprint(&f));
        assert_eq!(hashed.unwrap_err().kind(), std::io::ErrorKind::InvalidInput);
    }

    /// A symlink inside the root, dangling (what the server's path check cannot see through) or
    /// pointing outside, is refused at the open itself; the plain probe still follows one.
    #[test]
    fn a_symlink_beneath_the_root_is_refused() {
        use std::os::unix::fs::symlink;
        let (_t, root) = setup();
        let secret = root.parent().unwrap().join("secret.png");
        let link = root.join("workspace").join("link.png");
        symlink("/nonexistent/secret.png", &link).unwrap();
        let refused = |p: &Path| {
            let err = probe_beneath(&root, p).unwrap_err();
            assert_eq!(err.code, "PATH_NOT_ALLOWED", "{p:?}: {err:?}");
            assert_eq!(err.details["reason"], "symlink");
            // The path as given: the link's target is never reported.
            assert_eq!(err.details["path"], p.to_str().unwrap());
        };
        refused(&link);
        std::fs::remove_file(&link).unwrap();
        symlink(&secret, &link).unwrap();
        refused(&link);
        // Inside the root is refused too: the server passes canonical paths, so a symlink here was
        // swapped in after its check.
        std::fs::remove_file(&link).unwrap();
        symlink(root.join("workspace").join("clip.png"), &link).unwrap();
        refused(&link);
        // A symlinked folder above the file.
        symlink(root.parent().unwrap(), root.join("up")).unwrap();
        refused(&root.join("up").join("secret.png"));
        assert_eq!(probe(&link).unwrap().image.unwrap().width, 64);
    }
}
