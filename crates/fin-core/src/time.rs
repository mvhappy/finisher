//! Exact rational time. Timeline positions are seconds as `Rational`; never floats.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

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
    CoreError::new(
        "INVALID_TIME",
        format!("cannot parse time {input:?}: {why}"),
    )
    .with_details(json!({ "input": input }))
    .with_fix("use 12.4s, 00:12.4, 01:00:12.4, f300 or a rational like 62/5")
}

pub(crate) fn overflow() -> CoreError {
    CoreError::new("TIME_OVERFLOW", "rational time overflowed 64 bits")
        .with_fix("use smaller times, or a frame rate with a smaller denominator")
}

/// Names the input that overflowed, so a parse error points at what to change.
fn overflow_at(input: &str, e: CoreError) -> CoreError {
    if e.code != "TIME_OVERFLOW" {
        return e;
    }
    CoreError::new(
        "TIME_OVERFLOW",
        format!("time {input:?} does not fit a 64-bit rational"),
    )
    .with_details(json!({ "input": input }))
    .with_fix("use a smaller time value")
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

    pub fn is_positive(&self) -> bool {
        self.num > 0
    }

    pub fn is_negative(&self) -> bool {
        self.num < 0
    }

    /// `num as f64 / den as f64`: each part is converted, then divided, every step IEEE-754
    /// round-to-nearest, so the result is the same on every platform. It is the nearest `f64`
    /// to `num / den` while both parts fit in 2^53; beyond, a part is rounded before the
    /// division and the result may be one rounding further off. For evaluation only: positions
    /// stay rational.
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    pub fn is_zero(&self) -> bool {
        self.num == 0
    }

    pub fn checked_add(self, rhs: Rational) -> Result<Rational, CoreError> {
        let (a, b, c, d) = (
            self.num as i128,
            self.den as i128,
            rhs.num as i128,
            rhs.den as i128,
        );
        Self::from_i128(a * d + c * b, b * d)
    }

    pub fn checked_sub(self, rhs: Rational) -> Result<Rational, CoreError> {
        self.checked_add(rhs.checked_neg()?)
    }

    pub fn checked_neg(self) -> Result<Rational, CoreError> {
        Self::from_i128(-(self.num as i128), self.den as i128)
    }

    /// `self × rhs` (a time scaled by a speed, plan 4C Decision 14).
    pub fn checked_mul(self, rhs: Rational) -> Result<Rational, CoreError> {
        Self::from_i128(
            self.num as i128 * rhs.num as i128,
            self.den as i128 * rhs.den as i128,
        )
    }

    /// `self ÷ rhs`; `INVALID_TIME` when `rhs` is zero.
    pub fn checked_div(self, rhs: Rational) -> Result<Rational, CoreError> {
        Self::from_i128(
            self.num as i128 * rhs.den as i128,
            self.den as i128 * rhs.num as i128,
        )
    }
}

/// Zero.
impl Default for Rational {
    fn default() -> Self {
        Rational::zero()
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
        if self.den == 1 {
            write!(f, "{}", self.num)
        } else {
            write!(f, "{}/{}", self.num, self.den)
        }
    }
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

impl FromStr for Rational {
    type Err = CoreError;
    /// Accepts only the canonical form that `Display` writes: `n`, or `n/d` in lowest terms
    /// with `d > 1`; no sign but a leading `-` on a non-zero numerator, no leading zeros
    /// (floats are rejected). Anything else that reads as a rational (`2/4`, `-0`, `+1`,
    /// `01`, `0/5`, `3/1`) is `INVALID_TIME` naming the canonical form (`details.canonical`):
    /// one value has one spelling, so stored and hashed data compare byte for byte.
    fn from_str(s: &str) -> Result<Self, CoreError> {
        parse_rational(s).map_err(|e| overflow_at(s, e))
    }
}

/// `INVALID_TIME` for a rational written in another form than its canonical one.
fn noncanonical(input: &str, canonical: Rational) -> CoreError {
    CoreError::new(
        "INVALID_TIME",
        format!("time {input:?} is not in canonical form (\"{canonical}\")"),
    )
    .with_details(json!({ "input": input, "canonical": canonical.to_string() }))
    .with_fix(format!(
        "write it as \"{canonical}\" (lowest terms, no sign but a leading -, no leading zeros)"
    ))
}

fn parse_rational(s: &str) -> Result<Rational, CoreError> {
    let unsigned = s.strip_prefix('+').filter(|rest| !rest.starts_with('-'));
    let r = parse_rational_lenient(unsigned.unwrap_or(s))?;
    if r.to_string() != s {
        return Err(noncanonical(s, r));
    }
    Ok(r)
}

fn parse_rational_lenient(s: &str) -> Result<Rational, CoreError> {
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

impl Serialize for Rational {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// Decoding is lenient: a rational in another form than its canonical one (`"30/1"`, `"2/4"`,
/// `"+1"`) is read and normalised, so stored data a person edited by hand (a project file's
/// genesis rate) stays readable. Input from users and clients is strict: `FromStr` for typed
/// values, [`check_canonical_times`] for decoded documents (an RPC `tx.commit`).
impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        parse_lenient(&s).map_err(serde::de::Error::custom)
    }
}

/// Any rational form `n` or `n/d` (a leading `+` too), reduced.
fn parse_lenient(s: &str) -> Result<Rational, CoreError> {
    let unsigned = s.strip_prefix('+').filter(|rest| !rest.starts_with('-'));
    parse_rational_lenient(unsigned.unwrap_or(s)).map_err(|e| overflow_at(s, e))
}

/// `INVALID_TIME` (with `details {input, canonical, path}` and a fix) for the first string in
/// `input` (a client's JSON) that `decoded` (the same document decoded and encoded again)
/// holds in another, canonical, rational form: the strict check for documents, since decoding
/// itself normalises. `path` is a JSON pointer into `input`. Fields only one side has are
/// skipped.
pub fn check_canonical_times(input: &Value, decoded: &Value) -> Result<(), CoreError> {
    walk(input, decoded, &mut String::new())
}

fn walk(input: &Value, decoded: &Value, path: &mut String) -> Result<(), CoreError> {
    let len = path.len();
    let at = |path: &mut String, key: &str, a: &Value, b: &Value| {
        path.push('/');
        path.push_str(&key.replace('~', "~0").replace('/', "~1"));
        let r = walk(a, b, path);
        path.truncate(len);
        r
    };
    match (input, decoded) {
        (Value::String(x), Value::String(y)) if x != y => {
            if let (Ok(canonical), Ok(read)) = (y.parse::<Rational>(), parse_lenient(x))
                && canonical == read
            {
                let mut e = noncanonical(x, canonical);
                e.details["path"] = Value::String(path.clone());
                return Err(e);
            }
            Ok(())
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, va) in a {
                if let Some(vb) = b.get(k) {
                    at(path, k, va, vb)?;
                }
            }
            Ok(())
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (va, vb)) in a.iter().zip(b).enumerate() {
                at(path, &i.to_string(), va, vb)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

impl JsonSchema for Rational {
    fn schema_name() -> Cow<'static, str> {
        "Rational".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": r"^(0|-?[1-9]\d*)(/[1-9]\d*)?$",
            "description": "Exact rational seconds in canonical form: \"n/d\" in lowest terms with d > 1, or \"n\" when d is 1; no leading zeros, no \"+\", no \"-0\"."
        })
    }
}

/// One frame at `rate` frames per second (`1/rate`). Rejects a rate <= 0 with `INVALID_RANGE`.
pub fn frame_duration(rate: Rational) -> Result<Rational, CoreError> {
    if !rate.is_positive() {
        return Err(
            CoreError::new("INVALID_RANGE", "frame rate must be positive")
                .with_details(json!({ "rate": rate.to_string() }))
                .with_fix("use a rate greater than zero, e.g. 30 or 30000/1001"),
        );
    }
    Rational::from_i128(rate.den as i128, rate.num as i128)
}

/// Parses `12.4s`, `12s`, `-0.5s`, `00:12.4`, `01:00:12.4`, `f300` (frames at `rate`),
/// or a bare rational/integer (`62/5`, `12`; read as seconds). Decimals are exact.
pub fn parse_time(input: &str, rate: Rational) -> Result<Rational, CoreError> {
    parse_time_inner(input, rate).map_err(|e| overflow_at(input, e))
}

fn parse_time_inner(input: &str, rate: Rational) -> Result<Rational, CoreError> {
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
        return parse_clock(input);
    }
    let unsigned = input
        .strip_prefix('-')
        .or_else(|| input.strip_prefix('+'))
        .unwrap_or(input);
    if input.contains('/') || all_digits(unsigned) {
        return input.parse().map_err(|e: CoreError| {
            if e.code == "INVALID_TIME" && e.details.get("canonical").is_none() {
                bad("invalid rational")
            } else {
                e
            }
        });
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

/// `[HH:]MM:SS[.f]`, optionally negative. The leading field is unbounded (`90:00` is 90
/// minutes); minutes after hours and seconds must be below 60.
fn parse_clock(input: &str) -> Result<Rational, CoreError> {
    let bad = |why: &str| invalid_time(input, why);
    let shape = || bad("expected MM:SS[.f] or HH:MM:SS[.f]");
    let (neg, body) = match input.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, input),
    };
    let parts: Vec<&str> = body.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(shape());
    }
    let (secs, head) = parts.split_last().ok_or_else(shape)?;
    if head.iter().any(|p| !all_digits(p)) || secs.starts_with('-') {
        return Err(shape());
    }
    let mut total: i128 = 0;
    for (i, p) in head.iter().enumerate() {
        let field: i128 = p.parse().map_err(|_| overflow())?;
        if i > 0 && field >= 60 {
            return Err(bad("minutes must be below 60"));
        }
        total = total
            .checked_mul(60)
            .and_then(|t| t.checked_add(field))
            .ok_or_else(overflow)?;
    }
    let sec = parse_decimal(secs).ok_or_else(shape)??;
    if sec >= Rational::from_int(60) {
        return Err(bad("seconds must be below 60"));
    }
    let whole = Rational::from_i128(total.checked_mul(60).ok_or_else(overflow)?, 1)?;
    let sum = whole.checked_add(sec)?;
    if neg { sum.checked_neg() } else { Ok(sum) }
}
