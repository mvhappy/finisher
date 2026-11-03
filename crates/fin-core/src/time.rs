//! Exact rational time. Timeline positions are seconds as `Rational`; never floats.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;

use crate::error::CoreError;

/// A rational number `num/den` in lowest terms with `den > 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rational {
    num: i64,
    den: i64,
}

fn gcd(mut a: i128, mut b: i128) -> i128 {
    a = a.abs();
    b = b.abs();
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

fn invalid_time(input: &str, why: &str) -> CoreError {
    CoreError::new("INVALID_TIME", format!("cannot parse time {input:?}: {why}"))
        .with_details(json!({ "input": input }))
        .with_fix("use 12.4s, 00:12.4, 01:00:12.4, f300 or a rational like 62/5")
}

fn overflow() -> CoreError {
    CoreError::new("TIME_OVERFLOW", "rational time overflowed 64 bits")
}

impl Rational {
    /// Builds `num/den`, reduced, with the sign carried by the numerator.
    pub fn new(num: i64, den: i64) -> Result<Rational, CoreError> {
        Self::from_i128(num as i128, den as i128).map_err(|e| {
            if e.code == "INVALID_TIME" {
                CoreError::new("INVALID_TIME", "denominator must not be zero")
                    .with_details(json!({ "input": format!("{num}/{den}") }))
            } else {
                e
            }
        })
    }

    fn from_i128(num: i128, den: i128) -> Result<Rational, CoreError> {
        if den == 0 {
            return Err(invalid_time("", "denominator is zero"));
        }
        let g = gcd(num, den).max(1);
        let sign = if den < 0 { -1 } else { 1 };
        let (n, d) = (sign * num / g, sign * den / g);
        match (i64::try_from(n), i64::try_from(d)) {
            (Ok(num), Ok(den)) => Ok(Rational { num, den }),
            _ => Err(overflow()),
        }
    }

    pub const fn from_int(n: i64) -> Rational {
        Rational { num: n, den: 1 }
    }

    pub const fn zero() -> Rational {
        Rational::from_int(0)
    }

    pub const fn num(&self) -> i64 {
        self.num
    }

    pub const fn den(&self) -> i64 {
        self.den
    }

    pub fn is_zero(&self) -> bool {
        self.num == 0
    }

    pub fn checked_add(self, rhs: Rational) -> Result<Rational, CoreError> {
        let (a, b, c, d) = (self.num as i128, self.den as i128, rhs.num as i128, rhs.den as i128);
        Self::from_i128(a * d + c * b, b * d)
    }

    pub fn checked_sub(self, rhs: Rational) -> Result<Rational, CoreError> {
        self.checked_add(rhs.checked_neg()?)
    }

    pub fn checked_neg(self) -> Result<Rational, CoreError> {
        Self::from_i128(-(self.num as i128), self.den as i128)
    }
}

impl std::ops::Add for Rational {
    type Output = Result<Rational, CoreError>;
    fn add(self, rhs: Rational) -> Self::Output {
        self.checked_add(rhs)
    }
}

impl std::ops::Sub for Rational {
    type Output = Result<Rational, CoreError>;
    fn sub(self, rhs: Rational) -> Self::Output {
        self.checked_sub(rhs)
    }
}

impl std::ops::Neg for Rational {
    type Output = Result<Rational, CoreError>;
    fn neg(self) -> Self::Output {
        self.checked_neg()
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.num as i128 * other.den as i128).cmp(&(other.num as i128 * self.den as i128))
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.den == 1 { write!(f, "{}", self.num) } else { write!(f, "{}/{}", self.num, self.den) }
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

impl FromStr for Rational {
    type Err = CoreError;
    /// Accepts `n` or `n/d` (integers only; floats are rejected).
    fn from_str(s: &str) -> Result<Self, CoreError> {
        let (n, d) = match s.split_once('/') {
            Some((n, d)) => (n, Some(d)),
            None => (s, None),
        };
        let digits = n.strip_prefix('-').unwrap_or(n);
        if !all_digits(digits) || d.is_some_and(|d| !all_digits(d)) {
            return Err(invalid_time(s, "expected an integer or n/d"));
        }
        let num = n.parse::<i128>().map_err(|_| overflow())?;
        let den = d.map_or(Ok(1), |d| d.parse::<i128>().map_err(|_| overflow()))?;
        if den == 0 {
            return Err(invalid_time(s, "denominator is zero"));
        }
        Rational::from_i128(num, den)
    }
}

impl Serialize for Rational {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Rational {
    fn schema_name() -> Cow<'static, str> {
        "Rational".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": r"^-?\d+(/[1-9]\d*)?$",
            "description": "Exact rational seconds: \"n/d\" in lowest terms, or \"n\" when d is 1."
        })
    }
}

/// One frame at `rate` frames per second (`1/rate`).
pub fn frame_duration(rate: Rational) -> Rational {
    // rate.num may be zero or negative for malformed rates; callers validate rates.
    Rational::from_i128(rate.den as i128, rate.num as i128).unwrap_or(Rational::zero())
}

/// Parses `12.4s`, `12s`, `-0.5s`, `00:12.4`, `01:00:12.4`, `f300` (frames at `rate`),
/// or a bare rational/integer (`62/5`, `12`; read as seconds). Decimals are exact.
pub fn parse_time(input: &str, rate: Rational) -> Result<Rational, CoreError> {
    let bad = |why: &str| invalid_time(input, why);
    if let Some(frames) = input.strip_prefix('f') {
        let digits = frames.strip_prefix('-').unwrap_or(frames);
        if !all_digits(digits) {
            return Err(bad("expected f<frames>"));
        }
        if rate.num <= 0 {
            return Err(bad("frame times need a positive rate"));
        }
        let n: i128 = frames.parse().map_err(|_| overflow())?;
        return Rational::from_i128(n * rate.den as i128, rate.num as i128);
    }
    if let Some(sec) = input.strip_suffix('s') {
        return parse_decimal(sec).ok_or_else(|| bad("expected decimal seconds like 12.4s"))?;
    }
    if input.contains(':') {
        return parse_clock(input).ok_or_else(|| bad("expected MM:SS[.f] or HH:MM:SS[.f]"))?;
    }
    if input.contains('/') || all_digits(input.strip_prefix('-').unwrap_or(input)) {
        return input.parse().map_err(|e: CoreError| if e.code == "INVALID_TIME" { bad("invalid rational") } else { e });
    }
    Err(bad("a bare number needs a unit (s) or a rational form"))
}

/// `-?\d+(\.\d+)?` as an exact rational. `None` when the shape is wrong.
fn parse_decimal(s: &str) -> Option<Result<Rational, CoreError>> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = body.split_once('.').map_or((body, ""), |(i, f)| (i, f));
    if !all_digits(int) || (body.contains('.') && !all_digits(frac)) {
        return None;
    }
    if frac.len() > 30 || int.len() > 30 {
        return Some(Err(overflow()));
    }
    let scale = 10i128.pow(frac.len() as u32);
    let digits: i128 = format!("{int}{frac}").parse().ok()?;
    let n = if neg { -digits } else { digits };
    Some(Rational::from_i128(n, scale))
}

/// `[HH:]MM:SS[.f]`, optionally negative.
fn parse_clock(s: &str) -> Option<Result<Rational, CoreError>> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let parts: Vec<&str> = body.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let (secs, head) = parts.split_last()?;
    if head.iter().any(|p| !all_digits(p)) || secs.starts_with('-') {
        return None;
    }
    let mut total: i128 = 0;
    for p in head {
        total = total.checked_mul(60)?.checked_add(p.parse::<i128>().ok()?)?;
    }
    let sec = match parse_decimal(secs)? {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };
    let whole = Rational::from_i128(total.checked_mul(60)?, 1);
    let sum = whole.and_then(|w| w.checked_add(sec));
    Some(sum.and_then(|r| if neg { r.checked_neg() } else { Ok(r) }))
}
