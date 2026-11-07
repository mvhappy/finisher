//! The library's include and exclude globs (Decision 16), matched against `relKey`s.
//!
//! Syntax: `*` (any run of characters but `/`), `?` (one character but `/`), `**` (any run,
//! `/` included; `**/` also matches nothing, so `a/**/b` matches `a/b`), `[…]` (a class of
//! characters and `x-y` ranges; `[!…]` or `[^…]` negates; a leading `]` is literal; never
//! `/`). Every other character is literal. A pattern without `/` matches an entry's name at
//! any depth (`*.tmp`); one with `/` is anchored at the root (a leading `/` is dropped:
//! `/Media/*.mov`). On a case-insensitive root the pattern is folded like the keys.

/// A pattern that cannot be compiled.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GlobError {
    #[error("an empty pattern")]
    Empty,
    #[error("an unclosed `[` class")]
    UnclosedClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Lit(char),
    One,
    Star,
    /// `**` not followed by `/`: anything.
    DStar,
    /// `**/`: nothing, or anything ending in `/`.
    DStarSlash,
    Class {
        neg: bool,
        ranges: Vec<(char, char)>,
    },
}

/// A compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    toks: Vec<Tok>,
    anchored: bool,
}

impl Glob {
    pub fn new(pattern: &str, case_sensitive: bool) -> Result<Glob, GlobError> {
        let anchored = pattern.contains('/');
        let body = pattern.strip_prefix('/').unwrap_or(pattern);
        if body.is_empty() {
            return Err(GlobError::Empty);
        }
        let fold = |c: char| -> Vec<char> {
            if case_sensitive {
                vec![c]
            } else {
                c.to_lowercase().collect()
            }
        };
        let fold1 = |c: char| match fold(c).as_slice() {
            [one] => *one,
            _ => c,
        };
        let chars: Vec<char> = body.chars().collect();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '*' if chars.get(i + 1) == Some(&'*') => {
                    if chars.get(i + 2) == Some(&'/') {
                        toks.push(Tok::DStarSlash);
                        i += 3;
                    } else {
                        toks.push(Tok::DStar);
                        i += 2;
                    }
                    continue;
                }
                '*' => toks.push(Tok::Star),
                '?' => toks.push(Tok::One),
                '[' => {
                    let mut j = i + 1;
                    let neg = matches!(chars.get(j), Some('!' | '^'));
                    if neg {
                        j += 1;
                    }
                    let mut ranges = Vec::new();
                    let mut first = true;
                    loop {
                        let c = *chars.get(j).ok_or(GlobError::UnclosedClass)?;
                        if c == ']' && !first {
                            break;
                        }
                        first = false;
                        if chars.get(j + 1) == Some(&'-')
                            && chars.get(j + 2).is_some_and(|&e| e != ']')
                        {
                            ranges.push((fold1(c), fold1(chars[j + 2])));
                            j += 3;
                        } else {
                            ranges.push((fold1(c), fold1(c)));
                            j += 1;
                        }
                    }
                    toks.push(Tok::Class { neg, ranges });
                    i = j;
                }
                c => toks.extend(fold(c).into_iter().map(Tok::Lit)),
            }
            i += 1;
        }
        Ok(Glob { toks, anchored })
    }

    /// Whether `key` (a `relKey`, already folded on a case-insensitive root) matches.
    pub fn matches(&self, key: &str) -> bool {
        let subject = if self.anchored {
            key
        } else {
            key.rsplit('/').next().unwrap_or(key)
        };
        let s: Vec<char> = subject.chars().collect();
        let m = s.len();
        // next[j]: toks[i + 1..] matches s[j..]; cur[j]: toks[i..] matches s[j..].
        let mut next = vec![false; m + 1];
        next[m] = true;
        for tok in self.toks.iter().rev() {
            let mut cur = vec![false; m + 1];
            let mut slash_ahead = false;
            for j in (0..=m).rev() {
                let c = s.get(j).copied();
                cur[j] = match tok {
                    Tok::Lit(l) => c == Some(*l) && next[j + 1],
                    Tok::One => c.is_some_and(|c| c != '/') && next[j + 1],
                    Tok::Class { neg, ranges } => {
                        c.is_some_and(|c| {
                            c != '/' && ranges.iter().any(|&(a, b)| a <= c && c <= b) != *neg
                        }) && next[j + 1]
                    }
                    Tok::Star => next[j] || (c.is_some_and(|c| c != '/') && cur[j + 1]),
                    Tok::DStar => next[j] || (c.is_some() && cur[j + 1]),
                    Tok::DStarSlash => {
                        slash_ahead = slash_ahead || (c == Some('/') && next[j + 1]);
                        next[j] || slash_ahead
                    }
                };
            }
            next = cur;
        }
        next[0]
    }
}
