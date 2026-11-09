//! The launcher stays the program's parent on macOS too (plan 7C Decision 3, macOS
//! amendment): it forwards signals, passes the program's exit through and, when the session
//! ends, kills every process that carries the session's Seatbelt marker, one that left the
//! program's group and session with `setsid` (re-parented to `launchd`) included. The session
//! ends when the program exits, when the launcher is hung up (`SIGHUP`, what `finisherd` sends
//! at session end) and when its own parent dies. Each case runs confined and unconfined (the
//! mechanism stubbed disabled, F2), where the launcher applies the marker alone. These are
//! test builds, so every sweep also requires a process to hold its session's guard file open
//! (`seatbelt_mark::guard_path`): nothing but the processes these tests start can be reached.
#![cfg(target_os = "macos")]

mod macos_support;

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fin_jobhost::seatbelt_mark;
use macos_support::{LAUNCHER, agent, give_request, launch_cmd, tempdir};

const PATH: (&str, &str) = ("PATH", "/usr/bin:/bin");
const UNCONFINED: (&str, &str) = (fin_sandbox::status::TEST_ENV, "disabled");

/// Starts a grandchild that leaves the program's process group and session (`setsid`; macOS
/// has no `setsid` command) and waits until it has written its pid to `g.pid` in the working
/// directory.
const ESCAPE: &str = "perl -MPOSIX -e 'POSIX::setsid() > 0 or die \"setsid: $!\"; \
                      open(F, \">g.pid.tmp\") or die; print F $$; close F; \
                      rename(\"g.pid.tmp\", \"g.pid\") or die; exec(\"sleep\", \"300\")' \
                      </dev/null >/dev/null 2>&1 & while [ ! -s g.pid ]; do sleep 0.02; done";

/// The environments each case runs in.
fn modes() -> Vec<Vec<(&'static str, &'static str)>> {
    vec![vec![PATH], vec![PATH, UNCONFINED]]
}

/// The request's workspace (writable under the profile).
fn workspace(t: &Path) -> PathBuf {
    t.join("p/workspace/s1")
}

/// Exists and is not a zombie.
fn alive(pid: i32) -> bool {
    // SAFETY: plain data the call fills; the size is the struct's.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
    // SAFETY: a valid buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    n == size && info.pbsi_status != libc::SZOMB
}

/// Waits up to 5 s for `pid` to be gone (or a zombie).
fn gone(pid: i32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        if Instant::now() > until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// The grandchild's pid, and that it really left the program's session.
fn grandchild(t: &Path, program: u32) -> i32 {
    let text = std::fs::read_to_string(workspace(t).join("g.pid")).expect("g.pid");
    let g: i32 = text.trim().parse().expect("a pid");
    // SAFETY: plain calls with integer arguments.
    let (gs, ps) = unsafe { (libc::getsid(g), libc::getsid(program as i32)) };
    assert_eq!(gs, g, "the grandchild leads its own session");
    assert_ne!(gs, ps, "the grandchild left the program's session");
    g
}

/// Kills what a failed assertion would leave running: only a `sleep` or the launcher, so a pid
/// reused by another process of the user is never killed.
struct Reap(Vec<i32>);
impl Drop for Reap {
    fn drop(&mut self) {
        for &p in &self.0 {
            if p > 0 && alive(p) && ["sleep", "fin-sandbox-exec"].contains(&proc_name(p).as_str()) {
                // SAFETY: a process this test started (through the launcher).
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
    }
}

/// Kills `pid` (a process this test started) after 30 s, so a regression fails the test
/// instead of hanging the suite.
fn deadline(pid: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(30));
        // SAFETY: a process this test started; gone (and not reused so soon) otherwise.
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    });
}

/// Starts `script` under the launcher in the workspace, with its stdout piped, and waits for
/// its first line, `ready`.
fn started(t: &Path, script: &str, env: &[(&str, &str)]) -> std::process::Child {
    let req = serde_json::to_vec(&agent(t)).unwrap();
    let (mut cmd, keep) = launch_cmd(&req, &["--policy-fd", "3", "--", "sh", "-c", script], env);
    cmd.current_dir(workspace(t)).stdout(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    drop(keep);
    deadline(child.id());
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line, "ready\n", "{env:?}");
    child
}

#[test]
fn a_setsid_grandchild_goes_when_the_program_exits() {
    for env in modes() {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        // The program says its pid, starts the grandchild and exits 3 at once.
        let script = format!("{ESCAPE}; exit 3");
        let (mut cmd, keep) =
            launch_cmd(&req, &["--policy-fd", "3", "--", "sh", "-c", &script], &env);
        let out = cmd.current_dir(workspace(&t)).output().unwrap();
        drop(keep);
        let text = std::fs::read_to_string(workspace(&t).join("g.pid")).expect("g.pid");
        let g: i32 = text.trim().parse().expect("a pid");
        let _reap = Reap(vec![g]);
        // The program's exit code, as before.
        assert_eq!(out.status.code(), Some(3), "{env:?} {out:?}");
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the program"
        );
    }
}

#[test]
fn hang_up_ends_the_session() {
    for env in modes() {
        let (_d, t) = tempdir();
        let mut child = started(&t, &format!("{ESCAPE}; echo ready; exec sleep 300"), &env);
        let g = grandchild(&t, child.id());
        let _reap = Reap(vec![g]);
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
        let status = child.wait().unwrap();
        // The program was killed, and the launcher's exit says so.
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{env:?}");
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the hang-up"
        );
    }
}

#[test]
fn signals_reach_the_program_and_its_exit_passes_through() {
    for env in modes() {
        let (_d, t) = tempdir();
        let script = "trap 'echo int; exit 5' INT; echo ready; while :; do sleep 0.05; done";
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let (mut cmd, keep) =
            launch_cmd(&req, &["--policy-fd", "3", "--", "sh", "-c", script], &env);
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        drop(keep);
        deadline(child.id());
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n", "{env:?}");
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "int\n", "{env:?}");
        assert_eq!(child.wait().unwrap().code(), Some(5), "{env:?}");
    }
}

#[test]
fn a_signal_that_kills_the_program_kills_the_launcher_alike() {
    for env in modes() {
        let (_d, t) = tempdir();
        let mut child = started(&t, "echo ready; exec sleep 300", &env);
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
        assert_eq!(
            child.wait().unwrap().signal(),
            Some(libc::SIGTERM),
            "{env:?}"
        );
    }
}

#[test]
fn the_parents_death_ends_the_session() {
    for env in modes() {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let program = format!("{ESCAPE}; echo ready; exec sleep 300");
        // A parent that starts the launcher, says its pid and stays until it is killed.
        let mut parent = Command::new("sh");
        parent
            .args([
                "-c",
                "\"$0\" --policy-fd 3 -- sh -c \"$1\" & echo $!; exec sleep 300",
                LAUNCHER,
                &program,
            ])
            .env_clear()
            .envs(env.iter().copied())
            .current_dir(workspace(&t))
            .stdout(Stdio::piped());
        let keep = give_request(&mut parent, &req);
        let mut parent = parent.spawn().unwrap();
        drop(keep);
        deadline(parent.id());
        let mut out = BufReader::new(parent.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let launcher: i32 = line.trim().parse().expect("the launcher's pid");
        deadline(launcher as u32);
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n", "{env:?}");
        let text = std::fs::read_to_string(workspace(&t).join("g.pid")).expect("g.pid");
        let g: i32 = text.trim().parse().expect("a pid");
        let _reap = Reap(vec![g, launcher]);
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert!(gone(launcher), "{env:?}: the launcher outlived its parent");
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the parent"
        );
    }
}

#[test]
fn ending_one_session_leaves_another_alone() {
    for env in modes() {
        let (_d1, t1) = tempdir();
        let (_d2, t2) = tempdir();
        let script = format!("{ESCAPE}; echo ready; exec sleep 300");
        let mut one = started(&t1, &script, &env);
        let mut two = started(&t2, &script, &env);
        let g1 = grandchild(&t1, one.id());
        let g2 = grandchild(&t2, two.id());
        let _reap = Reap(vec![g1, g2]);
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(one.id() as i32, libc::SIGHUP) }, 0);
        one.wait().unwrap();
        assert!(gone(g1), "{env:?}: the first session's grandchild stayed");
        assert!(
            alive(g2),
            "{env:?}: the second session's grandchild was killed"
        );
        assert!(
            alive(two.id() as i32),
            "{env:?}: the second launcher was killed"
        );
        // SAFETY: as above.
        assert_eq!(unsafe { libc::kill(two.id() as i32, libc::SIGHUP) }, 0);
        two.wait().unwrap();
        assert!(gone(g2), "{env:?}: the second session's grandchild stayed");
    }
}

#[test]
fn an_ignored_sigchld_is_not_inherited_by_the_reaper() {
    use std::os::unix::process::CommandExt;
    // `SIG_IGN` survives `exec`: with it the kernel would reap the program unseen.
    for env in modes() {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let (mut cmd, keep) = launch_cmd(
            &req,
            &["--policy-fd", "3", "--", "sh", "-c", "exit 3"],
            &env,
        );
        // SAFETY: only async-signal-safe calls in the child.
        unsafe {
            cmd.pre_exec(|| {
                libc::signal(libc::SIGCHLD, libc::SIG_IGN);
                Ok(())
            });
        }
        let mut child = cmd.spawn().unwrap();
        drop(keep);
        deadline(child.id());
        assert_eq!(child.wait().unwrap().code(), Some(3), "{env:?}");
    }
}

/// Starts `sleep 300` under `sandbox-exec -p profile`, holding `hold` open when given, and
/// waits until it runs `sleep` (its profile then applied).
fn sandboxed_sleep(profile: &str, defines: &[String], hold: Option<&Path>) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut args: Vec<String> = vec!["-p".into(), profile.into()];
    for d in defines {
        args.extend(["-D".into(), d.clone()]);
    }
    args.extend(["/bin/sleep".into(), "300".into()]);
    let mut cmd = Command::new("/usr/bin/sandbox-exec");
    cmd.args(&args).env_clear();
    if let Some(p) = hold {
        let f = std::fs::File::open(p).unwrap();
        let fd = std::os::fd::AsRawFd::as_raw_fd(&f);
        // SAFETY: only async-signal-safe calls in the child; `f` outlives the spawn.
        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(fd, 9) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap();
        drop(f);
        return settled(child);
    }
    settled(cmd.spawn().unwrap())
}

fn settled(child: std::process::Child) -> std::process::Child {
    deadline(child.id());
    let pid = child.id() as i32;
    let until = Instant::now() + Duration::from_secs(5);
    while proc_name(pid) != "sleep" && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(proc_name(pid), "sleep");
    child
}

#[test]
fn a_foreign_sandbox_that_allows_every_name_carries_no_mark() {
    // Some system agents' sandboxes allow every `mach-lookup`, so they "allow" the session's
    // mark too; the mark's second name, which every session refuses, tells them apart. The
    // predicate is checked on this test's own process only: no sweep runs here.
    let mut foreign = sandboxed_sleep("(version 1)(allow default)", &[], None);
    let pid = foreign.id() as i32;
    let _reap = Reap(vec![pid]);
    let mark = seatbelt_mark::new_mark().unwrap();
    assert!(
        !seatbelt_mark::carries(pid, &mark),
        "an allow-everything sandbox matched"
    );
    foreign.kill().unwrap();
    foreign.wait().unwrap();
}

#[test]
fn a_test_builds_sweep_kills_only_what_holds_its_guard() {
    // The test builds' guard: a process that carries the mark in its sandbox but does not hold
    // the guard file open is not killed (here both processes are this test's own).
    let mark = seatbelt_mark::new_mark().unwrap();
    seatbelt_mark::make_guard(&mark).unwrap();
    let defines: Vec<String> = seatbelt_mark::params(&mark)
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    let mut bare = sandboxed_sleep(seatbelt_mark::MARK_ONLY, &defines, None);
    let guard = seatbelt_mark::guard_path(&mark);
    let mut tagged = sandboxed_sleep(seatbelt_mark::MARK_ONLY, &defines, Some(&guard));
    let (b, g) = (bare.id() as i32, tagged.id() as i32);
    let _reap = Reap(vec![b, g]);
    assert!(seatbelt_mark::carries(b, &mark) && seatbelt_mark::carries(g, &mark));
    assert_eq!(seatbelt_mark::kill_marked(&mark), 1);
    assert!(gone(g), "the tagged process stayed");
    assert!(alive(b), "the guard let an untagged process be killed");
    bare.kill().unwrap();
    bare.wait().unwrap();
    tagged.wait().unwrap();
    seatbelt_mark::remove_guard(&mark);
}

/// The process's short name.
fn proc_name(pid: i32) -> String {
    let mut buf = [0u8; 64];
    // SAFETY: a valid buffer of its own length.
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned()
}
