//! Private keys are owner-only, and a key someone else can reach is refused (`KEY_UNPROTECTED`,
//! with the command that fixes it), as `ssh` refuses an unprotected private key: Unix modes,
//! Windows DACLs (`src/private.rs`).

mod support;

use std::path::Path;

use fin_registry::editor::{RepoBuilder, Role, SigningKey};

fn pems(d: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "pem"))
        .collect()
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt;

    use super::support::tuf::Reg;
    use super::*;

    fn chmod(p: &Path, mode: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn sh(line: &str) {
        let ok = std::process::Command::new("sh")
            .args(["-c", line])
            .status()
            .unwrap();
        assert!(ok.success(), "{line}");
    }

    #[test]
    fn an_open_key_folder_is_refused_until_fixed() {
        let r = Reg::new(None);
        chmod(&r.keys, 0o755);
        let e = RepoBuilder::open(&r.repo, &r.keys).unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        assert_eq!(e.details["path"], r.keys.display().to_string());
        assert_eq!(e.details["problems"], serde_json::json!(["mode 0755"]));
        let fix = e.fix.unwrap();
        assert_eq!(fix, format!("chmod 700 '{}'", r.keys.display()));
        sh(&fix);
        RepoBuilder::open(&r.repo, &r.keys).unwrap();
    }

    #[test]
    fn a_readable_key_file_is_refused_when_signing() {
        let mut r = Reg::new(None);
        for p in pems(&r.keys) {
            chmod(&p, 0o644);
        }
        let e =
            r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
                .unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        assert_eq!(e.details["problems"], serde_json::json!(["mode 0644"]));
        assert!(e.fix.as_deref().unwrap().starts_with("chmod 600 '"));
        assert!(e.message.contains("rotate"), "{}", e.message);
        for p in pems(&r.keys) {
            chmod(&p, 0o600);
        }
        r.b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
            .unwrap();
    }

    #[test]
    fn a_loose_root_key_file_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("root.pem");
        let k = SigningKey::generate();
        k.write(&p).unwrap();
        assert_eq!(SigningKey::read(&p).unwrap().public(), k.public());
        chmod(&p, 0o640);
        assert_eq!(SigningKey::read(&p).unwrap_err().code, "KEY_UNPROTECTED");
        // A missing file is still an I/O error on the read.
        let e = SigningKey::read(&tmp.path().join("none.pem")).unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["op"]),
            ("IO_ERROR", &"read".into())
        );
    }

    /// `init` into an existing key folder others can reach writes nothing.
    #[test]
    fn init_refuses_an_existing_open_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let keys = tmp.path().join("keys");
        std::fs::create_dir(&keys).unwrap();
        chmod(&keys, 0o775);
        let repo = tmp.path().join("registry");
        let e = RepoBuilder::init(&repo, &keys, &[SigningKey::generate()], 1, None).unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        assert!(pems(&keys).is_empty());
        assert!(!repo.join("metadata/1.root.json").exists());
    }
}

/// Needs a Windows run: the DACLs, `icacls` and PowerShell's `Get-Acl`.
#[cfg(windows)]
mod windows {
    use super::*;

    fn cmd(line: &str) {
        use std::os::windows::process::CommandExt;
        let out = std::process::Command::new("cmd")
            .arg("/C")
            .raw_arg(line)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{line}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// `(protected, number of access rules)` of `path`, as PowerShell reads its DACL.
    fn acl(path: &Path) -> (bool, usize) {
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                "$a = Get-Acl -LiteralPath '{}'; \"$($a.AreAccessRulesProtected) $($a.Access.Count)\"",
                path.display()
            ))
            .output()
            .unwrap();
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (p, n) = s.split_once(' ').unwrap_or_else(|| panic!("{s}"));
        (p == "True", n.parse().unwrap())
    }

    /// A folder like one at the root of a data drive (`E:\`): Authenticated Users modify and
    /// Users read, inherited by everything below.
    fn permissive(tmp: &Path) -> std::path::PathBuf {
        let d = tmp.join("drive-root");
        std::fs::create_dir(&d).unwrap();
        cmd(&format!(
            "icacls \"{}\" /grant *S-1-5-11:(OI)(CI)M *S-1-5-32-545:(OI)(CI)R",
            d.display()
        ));
        d
    }

    #[test]
    fn a_registry_under_a_drive_root_gets_an_owner_only_key_ring() {
        let tmp = tempfile::tempdir().unwrap();
        let open = permissive(tmp.path());
        let keys = open.join("secrets/keys");
        let root = open.join("root.pem");
        let k = SigningKey::generate();
        k.write(&root).unwrap();
        assert_eq!(acl(&root), (true, 2));
        let mut b = RepoBuilder::init(&open.join("registry"), &keys, &[k], 1, None).unwrap();
        for d in [open.join("secrets"), keys.clone()] {
            assert_eq!(acl(&d), (true, 2), "{}", d.display());
        }
        for p in pems(&keys) {
            assert_eq!(acl(&p), (true, 2), "{}", p.display());
        }
        b.rotate(Role::Timestamp, &[SigningKey::generate()], 1)
            .unwrap();
        RepoBuilder::open(&open.join("registry"), &keys).unwrap();
        SigningKey::read(&root).unwrap();
    }

    #[test]
    fn a_key_ring_others_can_reach_is_refused_until_fixed() {
        let tmp = tempfile::tempdir().unwrap();
        let open = permissive(tmp.path());
        let repo = open.join("registry");
        let keys = open.join("keys");
        RepoBuilder::init(&repo, &keys, &[SigningKey::generate()], 1, None).unwrap();
        cmd(&format!(
            "icacls \"{}\" /grant *S-1-5-32-545:(OI)(CI)R",
            keys.display()
        ));
        let e = RepoBuilder::open(&repo, &keys).unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        assert_eq!(
            e.details["problems"],
            serde_json::json!(["allows S-1-5-32-545"])
        );
        let fix = e.fix.unwrap();
        assert!(fix.starts_with("in cmd: icacls"), "{fix}");
        cmd(fix.strip_prefix("in cmd: ").unwrap());
        RepoBuilder::open(&repo, &keys).unwrap();

        // A key file written by another tool into the open folder is refused at signing.
        let loose = open.join("loose.pem");
        std::fs::write(&loose, SigningKey::generate().to_pem()).unwrap();
        let e = SigningKey::read(&loose).unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        let problems = e.details["problems"].to_string();
        assert!(
            problems.contains("S-1-5-11") && problems.contains("S-1-5-32-545"),
            "{problems}"
        );
    }

    /// An existing folder that inherits a drive root's ACL is refused before `init` writes.
    #[test]
    fn init_refuses_an_existing_open_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let open = permissive(tmp.path());
        let keys = open.join("keys");
        std::fs::create_dir(&keys).unwrap();
        let repo = open.join("registry");
        let e = RepoBuilder::init(&repo, &keys, &[SigningKey::generate()], 1, None).unwrap_err();
        assert_eq!(e.code, "KEY_UNPROTECTED");
        assert!(pems(&keys).is_empty());
        assert!(!repo.join("metadata/1.root.json").exists());
    }
}
