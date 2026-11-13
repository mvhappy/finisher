//! Follow-up commands (review batch L): every `next`, `<key>Next`, `conflictsNext` and listing
//! named in a fix is written through [`cmd`], so it carries the targeting globals of the
//! command that printed it (`--project <dir>`, `--branch <b>`) and reads the same project and
//! branch. Set once per run from the parsed globals ([`set`]); unset (unit tests) it is `fin`.

use std::sync::OnceLock;

use crate::commands::Globals;

static PREFIX: OnceLock<String> = OnceLock::new();

/// A word as a shell would read it back: bare when it has no special character, else in
/// single quotes (`'` written `'\''`).
pub fn quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@#=,+%".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// `fin` with the targeting globals of `g`.
pub fn prefix_of(g: &Globals) -> String {
    let mut out = String::from("fin");
    if let Some(p) = &g.project {
        out.push_str(&format!(" --project {}", quote(&p.to_string_lossy())));
    }
    if let Some(b) = &g.branch {
        out.push_str(&format!(" --branch {}", quote(b)));
    }
    out
}

/// Records this run's globals (the first call wins).
pub fn set(g: &Globals) {
    let _ = PREFIX.set(prefix_of(g));
}

/// `fin [--project <dir>] [--branch <b>] <rest>`.
pub fn cmd(rest: &str) -> String {
    let prefix = PREFIX.get().map_or("fin", String::as_str);
    format!("{prefix} {rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carries_project_and_branch_quoted() {
        let g = Globals {
            project: Some("/tmp/my film".into()),
            branch: Some("work".into()),
            ..Globals::default()
        };
        assert_eq!(prefix_of(&g), "fin --project '/tmp/my film' --branch work");
        assert_eq!(prefix_of(&Globals::default()), "fin");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(cmd("view"), "fin view", "unset: plain fin");
    }
}
