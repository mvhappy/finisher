//! The local calendar date of a unix time, for daily backup names, without a time-zone
//! crate: `localtime_r` on unix (the C library reads `TZ` and the system zone files),
//! `FileTimeToLocalFileTime` on Windows (the current zone's bias, which is the right one for
//! "today"). Falls back to UTC when the OS cannot say.

/// `(year, month, day)` in local time of unix milliseconds `ms`.
pub(crate) fn local_ymd(ms: i64) -> (i64, i64, i64) {
    local(ms).unwrap_or_else(|| crate::backup::civil_from_days(ms.div_euclid(86_400_000)))
}

#[cfg(unix)]
fn local(ms: i64) -> Option<(i64, i64, i64)> {
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
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
    // 100 ns intervals since 1601-01-01.
    const EPOCH_DIFF_MS: i64 = 11_644_473_600_000;
    let ticks = u64::try_from(ms.checked_add(EPOCH_DIFF_MS)?.checked_mul(10_000)?).ok()?;
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: both pointers are to live FILETIMEs.
    if unsafe { FileTimeToLocalFileTime(&utc, &mut local) } == 0 {
        return None;
    }
    let t = (u64::from(local.dwHighDateTime) << 32) | u64::from(local.dwLowDateTime);
    let local_ms = i64::try_from(t / 10_000).ok()? - EPOCH_DIFF_MS;
    Some(crate::backup::civil_from_days(
        local_ms.div_euclid(86_400_000),
    ))
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
}
