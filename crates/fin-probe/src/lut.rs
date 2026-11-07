//! `.cube` LUTs (the Adobe/Resolve text format): `LUT_3D_SIZE` 2–64 and exactly size³ rows
//! of three numbers. 1D LUTs are not a kind.

use std::path::Path;

use fin_core::{AssetKind, CoreError, LutInfo};

use crate::{Probe, unsupported};

/// The edge sizes a probe accepts.
pub const SIZES: std::ops::RangeInclusive<u32> = 2..=64;

/// Keywords a `.cube` header may hold, before the first data row.
const KEYWORDS: &[&str] = &[
    "TITLE",
    "LUT_1D_SIZE",
    "LUT_3D_SIZE",
    "DOMAIN_MIN",
    "DOMAIN_MAX",
    "LUT_1D_INPUT_RANGE",
    "LUT_3D_INPUT_RANGE",
];

/// The lines that carry something: trimmed, without blank lines and `#` comments.
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

fn keyword(line: &str) -> &str {
    line.split_ascii_whitespace().next().unwrap_or("")
}

/// Whether the text's first line that carries something is a `.cube` keyword.
pub(crate) fn is_cube(text: &str) -> bool {
    lines(text)
        .next()
        .is_some_and(|l| KEYWORDS.contains(&keyword(l)))
}

fn numbers(line: &str, n: usize) -> Option<Vec<f64>> {
    let nums: Option<Vec<f64>> = line
        .split_ascii_whitespace()
        .map(|p| p.parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect();
    nums.filter(|v| v.len() == n)
}

pub(crate) fn probe(text: &str, path: &Path) -> Result<Probe, CoreError> {
    let bad = |why: String| unsupported(path, &why);
    let mut size: Option<u32> = None;
    let mut rows: u64 = 0;
    for (i, line) in lines(text).enumerate() {
        let word = keyword(line);
        if word.starts_with(|c: char| c.is_ascii_alphabetic()) {
            if rows > 0 {
                return Err(bad(format!("keyword {word} after the data rows")));
            }
            match word {
                "LUT_3D_SIZE" => {
                    let n = line[word.len()..].trim().parse::<u32>().ok();
                    match n.filter(|n| SIZES.contains(n)) {
                        Some(n) if size.is_none() => size = Some(n),
                        Some(_) => return Err(bad("LUT_3D_SIZE given twice".into())),
                        None => {
                            return Err(bad(format!(
                                "LUT_3D_SIZE must be {} to {}",
                                SIZES.start(),
                                SIZES.end()
                            )));
                        }
                    }
                }
                "LUT_1D_SIZE" => return Err(bad("1D LUTs are not supported".into())),
                "TITLE" | "DOMAIN_MIN" | "DOMAIN_MAX" | "LUT_3D_INPUT_RANGE" => {}
                other => return Err(bad(format!("unknown keyword {other}"))),
            }
            continue;
        }
        if numbers(line, 3).is_none() {
            return Err(bad(format!("data line {} is not three numbers", i + 1)));
        }
        rows += 1;
    }
    let size = size.ok_or_else(|| bad("no LUT_3D_SIZE".into()))?;
    let want = u64::from(size).pow(3);
    if rows != want {
        return Err(bad(format!(
            "{rows} data rows; LUT_3D_SIZE {size} needs {want}"
        )));
    }
    Ok(Probe {
        lut: Some(LutInfo { size }),
        ..Probe::of(AssetKind::Lut)
    })
}
