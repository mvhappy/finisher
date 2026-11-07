//! Scrubbing (plan D, Decision 26): Finisher-authored text loses the home, server-data and
//! temp directories, the user name, the host name and every other absolute path before it
//! becomes a [`super::class::Scrubbed`]. Pure: the emitter passes the [`ScrubCtx`].
//!
//! The input is cut to [`MAX_INPUT`] bytes first. Then, in order:
//! 1. the directories, longest first, case-insensitively (Unicode lower case), where the
//!    directory ends (not inside a longer name): `<data>`, `<tmp>`, `<home>`. When a
//!    separator follows, the rest of the path goes too: `<home>/<path>`;
//! 2. any absolute path that starts a token (at the start, after white space or one of
//!    `"'`([=:,`): `/…`, `~/…`, `~\…`, `~user/…`, `C:\…`, `C:/…` and `\\…` become `<path>`;
//! 3. the host and the user name, the longer first, case-insensitively (Unicode lower case),
//!    anywhere outside a placeholder when they have at least 3 characters, else as a whole
//!    word (`<host>`, `<user>`).
//!
//! Where a path ends: a path that begins right after a quote runs to the matching quote. Any
//! other runs to the next white space or one of `"'`)]>,;`, and then on over up to two more
//! words when one of them holds a separator (`C:\Program Files\App\x.dll`,
//! `/Users/a/My Movies/x.mov`), unless that word starts a path of its own.
//!
//! The result is cut to [`MAX_SCRUBBED`] bytes (on a character boundary, with `…`).

use super::class::ScrubCtx;

/// The longest scrubbed text kept, in bytes.
pub const MAX_SCRUBBED: usize = 4096;

/// The longest input scrubbed, in bytes: longer text is cut first.
pub const MAX_INPUT: usize = 4 * MAX_SCRUBBED;

const PLACEHOLDERS: [&str; 6] = ["<home>", "<data>", "<tmp>", "<path>", "<user>", "<host>"];

/// `text` with the rules above applied.
pub fn scrub(text: &str, ctx: &ScrubCtx) -> String {
    let mut out = cut(text, MAX_INPUT).to_string();
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
        out = replace_dir(&out, dir, placeholder);
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
        let word = name.chars().count() < 3;
        out = outside_placeholders(&out, |seg| replace_name(seg, name, placeholder, word));
    }
    truncate(out)
}

fn trim_sep(dir: &str) -> &str {
    let t = dir.trim_end_matches(['/', '\\']);
    if t.is_empty() { dir } else { t }
}

/// `text` cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The occurrences of `needle` in `text`, case-insensitively (Unicode lower case), as byte
/// ranges of `text`, left to right, not overlapping. A match must start and end on the
/// boundary of a character's lower-case form.
fn find_ci(text: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut lower = String::with_capacity(text.len());
    // For each byte of `lower` (and its end): the offset in `text` of the character whose
    // lower-case form starts there, if one does.
    let mut orig: Vec<Option<usize>> = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        let start = lower.len();
        lower.extend(c.to_lowercase());
        orig.push(Some(i));
        orig.extend(std::iter::repeat_n(None, lower.len() - start - 1));
    }
    orig.push(Some(text.len()));
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&needle).map(|i| i + from) {
        let end = i + needle.len();
        match (orig[i], orig[end]) {
            (Some(s), Some(e)) => {
                out.push((s, e));
                from = end;
            }
            _ => from = i + lower[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    out
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

fn starts_token(prev: Option<char>) -> bool {
    prev.is_none_or(|c| c.is_whitespace() || "\"'`([=:,".contains(c))
}

fn ends_path(c: char) -> bool {
    c.is_whitespace() || "\"'`)]>,;".contains(c)
}

/// Whether an absolute path starts `rest`.
fn starts_path(rest: &str) -> bool {
    let b = rest.as_bytes();
    let posix = b.first() == Some(&b'/')
        && b.get(1)
            .is_some_and(|c| !c.is_ascii_whitespace() && !b"\"'`)]>,;".contains(c));
    let tilde = b.first() == Some(&b'~') && {
        let user = b[1..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
            .count();
        matches!(b.get(1 + user), Some(b'/' | b'\\'))
    };
    let drive =
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/');
    let unc = b.starts_with(b"\\\\");
    posix || tilde || drive || unc
}

/// Where a path that starts at `start` ends (module docs); `quote` is the quote right before
/// the token, if any.
fn path_end(text: &str, start: usize, quote: Option<char>) -> usize {
    let rest = &text[start..];
    if let Some(i) = quote.and_then(|q| rest.find(q)) {
        return start + i;
    }
    let mut end = start + rest.find(ends_path).unwrap_or(rest.len());
    'more: loop {
        let mut cursor = end;
        for _ in 0..3 {
            if !text[cursor..].starts_with(' ') {
                break 'more;
            }
            let word_start = cursor + 1;
            let len = text[word_start..]
                .find(|c: char| c.is_whitespace() || "\"'`,;".contains(c))
                .unwrap_or(text.len() - word_start);
            let word = &text[word_start..word_start + len];
            if word.is_empty() || starts_path(word) {
                break 'more;
            }
            cursor = word_start + len;
            if word.contains(['/', '\\']) {
                end = cursor;
                continue 'more;
            }
        }
        break;
    }
    end
}

/// `text` with each occurrence of the directory `dir` replaced by `placeholder`, and the rest
/// of its path, when a separator follows, by `<path>`.
fn replace_dir(text: &str, dir: &str, placeholder: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e) in find_ci(text, dir) {
        let next = text[e..].chars().next();
        // Inside a longer name (`/home/alice2`): not this directory.
        if s < last || next.is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-')) {
            continue;
        }
        out.push_str(&text[last..s]);
        out.push_str(placeholder);
        last = e;
        if let Some(sep @ ('/' | '\\')) = next {
            let quote = text[..s].chars().next_back().filter(|c| is_quote(*c));
            let tail = e + 1;
            let end = path_end(text, tail, quote);
            if end > tail {
                out.push(sep);
                out.push_str("<path>");
                last = end;
            }
        }
    }
    out.push_str(&text[last..]);
    out
}

fn absolute_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev: Option<char> = None;
    let mut i = 0;
    while let Some(c) = text[i..].chars().next() {
        if starts_token(prev) && starts_path(&text[i..]) {
            out.push_str("<path>");
            i = path_end(text, i, prev.filter(|c| is_quote(*c)));
            prev = Some('>');
            continue;
        }
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

/// `f` applied to the text between placeholders, which are kept as they are.
fn outside_placeholders(text: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let next = PLACEHOLDERS
            .iter()
            .filter_map(|p| rest.find(p).map(|i| (i, p.len())))
            .min();
        let Some((i, len)) = next else {
            out.push_str(&f(rest));
            return out;
        };
        out.push_str(&f(&rest[..i]));
        out.push_str(&rest[i..i + len]);
        rest = &rest[i + len..];
    }
}

/// `text` with every occurrence of `name` replaced; `word` keeps only those not inside a word
/// (alphanumeric on either side).
fn replace_name(text: &str, name: &str, with: &str, word: bool) -> String {
    let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (s, e) in find_ci(text, name) {
        let boundary = !alnum(text[..s].chars().next_back()) && !alnum(text[e..].chars().next());
        if !word || boundary {
            out.push_str(&text[last..s]);
            out.push_str(with);
            last = e;
        }
    }
    out.push_str(&text[last..]);
    out
}

fn truncate(mut s: String) -> String {
    if s.len() <= MAX_SCRUBBED {
        return s;
    }
    let keep = cut(&s, MAX_SCRUBBED - '…'.len_utf8()).len();
    s.truncate(keep);
    s.push('…');
    s
}
