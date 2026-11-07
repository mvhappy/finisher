//! Scrubbing (plan D, Decision 26): Finisher-authored text loses the home, server-data and
//! temp directories, the user name, the host name and every other absolute path before it
//! becomes a [`super::class::Scrubbed`]. Pure: the emitter passes the [`ScrubCtx`].
//!
//! Rules, in order:
//! 1. the directories, longest first, case-insensitively (`<data>`, `<tmp>`, `<home>`);
//! 2. any absolute path that starts a token (at the start, after white space or one of
//!    `"'`([=:,`): `/…`, `~/…`, `C:\…`, `C:/…` and `\\…` run to the next white space or
//!    one of `"'`)]>,;` and become `<path>`;
//! 3. the host and the user name, the longer first, case-insensitively, anywhere in the text
//!    when they have at least 3 characters, else as a whole word (`<host>`, `<user>`).
//!
//! The result is cut to [`MAX_SCRUBBED`] bytes (on a character boundary, with `…`).

use super::class::ScrubCtx;

/// The longest scrubbed text kept, in bytes.
pub const MAX_SCRUBBED: usize = 4096;

/// `text` with the rules above applied.
pub fn scrub(text: &str, ctx: &ScrubCtx) -> String {
    let mut out = text.to_string();
    let mut dirs: Vec<(&str, &str)> = [
        (ctx.server_data.as_deref(), "<data>"),
        (ctx.tmp.as_deref(), "<tmp>"),
        (ctx.home.as_deref(), "<home>"),
    ]
    .into_iter()
    .filter_map(|(d, p)| Some((trim_sep(d?), p)))
    .filter(|(d, _)| d.len() > 1)
    .collect();
    dirs.sort_by_key(|(d, _)| std::cmp::Reverse(d.len()));
    for (dir, placeholder) in dirs {
        out = replace_ci(&out, dir, placeholder, false);
    }
    out = absolute_paths(&out);
    let mut names: Vec<(&str, &str)> = [
        (ctx.user.as_deref(), "<user>"),
        (ctx.host.as_deref(), "<host>"),
    ]
    .into_iter()
    .filter_map(|(n, p)| Some((n?.trim(), p)))
    .filter(|(n, _)| !n.is_empty())
    .collect();
    // The longer first: a host name often holds the user name.
    names.sort_by_key(|(n, _)| std::cmp::Reverse(n.len()));
    for (name, placeholder) in names {
        out = replace_ci(&out, name, placeholder, name.chars().count() < 3);
    }
    truncate(out)
}

fn trim_sep(dir: &str) -> &str {
    let t = dir.trim_end_matches(['/', '\\']);
    if t.is_empty() { dir } else { t }
}

/// `text` with every ASCII-case-insensitive occurrence of `needle` replaced; `word` keeps only
/// occurrences not inside a word (alphanumeric on either side).
fn replace_ci(text: &str, needle: &str, with: &str, word: bool) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle).map(|i| i + from) {
        let end = i + needle.len();
        let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
        let boundary = !alnum(text[..i].chars().next_back()) && !alnum(text[end..].chars().next());
        if !word || boundary {
            out.push_str(&text[last..i]);
            out.push_str(with);
            last = end;
        }
        from = end;
    }
    out.push_str(&text[last..]);
    out
}

fn starts_token(prev: Option<char>) -> bool {
    prev.is_none_or(|c| c.is_whitespace() || "\"'`([=:,".contains(c))
}

fn ends_path(c: char) -> bool {
    c.is_whitespace() || "\"'`)]>,;".contains(c)
}

/// The length in bytes of the absolute-path prefix of `rest`, if it starts with one.
fn path_start(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    let posix = b.first() == Some(&b'/')
        && b.get(1)
            .is_some_and(|c| !c.is_ascii_whitespace() && !b"\"'`)]>,;".contains(c));
    let home = b.starts_with(b"~/");
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let unc = b.starts_with(b"\\\\");
    if !(posix || home || drive || unc) {
        return None;
    }
    Some(rest.find(ends_path).unwrap_or(rest.len()))
}

fn absolute_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        if starts_token(prev)
            && let Some(len) = path_start(rest)
        {
            out.push_str("<path>");
            i += len;
            prev = Some('>');
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        prev = Some(c);
        i += c.len_utf8().max(1);
    }
    out
}

fn truncate(mut s: String) -> String {
    if s.len() <= MAX_SCRUBBED {
        return s;
    }
    let mut cut = MAX_SCRUBBED - '…'.len_utf8();
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
    s.push('…');
    s
}
