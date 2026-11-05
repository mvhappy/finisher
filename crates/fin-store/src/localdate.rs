//! The local calendar date of a unix time, for daily backup names, without a time-zone
//! crate: `localtime_r` on unix (the C library reads `TZ` and the system zone files),
//! `SystemTimeToTzSpecificLocalTimeEx` with the current zone on Windows (that zone's rules
//! for the date converted, DST included). Falls back to UTC when the OS cannot say.
//!
//! Windows used `FileTimeToLocalFileTime` before, which applies the bias in force *now* to
//! any time: right for "today", but a time on the other side of a DST change (the backup's
//! "tomorrow" bound across one) came out an hour off.

/// `(year, month, day)` in local time of unix milliseconds `ms`.
pub(crate) fn local_ymd(ms: i64) -> (i64, i64, i64) {
    local(ms).unwrap_or_else(|| crate::backup::civil_from_days(ms.div_euclid(86_400_000)))
}

#[cfg(unix)]
unsafe extern "C" {
    fn tzset();
}

#[cfg(unix)]
fn local(ms: i64) -> Option<(i64, i64, i64)> {
    // POSIX does not require `localtime_r` to read `TZ`; `tzset` does (once is enough: a
    // zone change while running is picked up at the next start).
    static TZ: std::sync::Once = std::sync::Once::new();
    // SAFETY: `tzset` only initialises the C library's zone state.
    TZ.call_once(|| unsafe { tzset() });
    // `time_t` is 32 bits on some targets.
    #[allow(clippy::useless_conversion)]
    let t: libc::time_t = ms.div_euclid(1000).try_into().ok()?;
    // SAFETY: `localtime_r` writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::localtime_r(&t, &mut tm) };
    if r.is_null() {
        return None;
    }
    Some((
        i64::from(tm.tm_year) + 1900,
        i64::from(tm.tm_mon) + 1,
        i64::from(tm.tm_mday),
    ))
}

#[cfg(windows)]
fn local(ms: i64) -> Option<(i64, i64, i64)> {
    use windows_sys::Win32::System::Time::{
        DYNAMIC_TIME_ZONE_INFORMATION, GetDynamicTimeZoneInformation, TIME_ZONE_ID_INVALID,
    };
    let mut tz = DYNAMIC_TIME_ZONE_INFORMATION::default();
    // SAFETY: `tz` is a live, writable DYNAMIC_TIME_ZONE_INFORMATION.
    if unsafe { GetDynamicTimeZoneInformation(&mut tz) } == TIME_ZONE_ID_INVALID {
        return None;
    }
    local_in(ms, &tz)
}

/// The date of unix ms `ms` in zone `tz`, by that zone's rules for that date (also its
/// dynamic, per-year rules): `SystemTimeToTzSpecificLocalTimeEx`.
///
/// Not tested: with "Adjust for daylight saving time automatically" turned off, the current
/// zone comes with `DynamicDaylightTimeDisabled` set, and Windows then applies the standard
/// bias all year (documented), so the date follows the clock the user sees.
#[cfg(windows)]
fn local_in(
    ms: i64,
    tz: &windows_sys::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION,
) -> Option<(i64, i64, i64)> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{
        FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx,
    };
    // 100 ns intervals since 1601-01-01.
    const EPOCH_DIFF_MS: i64 = 11_644_473_600_000;
    let ticks = u64::try_from(ms.checked_add(EPOCH_DIFF_MS)?.checked_mul(10_000)?).ok()?;
    let ft = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: every pointer is to a live struct of the right type.
    let ok = unsafe {
        FileTimeToSystemTime(&ft, &mut utc) != 0
            && SystemTimeToTzSpecificLocalTimeEx(tz, &utc, &mut local) != 0
    };
    ok.then(|| {
        (
            i64::from(local.wYear),
            i64::from(local.wMonth),
            i64::from(local.wDay),
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn local(_ms: i64) -> Option<(i64, i64, i64)> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_local_date_is_within_a_day_of_utc() {
        let ms = 1_793_793_600_000; // 2026-11-04T12:00:00Z
        let (y, m, d) = super::local_ymd(ms);
        assert_eq!((y, m), (2026, 11));
        assert!((3..=5).contains(&d), "{d}");
    }

    /// Unix ms of `<date>T<hh>:<mm>:00Z`.
    #[cfg(windows)]
    fn utc(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
        // Days since 1970-01-01 (H. Hinnant's `days_from_civil`).
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
        let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
        assert_eq!(
            crate::backup::civil_from_days(days),
            (if m <= 2 { y + 1 } else { y }, m, d)
        );
        (days * 86_400 + hh * 3_600 + mm * 60) * 1_000
    }

    /// Windows zones by registry key name, 30 minutes either side of local midnight around
    /// the 2026 DST changes (EU: 29 March and 25 October, 01:00 UTC; US: 8 March and
    /// 1 November, 2:00 local), and one date under another year's rules (US 2006: DST from
    /// 2 April), without touching the system's zone.
    #[cfg(windows)]
    #[test]
    fn local_dates_follow_each_zones_rules_for_the_date() {
        use windows_sys::Win32::System::Time::{
            DYNAMIC_TIME_ZONE_INFORMATION, EnumDynamicTimeZoneInformation,
        };
        let zone = |key: &str| {
            (0..)
                .map_while(|i| {
                    let mut tz = DYNAMIC_TIME_ZONE_INFORMATION::default();
                    // SAFETY: `tz` is live and writable; a non-zero result ends the list.
                    (unsafe { EnumDynamicTimeZoneInformation(i, &mut tz) } == 0).then_some(tz)
                })
                .find(|tz| {
                    let n = tz
                        .TimeZoneKeyName
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(128);
                    String::from_utf16_lossy(&tz.TimeZoneKeyName[..n]) == key
                })
                .unwrap_or_else(|| panic!("no zone {key}"))
        };
        let cases: &[(&str, i64, (i64, i64, i64))] = &[
            // CET/CEST.
            (
                "W. Europe Standard Time",
                utc(2026, 3, 28, 23, 30),
                (2026, 3, 29),
            ),
            (
                "W. Europe Standard Time",
                utc(2026, 3, 29, 22, 30),
                (2026, 3, 30),
            ),
            (
                "W. Europe Standard Time",
                utc(2026, 3, 29, 21, 30),
                (2026, 3, 29),
            ),
            (
                "W. Europe Standard Time",
                utc(2026, 10, 24, 22, 30),
                (2026, 10, 25),
            ),
            (
                "W. Europe Standard Time",
                utc(2026, 10, 25, 22, 30),
                (2026, 10, 25),
            ),
            (
                "W. Europe Standard Time",
                utc(2026, 10, 25, 23, 30),
                (2026, 10, 26),
            ),
            // GMT/BST.
            ("GMT Standard Time", utc(2026, 3, 29, 23, 30), (2026, 3, 30)),
            (
                "GMT Standard Time",
                utc(2026, 10, 25, 23, 30),
                (2026, 10, 25),
            ),
            // EET/EEST.
            ("FLE Standard Time", utc(2026, 3, 29, 21, 30), (2026, 3, 30)),
            (
                "FLE Standard Time",
                utc(2026, 10, 25, 21, 30),
                (2026, 10, 25),
            ),
            // PST/PDT: the nights after the 2026 changes (each date differs under the other
            // bias), and 2006, when DST began on 2 April (under 2026 rules 20 March would be
            // PDT and 00:30 on the 20th).
            (
                "Pacific Standard Time",
                utc(2026, 3, 8, 7, 30),
                (2026, 3, 7),
            ),
            (
                "Pacific Standard Time",
                utc(2026, 3, 9, 7, 30),
                (2026, 3, 9),
            ),
            (
                "Pacific Standard Time",
                utc(2026, 11, 1, 7, 30),
                (2026, 11, 1),
            ),
            (
                "Pacific Standard Time",
                utc(2026, 11, 2, 7, 30),
                (2026, 11, 1),
            ),
            (
                "Pacific Standard Time",
                utc(2006, 3, 20, 7, 30),
                (2006, 3, 19),
            ),
            // No DST, far from UTC.
            (
                "Line Islands Standard Time",
                utc(2026, 3, 29, 10, 30),
                (2026, 3, 30),
            ),
            (
                "Hawaiian Standard Time",
                utc(2026, 3, 30, 9, 30),
                (2026, 3, 29),
            ),
        ];
        for (key, ms, want) in cases {
            assert_eq!(
                super::local_in(*ms, &zone(key)),
                Some(*want),
                "{key} at {ms}"
            );
        }
        // The system zone, where it is one of these (the test PC: W. Europe).
        let mut now = DYNAMIC_TIME_ZONE_INFORMATION::default();
        // SAFETY: `now` is live and writable.
        unsafe { windows_sys::Win32::System::Time::GetDynamicTimeZoneInformation(&mut now) };
        let n = now
            .TimeZoneKeyName
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(128);
        let key = String::from_utf16_lossy(&now.TimeZoneKeyName[..n]);
        for (_, ms, want) in cases.iter().filter(|c| c.0 == key) {
            assert_eq!(super::local_ymd(*ms), *want, "system zone {key} at {ms}");
        }
    }
}
