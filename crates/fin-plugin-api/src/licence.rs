//! Licence classes of SPDX expressions (8A Decision 6).
//!
//! - `permissive`: an id the repository's `deny.toml` allows (MPL-2.0 aside), plus `OFL-1.1`,
//!   the font licence fin-fonts' catalog rules allow (font packs);
//! - `weakCopyleft`: `MPL-2.0`, `EPL-2.0`;
//! - `copyleft`: GPL, LGPL, AGPL, SSPL, EUPL, CC-BY-SA (any version, `+` or exception);
//! - `nonCommercial`: any NC licence (`CC-BY-NC*`, PolyForm Noncommercial, a `LicenseRef`
//!   naming non-commercial use): refused;
//! - `unknown`: not SPDX, a `LicenseRef`, or an SPDX id in none of the lists: refused for
//!   bundled plugins, a warning elsewhere.
//!
//! `OR` takes its most permissive branch and `AND` its least, in the order permissive <
//! weakCopyleft < unknown < copyleft < nonCommercial: an unknown branch beside a copyleft one
//! under `AND` still needs the copyleft acceptance, and one beside a permissive one under `OR`
//! is the permissive one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spdx::expression::{ExprNode, Operator};
use spdx::{Expression, LicenseItem, LicenseReq, ParseMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LicenceClass {
    Permissive,
    WeakCopyleft,
    Copyleft,
    NonCommercial,
    Unknown,
}

impl LicenceClass {
    /// Rank for `AND` (worst wins) and `OR` (best wins).
    fn rank(self) -> u8 {
        match self {
            LicenceClass::Permissive => 0,
            LicenceClass::WeakCopyleft => 1,
            LicenceClass::Unknown => 2,
            LicenceClass::Copyleft => 3,
            LicenceClass::NonCommercial => 4,
        }
    }

    fn worse(self, o: LicenceClass) -> LicenceClass {
        if o.rank() > self.rank() { o } else { self }
    }

    fn better(self, o: LicenceClass) -> LicenceClass {
        if o.rank() < self.rank() { o } else { self }
    }
}

const PERMISSIVE: &[&str] = &[
    "MIT",
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "CC0-1.0",
    "BSL-1.0",
    "OFL-1.1",
];
const WEAK_COPYLEFT: &[&str] = &["MPL-2.0", "EPL-2.0"];
const COPYLEFT_PREFIXES: &[&str] = &["GPL-", "LGPL-", "AGPL-", "SSPL-", "EUPL-", "CC-BY-SA-"];

/// Whether a licence name says non-commercial (`CC-BY-NC-4.0`, `CC-BY-NC-SA-4.0`,
/// `PolyForm-Noncommercial-1.0.0`, `LicenseRef-…-NonCommercial`).
fn says_non_commercial(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("noncommercial")
        || lower.contains("non-commercial")
        || lower.split(['-', '_', '.', ' ']).any(|part| part == "nc")
}

fn class_of(req: &LicenseReq) -> LicenceClass {
    let name = match &req.license {
        LicenseItem::Spdx { id, .. } => id.name,
        LicenseItem::Other(r) => {
            return if says_non_commercial(&r.lic_ref) {
                LicenceClass::NonCommercial
            } else {
                LicenceClass::Unknown
            };
        }
    };
    if says_non_commercial(name) {
        return LicenceClass::NonCommercial;
    }
    if COPYLEFT_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return LicenceClass::Copyleft;
    }
    if WEAK_COPYLEFT.contains(&name) {
        return LicenceClass::WeakCopyleft;
    }
    let addition_ok = match &req.addition {
        None => true,
        // `deny.toml` allows exactly this one.
        Some(a) => name == "Apache-2.0" && a.to_string() == "LLVM-exception",
    };
    if PERMISSIVE.contains(&name) && addition_ok {
        LicenceClass::Permissive
    } else {
        LicenceClass::Unknown
    }
}

const MODE: ParseMode = ParseMode {
    allow_slash_as_or_operator: false,
    allow_imprecise_license_names: false,
    allow_postfix_plus_on_gpl: true,
    // A deprecated id (`GPL-2.0`) still classifies by its family.
    allow_deprecated: true,
    allow_unknown: false,
};

/// The class of an SPDX expression. Text that does not parse is `unknown`, or
/// `nonCommercial` when it says so.
pub fn classify(spdx: &str) -> LicenceClass {
    let Ok(expr) = Expression::parse_mode(spdx, MODE) else {
        return if says_non_commercial(spdx) {
            LicenceClass::NonCommercial
        } else {
            LicenceClass::Unknown
        };
    };
    let mut stack: Vec<LicenceClass> = Vec::new();
    for node in expr.iter() {
        match node {
            ExprNode::Req(r) => stack.push(class_of(&r.req)),
            ExprNode::Op(op) => {
                let (Some(b), Some(a)) = (stack.pop(), stack.pop()) else {
                    return LicenceClass::Unknown;
                };
                stack.push(match op {
                    Operator::And => a.worse(b),
                    Operator::Or => a.better(b),
                });
            }
        }
    }
    match stack.as_slice() {
        [c] => *c,
        _ => LicenceClass::Unknown,
    }
}
