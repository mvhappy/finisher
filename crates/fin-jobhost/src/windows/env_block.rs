//! The two UTF-16 blocks `CreateProcessW` takes.
//!
//! - **Environment** (`CREATE_UNICODE_ENVIRONMENT`): exactly the request's variables, nothing
//!   of the host's. Names keep their case (`Path` stays `Path`); a later name equal to an
//!   earlier one ignoring ASCII case replaces it; the block is sorted by upper-cased name, as
//!   Windows keeps its own. A name that is empty, holds `=` after its first character or a
//!   NUL, or a value with a NUL, is refused.
//! - **Command line**: the program, always quoted (a path cannot hold `"`), then each argument
//!   quoted by the documented `CommandLineToArgvW` rules (which the MSVC runtime and Rust's
//!   `std::env::args` share): an argument with no space, tab, newline, vertical tab or `"`,
//!   and not empty, goes as is; otherwise it is quoted, `"` becomes `\"`, and backslashes are
//!   doubled where a quote follows them.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

fn invalid(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, what)
}

/// `s` as UTF-16 with a terminating NUL; an interior NUL is refused.
pub(super) fn wide(s: &OsStr) -> io::Result<Vec<u16>> {
    let mut w: Vec<u16> = s.encode_wide().collect();
    if w.contains(&0) {
        return Err(invalid(format!("{s:?} holds a NUL")));
    }
    w.push(0);
    Ok(w)
}

fn upper(c: u16) -> u16 {
    if (u16::from(b'a')..=u16::from(b'z')).contains(&c) {
        c - 32
    } else {
        c
    }
}

fn key_of(name: &[u16]) -> Vec<u16> {
    name.iter().map(|&c| upper(c)).collect()
}

/// The environment block: `NAME=value\0…\0`, or `\0\0` when empty.
pub(super) fn env_block(env: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut vars: Vec<(Vec<u16>, Vec<u16>, Vec<u16>)> = Vec::with_capacity(env.len());
    for (k, v) in env {
        let name: Vec<u16> = k.encode_wide().collect();
        let value: Vec<u16> = v.encode_wide().collect();
        if name.is_empty() || name[1..].contains(&u16::from(b'=')) || name.contains(&0) {
            return Err(invalid(format!("bad environment name {k:?}")));
        }
        if value.contains(&0) {
            return Err(invalid(format!("the value of {k:?} holds a NUL")));
        }
        let key = key_of(&name);
        match vars.iter_mut().find(|(k, _, _)| *k == key) {
            Some(slot) => *slot = (key, name, value),
            None => vars.push((key, name, value)),
        }
    }
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    let mut block = Vec::new();
    for (_, name, value) in vars {
        block.extend_from_slice(&name);
        block.push(u16::from(b'='));
        block.extend_from_slice(&value);
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

fn push_quoted(out: &mut Vec<u16>, arg: &[u16]) {
    const BS: u16 = b'\\' as u16;
    const QUOTE: u16 = b'"' as u16;
    let plain = !arg.is_empty()
        && !arg
            .iter()
            .any(|&c| matches!(c, 0x20 | 0x09 | 0x0A | 0x0B) || c == QUOTE);
    if plain {
        out.extend_from_slice(arg);
        return;
    }
    out.push(QUOTE);
    let mut backslashes = 0usize;
    for &c in arg {
        if c == BS {
            backslashes += 1;
            continue;
        }
        let n = if c == QUOTE {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        out.extend(std::iter::repeat_n(BS, n));
        backslashes = 0;
        out.push(c);
    }
    out.extend(std::iter::repeat_n(BS, backslashes * 2));
    out.push(QUOTE);
}

/// The command line for `program` and `args`, NUL-terminated.
pub(super) fn command_line(program: &Path, args: &[OsString]) -> io::Result<Vec<u16>> {
    let prog: Vec<u16> = program.as_os_str().encode_wide().collect();
    if prog.contains(&u16::from(b'"')) || prog.contains(&0) {
        return Err(invalid(format!("bad program path {program:?}")));
    }
    let mut out = Vec::with_capacity(prog.len() + 3);
    out.push(u16::from(b'"'));
    out.extend_from_slice(&prog);
    out.push(u16::from(b'"'));
    for a in args {
        let w: Vec<u16> = a.encode_wide().collect();
        if w.contains(&0) {
            return Err(invalid(format!("argument {a:?} holds a NUL")));
        }
        out.push(u16::from(b' '));
        push_quoted(&mut out, &w);
    }
    out.push(0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    /// The arguments `CommandLineToArgvW` reads back from `line`.
    fn parse(line: &[u16]) -> Vec<OsString> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
        let mut n = 0;
        // SAFETY: a NUL-terminated line; the array is freed below.
        let argv = unsafe { CommandLineToArgvW(line.as_ptr(), &mut n) };
        assert!(!argv.is_null());
        let out = (0..n as usize)
            .map(|i| {
                // SAFETY: `n` NUL-terminated strings.
                let p = unsafe { *argv.add(i) };
                let len = (0..).take_while(|&j| unsafe { *p.add(j) } != 0).count();
                OsString::from_wide(unsafe { std::slice::from_raw_parts(p, len) })
            })
            .collect();
        // SAFETY: allocated by CommandLineToArgvW.
        unsafe { LocalFree(argv.cast()) };
        out
    }

    #[test]
    fn fifty_awkward_arguments_round_trip() {
        let mut args: Vec<String> = [
            "",
            " ",
            "  ",
            "\t",
            "\n",
            "\u{b}",
            "a b",
            "\"",
            "\"\"",
            "\\",
            "\\\\",
            "\\\"",
            "a\\",
            "a\\\\",
            "a\\\"b",
            "\\\\\"",
            "\"a b\"",
            "a\"b c",
            "c:\\path with space\\",
            "c:\\trailing\\",
            "x\\\\y\\\\",
            "'single'",
            "%PATH%",
            "^&|<>",
            "!bang!",
            "a=b",
            "--flag=\"v\"",
            "é",
            "日本語",
            "emoji 🎬",
            "\\\\server\\share\\",
            "a\tb",
            "a\nb",
            "\"\\\"",
            " lead",
            "trail ",
            "\\ \\",
            "\" \"",
            "\\\\\\\"\\\\\\",
            "end\\\\\"",
            "--warm",
            "-",
            "--",
            "a b \\\" c",
            "\"\"\"",
            "a\\b\\c",
            "{\"json\":1}",
            "semi;colon",
            "comma,list",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        args.push("x".repeat(2000));
        assert!(args.len() >= 50, "{}", args.len());
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        let prog = Path::new(r"C:\Program Files\fin workers\fin-workers.exe");
        let line = command_line(prog, &args).unwrap();
        let back = parse(&line);
        assert_eq!(back[0], prog.as_os_str());
        assert_eq!(&back[1..], &args[..]);
    }

    #[test]
    fn nul_and_quotes_are_refused() {
        assert!(command_line(Path::new("C:\\a\"b.exe"), &[]).is_err());
        assert!(command_line(Path::new("C:\\a.exe"), &["a\0b".into()]).is_err());
        assert!(wide(OsStr::new("a\0")).is_err());
    }

    fn block_of(env: &[(&str, &str)]) -> io::Result<String> {
        let env: Vec<(OsString, OsString)> = env
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        env_block(&env).map(|b| String::from_utf16(&b).unwrap())
    }

    #[test]
    fn env_block_is_sorted_and_keeps_case() {
        assert_eq!(block_of(&[]).unwrap(), "\0\0");
        assert_eq!(
            block_of(&[("b", "2"), ("Path", "x"), ("A", "1")]).unwrap(),
            "A=1\0b=2\0Path=x\0\0"
        );
        // A later name equal ignoring case replaces the earlier one, with its own case.
        assert_eq!(
            block_of(&[("PATH", "1"), ("Path", "2")]).unwrap(),
            "Path=2\0\0"
        );
        assert_eq!(block_of(&[("=C:", "C:\\")]).unwrap(), "=C:=C:\\\0\0");
        for bad in [("", "v"), ("a=b", "v"), ("a\0", "v"), ("a", "v\0")] {
            assert!(block_of(&[bad]).is_err(), "{bad:?}");
        }
    }
}
