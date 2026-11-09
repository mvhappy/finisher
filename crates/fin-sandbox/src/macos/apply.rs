//! Applies a Seatbelt profile (spec §8.2). Agents and hooks: the launcher calls
//! [`restrict_self`] and then `exec`s the program, which inherits the sandbox (what
//! `sandbox-exec` itself does, without putting the profile on argv). Workers: the same call in
//! the self-sandbox entry, before `hello`. Plugins: [`plugin_command`] rewrites the program to
//! `/usr/bin/sandbox-exec -p <profile> -D… -- <program> <args…>` (the job host starts its
//! children with `posix_spawn`, so nothing runs between `fork` and `exec`).
//!
//! A session's profile carries its mark ([`fin_jobhost::seatbelt_mark`], plan 7C Decision 3's
//! macOS amendment): the rule is appended to the generated text and the mark passed as a
//! parameter, so the goldens are unchanged. [`mark_only`] marks an unconfined session.

use std::ffi::{CString, OsString, c_char, c_int};
use std::path::{Path, PathBuf};

use fin_jobhost::seatbelt_mark;

use super::sbpl::{Sbpl, check_read_links, render, render_for, resolve_real};
use crate::error::ApplyError;
use crate::policy::SandboxPolicy;

/// `sandbox-exec`, whose presence is the mechanism's status on this host.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

unsafe extern "C" {
    // libsystem_sandbox (part of libSystem): the call `sandbox-exec` makes. `parameters` is a
    // NULL-terminated list of key, value, key, value…
    fn sandbox_init_with_parameters(
        profile: *const c_char,
        flags: u64,
        parameters: *const *const c_char,
        errorbuf: *mut *mut c_char,
    ) -> c_int;
    fn sandbox_free_error(errorbuf: *mut c_char);
}

/// The profile of `policy` for this host (paths resolved as the kernel matches them).
pub fn profile(policy: &SandboxPolicy) -> Result<Sbpl, ApplyError> {
    check_read_links(policy)?;
    render(policy, &resolve_real)
}

/// `sbpl` with the rule that carries `mark` appended.
pub fn with_mark(mut sbpl: Sbpl, mark: &str) -> Sbpl {
    sbpl.text.push_str(seatbelt_mark::RULE);
    sbpl.params.extend(seatbelt_mark::params(mark));
    sbpl
}

/// Confines the calling process (and everything it `exec`s or starts) to `policy`. `program`
/// is the binary the process keeps running (a worker's own, which it may read and whose
/// directory it may list; see [`super::sbpl::rules_for`]); the launcher, which `exec`s another
/// program, passes `None` and its session's `mark`.
pub fn restrict_self(
    policy: &SandboxPolicy,
    program: Option<&Path>,
    mark: Option<&str>,
) -> Result<(), ApplyError> {
    check_read_links(policy)?;
    let mut sbpl = render_for(policy, &resolve_real, program)?;
    if let Some(m) = mark {
        sbpl = with_mark(sbpl, m);
    }
    init(sbpl)
}

/// Marks the calling process (and everything it starts) with `mark` and nothing else: an
/// unconfined session (the mechanism stubbed disabled) is still found by its mark.
pub fn mark_only(mark: &str) -> Result<(), ApplyError> {
    init(Sbpl {
        text: seatbelt_mark::MARK_ONLY.into(),
        params: seatbelt_mark::params(mark).into(),
    })
}

fn init(sbpl: Sbpl) -> Result<(), ApplyError> {
    let os = |error: String| ApplyError::Os {
        call: "sandbox_init_with_parameters".into(),
        error,
    };
    let text = CString::new(sbpl.text).map_err(|e| os(e.to_string()))?;
    let mut owned = Vec::with_capacity(sbpl.params.len() * 2);
    for (k, v) in &sbpl.params {
        owned.push(CString::new(k.as_str()).map_err(|e| os(e.to_string()))?);
        owned.push(CString::new(v.as_str()).map_err(|e| os(e.to_string()))?);
    }
    let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    let mut err: *mut c_char = std::ptr::null_mut();
    // SAFETY: NUL-terminated strings and a NULL-terminated list that outlive the call; `err`
    // is a valid out pointer, freed with the library's own function.
    let rc = unsafe { sandbox_init_with_parameters(text.as_ptr(), 0, ptrs.as_ptr(), &mut err) };
    if rc != 0 {
        let msg = if err.is_null() {
            format!("returned {rc}")
        } else {
            // SAFETY: the library returned a NUL-terminated message.
            let m = unsafe { std::ffi::CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned();
            // SAFETY: allocated by the library for this call.
            unsafe { sandbox_free_error(err) };
            m
        };
        return Err(os(msg));
    }
    Ok(())
}

/// The program and arguments that start `program args…` under `policy` through
/// `sandbox-exec` (a plugin, Decision 3), its profile carrying `mark`.
pub fn plugin_command(
    policy: &SandboxPolicy,
    program: &Path,
    args: &[OsString],
    mark: &str,
) -> Result<(PathBuf, Vec<OsString>), ApplyError> {
    let sbpl = with_mark(profile(policy)?, mark);
    let mut out: Vec<OsString> = vec!["-p".into(), sbpl.text.clone().into()];
    for d in sbpl.defines() {
        out.push("-D".into());
        out.push(d.into());
    }
    out.push("--".into());
    out.push(program.as_os_str().to_owned());
    out.extend(args.iter().cloned());
    Ok((PathBuf::from(SANDBOX_EXEC), out))
}
