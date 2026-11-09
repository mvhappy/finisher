//! The Seatbelt profile generator (spec §8.2; plan 7C Decision 14). Pure: a policy and a path
//! resolver in, a profile out, on every OS (the goldens are checked everywhere).
//!
//! The profile starts `(deny default)` and states the policy's roots as rules; Seatbelt takes
//! the **last** matching rule, so the path rules are emitted shallowest first and, at one depth,
//! grants before exclusions. That is the reference model of [`SandboxPolicy::readable`] and
//! [`SandboxPolicy::writable`] exactly (the most specific root wins, a tie goes to the
//! exclusion), with no enumeration. Then, as the model has it, a directory that holds an
//! excluded path cannot itself be written (renamed, removed) or, holding a denied path, listed.
//! Every path reaches the profile as a parameter (`(param "P3")`), never as text, so no path
//! needs quoting; a path that is not UTF-8 or holds a NUL, CR or LF is refused.
//!
//! Beyond the policy, every profile gets the **Seatbelt base** (spike S9 B6): read-only system
//! code (`/` itself, which dyld needs, and `/System/Library`),
//! `sysctl-read`, and `file-read-metadata` on the ancestors of each granted path (`realpath`,
//! and `fin`'s socket, sp5 §4.11). The `agent` profile adds what its CLIs need (DNS through
//! mDNSResponder's socket, the preferences daemon's shared memory); the `worker` profile the
//! IOKit user client of Metal. As defence in depth, the programs that would
//! act outside the sandbox (`open`, `osascript`, `launchctl`, `security`, setuid helpers) are
//! refused by name (a copy in an agent's write root escapes the name, so the `mach-lookup`
//! allow-list and `(deny default)`'s refusal of `lsopen` and setuid are the guarantee). Apple Events, `fork` for a
//! worker and everything else unnamed are refused by `(deny default)` alone. Every rule here
//! has a test that fails without it (`tests/escape_macos.rs`, `fin-workers`'
//! `tests/self_sandbox_macos.rs`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use super::resolve::{Resolve, SYSTEM_LINKS, Use, check_read_links, resolve_real};
use crate::error::ApplyError;
use crate::policy::{DeviceAccess, Net, Profile, SandboxPolicy};

/// System code every process may read (the Seatbelt base): the frameworks a worker loads
/// after it restricted itself (Metal). The dyld shared cache needs no rule; `/usr/lib` and the
/// rest of the toolchain are the policy's own read roots where a profile has them.
pub const SYSTEM_READ: &[&str] = &["/System/Library"];
/// Programs refused by name (spec §8.2, defence in depth behind `mach-lookup`).
pub const REFUSED_EXEC: &[&str] = &[
    "/usr/bin/open",
    "/usr/bin/osascript",
    "/usr/bin/osacompile",
    "/bin/launchctl",
    "/usr/bin/security",
    "/usr/bin/sudo",
    "/usr/bin/su",
    "/usr/bin/login",
    "/usr/bin/newgrp",
    "/usr/bin/at",
    "/usr/bin/batch",
    "/usr/bin/crontab",
    "/usr/libexec/authopen",
    "/usr/libexec/security_authtrampoline",
];
/// mDNSResponder's socket: name resolution for the `agent` profile (spike S9 B6).
pub const MDNS_SOCKET: &str = "/private/var/run/mDNSResponder";
/// The IOKit user clients of a media worker: the Apple GPU (wgpu on Metal). Apple silicon only
/// (spike S9 B6); Intel and AMD Macs use other classes. VideoToolbox adds
/// `IOSurfaceRootUserClient` (without it decode silently delivers no frame) when sub-project 6
/// uses it.
pub const WORKER_IOKIT: &[&str] = &["AGXDeviceUserClient"];

/// One profile rule: `(<allow|deny> <ops…> <filter…>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub allow: bool,
    pub ops: Vec<&'static str>,
    /// No filter: the rule matches every object of its operations.
    pub filters: Vec<Filter>,
}

/// What a rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    /// The path and everything below it.
    Subpath(PathBuf),
    /// The path itself.
    Literal(PathBuf),
    /// A connect to this pathname socket.
    UnixSocket(PathBuf),
    /// A connect to any pathname socket.
    AnyUnixSocket,
    /// A connect to `host:port` (`localhost` or `*`; `*` port).
    Ip(&'static str, Option<u16>),
    MachName(String),
    ShmPrefix(&'static str),
    ShmName(&'static str),
    IokitClass(&'static str),
    /// `(target self)` or `(target same-sandbox)`.
    Target(&'static str),
    /// A file of this type (`FIFO`).
    VnodeType(&'static str),
}

/// A generated profile: its text and the parameters its `(param "Pn")` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sbpl {
    pub text: String,
    pub params: Vec<(String, String)>,
}

impl Sbpl {
    /// The `-D KEY=VALUE` arguments of `sandbox-exec`.
    pub fn defines(&self) -> Vec<String> {
        self.params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect()
    }

    /// The text followed by its parameters as comments (the goldens' form: still a profile).
    pub fn golden(&self) -> String {
        let mut s = self.text.clone();
        s.push_str(";; parameters\n");
        for (k, v) in &self.params {
            s.push_str(&format!(";; {k} = {v}\n"));
        }
        s
    }
}

fn depth(p: &Path) -> usize {
    p.components().count()
}

fn strict_ancestors(p: &Path) -> impl Iterator<Item = &Path> {
    p.ancestors().skip(1)
}

fn rule(allow: bool, ops: &[&'static str], filters: Vec<Filter>) -> Rule {
    Rule {
        allow,
        ops: ops.to_vec(),
        filters,
    }
}

/// The policy's rules, in order (last match wins).
pub fn rules(policy: &SandboxPolicy, resolve: &Resolve) -> Result<Vec<Rule>, ApplyError> {
    rules_for(policy, resolve, None)
}

/// [`rules`] for a process that applies the profile to itself and keeps running `program` (a
/// worker): it may read the metadata of the binary's ancestors, which CoreFoundation resolves
/// to find the main bundle (without it the media worker crashes, `SIGSEGV`, once Metal
/// starts). Nothing of the binary's directory is readable.
pub fn rules_for(
    policy: &SandboxPolicy,
    resolve: &Resolve,
    program: Option<&Path>,
) -> Result<Vec<Rule>, ApplyError> {
    let res = |ps: &[PathBuf], u| -> Result<Vec<PathBuf>, ApplyError> {
        ps.iter().map(|p| resolve(p, u)).collect()
    };
    let profile = policy.profile;
    let reads = res(&policy.read_roots(), Use::Read)?;
    let writes = res(&policy.write, Use::Other)?;
    let protect = res(&policy.protect, Use::Exclude)?;
    let denied = res(&policy.denied(), Use::Exclude)?;
    let sockets = res(&policy.unix_sockets, Use::Other)?;
    let devices = policy
        .devices
        .iter()
        .map(|d| Ok((resolve(&d.path, Use::Other)?, d.access)))
        .collect::<Result<Vec<(PathBuf, DeviceAccess)>, ApplyError>>()?;
    let system: Vec<PathBuf> = SYSTEM_READ.iter().map(PathBuf::from).collect();
    let program = program.map(|p| resolve(p, Use::Other)).transpose()?;
    let exec_reads = profile != Profile::Worker;
    let exec_writes = profile == Profile::Agent;

    let mut out = vec![rule(
        true,
        &["file-read*"],
        vec![Filter::Literal("/".into())],
    )];
    // The path rules: (depth, exclusion, order) sorts shallowest first, grants before
    // exclusions at one depth.
    let mut paths: Vec<(usize, bool, Rule)> = Vec::new();
    let mut push = |p: &PathBuf, deny: bool, ops: &[&'static str]| {
        paths.push((
            depth(p),
            deny,
            rule(!deny, ops, vec![Filter::Subpath(p.clone())]),
        ));
    };
    for p in system.iter().filter(|p| !reads.contains(p)) {
        push(p, false, &["file-read*"]);
    }
    // A root both read and written (the sign-in store) is stated once, as a write root.
    for p in reads.iter().filter(|p| !writes.contains(p)) {
        push(p, false, &["file-read*"]);
        if exec_reads {
            push(p, false, &["process-exec*"]);
        }
    }
    for p in &writes {
        push(p, false, &["file-read*", "file-write*"]);
        if exec_writes {
            push(p, false, &["process-exec*"]);
        }
    }
    // Writing excludes a protected path; reading it then needs a read root that wins over
    // the denied roots (the model's `readable`: through a read root, or by being writable).
    let deepest = |roots: &[PathBuf], p: &Path| {
        roots
            .iter()
            .filter(|r| p.starts_with(r))
            .map(|r| depth(r))
            .max()
    };
    let read_wins = |p: &Path| match (deepest(&reads, p), deepest(&denied, p)) {
        (Some(r), Some(d)) => r > d,
        (r, _) => r.is_some(),
    };
    for p in &protect {
        if read_wins(p) {
            push(p, true, &["file-write*"]);
        } else {
            push(p, true, &["file-read*", "file-write*", "process-exec*"]);
        }
    }
    for p in &denied {
        push(p, true, &["file-read*", "file-write*", "process-exec*"]);
    }
    paths.sort_by_key(|(d, deny, _)| (*d, *deny));
    out.extend(paths.into_iter().map(|(_, _, r)| r));

    for (p, access) in &devices {
        let ops: &[&'static str] = match access {
            DeviceAccess::Read => &["file-read*"],
            DeviceAccess::ReadWrite => &["file-read*", "file-write-data", "file-ioctl"],
        };
        out.push(rule(true, ops, vec![Filter::Literal(p.clone())]));
    }

    // A directory holding an excluded path is not writable itself (the model's `holds`), so
    // not listable through its write root either; holding a denied path, not listable at all.
    let inside = |a: &Path, roots: &[PathBuf]| roots.iter().any(|r| a.starts_with(r));
    let mut no_write = Vec::new();
    let mut no_list = Vec::new();
    let granted: Vec<PathBuf> = reads.iter().chain(&writes).cloned().collect();
    for e in protect.iter().chain(&denied) {
        no_write.extend(
            strict_ancestors(e)
                .filter(|a| inside(a, &writes))
                .map(Path::to_path_buf),
        );
    }
    for e in &protect {
        no_list.extend(
            strict_ancestors(e)
                .filter(|a| inside(a, &writes) && !read_wins(a))
                .map(Path::to_path_buf),
        );
    }
    for e in &denied {
        no_list.extend(
            strict_ancestors(e)
                .filter(|a| inside(a, &granted))
                .map(Path::to_path_buf),
        );
    }
    // A write root itself can be neither removed, renamed nor replaced (by a link, say) unless
    // its directory is writable: what Landlock grants (no right on the parent) and what the
    // model says (`writable` of the parent). Its entries stay writable.
    let excluded: Vec<PathBuf> = protect.iter().chain(&denied).cloned().collect();
    let writable_at = |p: &Path| {
        let wins = match (deepest(&writes, p), deepest(&excluded, p)) {
            (Some(w), Some(x)) => w > x,
            (w, _) => w.is_some(),
        };
        wins && !excluded.iter().any(|e| e.starts_with(p) && e != p)
    };
    let fixed: Vec<Filter> = writes
        .iter()
        .filter(|w| !w.parent().is_some_and(writable_at))
        .cloned()
        .map(Filter::Literal)
        .collect();
    if !fixed.is_empty() {
        out.push(rule(
            false,
            &["file-write-unlink", "file-write-create"],
            fixed,
        ));
    }
    for (list, ops) in [
        (no_write, &["file-write*"][..]),
        (no_list, &["file-read-data"]),
    ] {
        let mut list = list;
        list.sort();
        list.dedup();
        if !list.is_empty() {
            out.push(rule(
                false,
                ops,
                list.into_iter().map(Filter::Literal).collect(),
            ));
        }
    }

    // Metadata of every granted path's ancestors (`realpath`, and the socket's, sp5 §4.11).
    let mut meta: Vec<PathBuf> = granted
        .iter()
        .chain(&sockets)
        .chain(&system)
        .chain(devices.iter().map(|(p, _)| p))
        .chain(&program)
        .flat_map(|p| strict_ancestors(p).map(Path::to_path_buf))
        .filter(|a| a.parent().is_some())
        .collect();
    // The top-level links themselves (`/var` → `/private/var`): a path given through one is
    // resolved by reading the link.
    meta.extend(SYSTEM_LINKS.iter().map(|(link, _)| PathBuf::from(link)));
    meta.sort();
    meta.dedup();
    if !meta.is_empty() {
        out.push(rule(
            true,
            &["file-read-metadata"],
            meta.into_iter().map(Filter::Literal).collect(),
        ));
    }

    out.extend(process_rules(profile));
    out.extend(net_rules(policy, &sockets));
    if !policy.mach.is_empty() {
        let names = policy.mach.iter().cloned().map(Filter::MachName).collect();
        out.push(rule(true, &["mach-lookup"], names));
    }
    out.extend(platform_rules(profile));
    out.push(rule(
        false,
        &["process-exec*"],
        REFUSED_EXEC
            .iter()
            .map(|p| Filter::Literal(p.into()))
            .collect(),
    ));
    Ok(out)
}

fn process_rules(profile: Profile) -> Vec<Rule> {
    let mut out = Vec::new();
    if matches!(profile, Profile::Worker | Profile::Plugin) {
        // FIFOs only for `agent` and `hook` (Decision 6a; Landlock `MAKE_FIFO`).
        out.push(rule(
            false,
            &["file-write-create"],
            vec![Filter::VnodeType("FIFO")],
        ));
    }
    out.extend(processes(profile));
    out
}

fn processes(profile: Profile) -> Vec<Rule> {
    if profile == Profile::Worker {
        // One process: no fork and no exec (Decision 8), which `(deny default)` refuses; a
        // process may always signal itself.
        return vec![];
    }
    vec![
        rule(true, &["process-fork"], vec![]),
        rule(true, &["signal"], vec![Filter::Target("same-sandbox")]),
    ]
}

/// Network rules. Seatbelt does not evaluate filtered `network-outbound` rules by order: a
/// filtered allow wins over a filtered deny whichever comes last, and a filtered deny wins over
/// an unfiltered allow (observed on macOS 27). So open egress is an unfiltered allow, the
/// refusals (loopback, every Unix socket) are filtered denies, and the exceptions (the
/// listener's ports, the granted sockets, mDNSResponder) filtered allows.
fn net_rules(policy: &SandboxPolicy, sockets: &[PathBuf]) -> Vec<Rule> {
    let out_ip = |host, port| rule(true, &["network-outbound"], vec![Filter::Ip(host, port)]);
    let mut out = Vec::new();
    match &policy.net {
        Net::Open { loopback } => {
            out.push(rule(true, &["network-outbound"], vec![]));
            out.push(rule(
                false,
                &["network-outbound"],
                vec![Filter::Ip("localhost", None), Filter::AnyUnixSocket],
            ));
            out.extend(loopback.iter().map(|p| out_ip("localhost", Some(*p))));
        }
        Net::Loopback { ports } => out.extend(ports.iter().map(|p| out_ip("localhost", Some(*p)))),
        Net::None => {}
    }
    if !sockets.is_empty() {
        out.push(rule(
            true,
            &["network-outbound"],
            sockets.iter().cloned().map(Filter::UnixSocket).collect(),
        ));
    }
    out
}

fn platform_rules(profile: Profile) -> Vec<Rule> {
    let mut out = vec![rule(true, &["sysctl-read"], vec![])];
    match profile {
        Profile::Agent => {
            out.push(rule(
                true,
                &["network-outbound"],
                vec![Filter::UnixSocket(MDNS_SOCKET.into())],
            ));
            out.push(rule(true, &["system-socket"], vec![]));
            out.push(rule(
                true,
                &["ipc-posix-shm-read-data", "ipc-posix-shm-read-metadata"],
                vec![
                    Filter::ShmPrefix("apple.cfprefs."),
                    Filter::ShmName("apple.shm.notification_center"),
                ],
            ));
        }
        Profile::Worker => out.push(rule(
            true,
            &["iokit-open-user-client"],
            WORKER_IOKIT.iter().map(|c| Filter::IokitClass(c)).collect(),
        )),
        Profile::Hook | Profile::Plugin => {}
    }
    out
}

/// Interns each path as a parameter.
struct Params {
    keys: BTreeMap<String, String>,
    list: Vec<(String, String)>,
}

impl Params {
    fn key(&mut self, p: &Path) -> Result<String, ApplyError> {
        let bad = |why: &str| ApplyError::Rule {
            path: p.to_path_buf(),
            error: format!("cannot be a Seatbelt parameter: {why}"),
        };
        let s = p.to_str().ok_or_else(|| bad("not UTF-8"))?;
        if s.chars().any(|c| matches!(c, '\0' | '\n' | '\r')) {
            return Err(bad("it holds a NUL, CR or LF"));
        }
        if let Some(k) = self.keys.get(s) {
            return Ok(k.clone());
        }
        let k = format!("P{}", self.list.len());
        self.keys.insert(s.to_owned(), k.clone());
        self.list.push((k.clone(), s.to_owned()));
        Ok(k)
    }
}

fn filter_text(f: &Filter, params: &mut Params) -> Result<String, ApplyError> {
    Ok(match f {
        Filter::Subpath(p) => format!("(subpath (param \"{}\"))", params.key(p)?),
        Filter::Literal(p) => format!("(literal (param \"{}\"))", params.key(p)?),
        Filter::UnixSocket(p) => format!(
            "(remote unix-socket (path-literal (param \"{}\")))",
            params.key(p)?
        ),
        Filter::AnyUnixSocket => "(remote unix-socket)".into(),
        Filter::Ip(host, port) => match port {
            Some(port) => format!("(remote ip \"{host}:{port}\")"),
            None => format!("(remote ip \"{host}:*\")"),
        },
        // Checked by `validate` ([A-Za-z0-9._-]).
        Filter::MachName(n) => format!("(global-name \"{n}\")"),
        Filter::ShmPrefix(n) => format!("(ipc-posix-name-prefix \"{n}\")"),
        Filter::ShmName(n) => format!("(ipc-posix-name \"{n}\")"),
        Filter::IokitClass(c) => format!("(iokit-user-client-class \"{c}\")"),
        Filter::Target(t) => format!("(target {t})"),
        Filter::VnodeType(t) => format!("(vnode-type {t})"),
    })
}

/// The profile text of `policy` (validated first) and its parameters.
pub fn render(policy: &SandboxPolicy, resolve: &Resolve) -> Result<Sbpl, ApplyError> {
    render_for(policy, resolve, None)
}

/// [`render`] with [`rules_for`]'s `program`.
pub fn render_for(
    policy: &SandboxPolicy,
    resolve: &Resolve,
    program: Option<&Path>,
) -> Result<Sbpl, ApplyError> {
    policy.validate()?;
    let mut params = Params {
        keys: BTreeMap::new(),
        list: Vec::new(),
    };
    let profile = serde_json::to_value(policy.profile).unwrap_or_default();
    let mut text = format!(
        ";; Generated by fin-sandbox from the {} profile's policy (plan 7C Task 8).\n;; Last match wins; every path is a parameter.\n(version 1)\n(deny default)\n",
        profile.as_str().unwrap_or("?")
    );
    for r in rules_for(policy, resolve, program)? {
        text.push('(');
        text.push_str(if r.allow { "allow" } else { "deny" });
        for op in &r.ops {
            text.push(' ');
            text.push_str(op);
        }
        let many = r.filters.len() > 1;
        for f in &r.filters {
            text.push_str(if many { "\n    " } else { " " });
            text.push_str(&filter_text(f, &mut params)?);
        }
        text.push_str(")\n");
    }
    Ok(Sbpl {
        text,
        params: params.list,
    })
}
