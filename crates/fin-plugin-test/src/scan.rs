//! What must never be in a plugin's logs (spec §11): a path or a secret. A path is what the
//! host's scrubber treats as one (`apps/finisherd/src/plugins/scrub.ts`'s `PATH_LIKE`): an
//! absolute POSIX path of two or more parts, a drive path or a UNC path, after the start of
//! the text or one of `\s " ' ( = : ,`. URLs are not paths (`https://h/p`: the `/` after `:`
//! starts an empty part).

/// Characters a path part holds (`[\w.@~+-]`).
fn part_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || matches!(c, '.' | '@' | '~' | '+' | '-')
}

fn lead(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '(' | '=' | ':' | ',')
}

/// `/a/…`: a part, then at least one more `/`.
fn posix_at(s: &[char]) -> bool {
    if s.first() != Some(&'/') {
        return false;
    }
    let part = s[1..].iter().take_while(|c| part_char(**c)).count();
    part > 0 && s.get(1 + part) == Some(&'/')
}

/// `C:\` or `C:/`.
fn drive_at(s: &[char]) -> bool {
    s.len() >= 3 && s[0].is_ascii_alphabetic() && s[1] == ':' && matches!(s[2], '\\' | '/')
}

/// `\\server\`.
fn unc_at(s: &[char]) -> bool {
    if s.len() < 3 || s[0] != '\\' || s[1] != '\\' {
        return false;
    }
    let host = s[2..]
        .iter()
        .take_while(|c| c.is_alphanumeric() || matches!(**c, '_' | '.' | '$' | '-'))
        .count();
    host > 0 && s.get(2 + host) == Some(&'\\')
}

/// Whether `text` holds a path as the host's scrubber sees one.
pub fn has_path(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len()).any(|i| {
        (i == 0 || lead(chars[i - 1]))
            && (posix_at(&chars[i..]) || drive_at(&chars[i..]) || unc_at(&chars[i..]))
    })
}

/// Why `text` must not be logged: `path` or `secret` (one of `secrets` appears in it).
pub fn leak(text: &str, secrets: &[String], paths: &[String]) -> Option<&'static str> {
    if secrets
        .iter()
        .any(|s| !s.is_empty() && text.contains(s.as_str()))
    {
        return Some("secret");
    }
    if has_path(text)
        || paths
            .iter()
            .any(|p| !p.is_empty() && text.contains(p.as_str()))
    {
        return Some("path");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_as_the_host_scrubs_them() {
        for p in [
            "/home/someone/file.txt",
            "open /var/lib/x failed",
            "at (/srv/app/main.mjs:3:1)",
            "path=/tmp/a",
            r"C:\Users\x",
            "D:/data",
            r"\\server\share",
        ] {
            assert!(has_path(p), "{p}");
        }
        for p in [
            "trim-silence: 4 items",
            "https://api.example.org/about",
            "a/b/c",
            "/single",
            "ratio 1/2/3",
        ] {
            assert!(!has_path(p), "{p}");
        }
    }

    #[test]
    fn secrets_and_known_dirs() {
        let secrets = ["s3cr3t".to_owned()];
        let paths = ["plugin-tmp-xyz".to_owned()];
        assert_eq!(leak("cookie s3cr3t", &secrets, &paths), Some("secret"));
        assert_eq!(leak("in plugin-tmp-xyz", &secrets, &paths), Some("path"));
        assert_eq!(leak("all fine", &secrets, &paths), None);
    }
}
