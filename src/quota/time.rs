//! Time for quota data without a date crate: RFC 3339 and HTTP dates, epoch
//! numbers in seconds or milliseconds, short Russian durations and the local
//! wall-clock time of an instant.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(s: &str, from: usize, to: usize) -> Option<i64> {
    let part = s.get(from..to)?;
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

fn unix_from_parts(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> Option<i64> {
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    };
    let valid = (1..=12).contains(&month)
        && (1..=days).contains(&day)
        && (0..=9999).contains(&year)
        && (0..=23).contains(&hour)
        && (0..=59).contains(&minute)
        && (0..=60).contains(&second);
    valid.then(|| days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// `2026-10-05T14:20:00Z`, `2026-10-05T14:20:00.123+03:00`, `2026-10-05 14:20:00`
/// (no offset reads as UTC). Fractions of a second are dropped.
pub fn parse_rfc3339(text: &str) -> Option<i64> {
    let s = text.trim();
    let b = s.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let base = unix_from_parts(
        digits(s, 0, 4)?,
        digits(s, 5, 7)?,
        digits(s, 8, 10)?,
        digits(s, 11, 13)?,
        digits(s, 14, 16)?,
        digits(s, 17, 19)?,
    )?;
    let mut rest = &s[19..];
    if let Some(fraction) = rest.strip_prefix('.') {
        let end = fraction.bytes().position(|c| !c.is_ascii_digit()).unwrap_or(fraction.len());
        if end == 0 {
            return None;
        }
        rest = &fraction[end..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let raw = &rest[1..];
            if !matches!(raw.len(), 4 | 5) || (raw.len() == 5 && raw.as_bytes()[2] != b':') {
                return None;
            }
            let tz = raw.replace(':', "");
            if tz.len() != 4 {
                return None;
            }
            let hours = digits(&tz, 0, 2)?;
            let minutes = digits(&tz, 2, 4)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3_600 + minutes * 60)
        }
    };
    Some(base - offset)
}

/// IMF-fixdate, the only form `Retry-After` uses: `Wed, 21 Oct 2015 07:28:00 GMT`.
pub fn parse_http_date(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() != 6 || parts[5] != "GMT" {
        return None;
    }
    let day = digits(parts[1], 0, parts[1].len())?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| *m == parts[2])? as i64
        + 1;
    let year = digits(parts[3], 0, parts[3].len())?;
    let clock = parts[4];
    if clock.len() != 8 || clock.as_bytes()[2] != b':' || clock.as_bytes()[5] != b':' {
        return None;
    }
    unix_from_parts(year, month, day, digits(clock, 0, 2)?, digits(clock, 3, 5)?, digits(clock, 6, 8)?)
}

/// `Retry-After`: delay in seconds or an HTTP date. Returns the delay from `now`.
pub fn retry_after_secs(value: &str, now: i64) -> Option<i64> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<i64>() {
        return (secs >= 0).then_some(secs);
    }
    parse_http_date(value).map(|at| at.saturating_sub(now).max(0))
}

/// An instant given as a number (seconds, or milliseconds when above 10^12) or
/// as an RFC 3339 string. Values before 2001 are rejected as implausible.
pub fn epoch_from_json(value: &serde_json::Value) -> Option<i64> {
    let secs = match value {
        serde_json::Value::Number(n) => {
            let raw = n.as_f64()?;
            if !raw.is_finite() {
                return None;
            }
            if raw >= 1e12 {
                (raw / 1000.0) as i64
            } else {
                raw as i64
            }
        }
        serde_json::Value::String(s) => match s.trim().parse::<f64>() {
            Ok(raw) if raw.is_finite() => {
                if raw >= 1e12 {
                    (raw / 1000.0) as i64
                } else {
                    raw as i64
                }
            }
            _ => parse_rfc3339(s)?,
        },
        _ => return None,
    };
    (secs >= 1_000_000_000).then_some(secs)
}

/// `2ч5м`, `45м`, `3д4ч` — the same shape as the Claude status line.
pub fn format_left(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if days > 0 {
        crate::tr_format!("{days}d{hours}h", "{days}д{hours}ч")
    } else if hours > 0 {
        crate::tr_format!("{hours}h{minutes}m", "{hours}ч{minutes}м")
    } else {
        crate::tr_format!("{minutes}m", "{minutes}м")
    }
}

/// Local wall-clock `(day, month, hour, minute)` of a Unix instant, through the
/// system's time zone rules.
#[cfg(windows)]
pub fn local_parts(unix: i64) -> Option<(u16, u16, u16, u16)> {
    use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = u64::try_from(unix).ok()?.checked_add(11_644_473_600)?.checked_mul(10_000_000)?;
    let file_time = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    // SAFETY: both structs are plain data owned by this frame; the calls only
    // read the input and write the output struct.
    unsafe {
        let mut utc: SYSTEMTIME = std::mem::zeroed();
        if FileTimeToSystemTime(&file_time, &mut utc) == 0 {
            return None;
        }
        let mut local: SYSTEMTIME = std::mem::zeroed();
        if SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) == 0 {
            return None;
        }
        Some((local.wDay, local.wMonth, local.wHour, local.wMinute))
    }
}

/// `16:40` for an instant within the next 24 hours, `6.10 09:00` otherwise.
pub fn format_clock(unix: i64, now: i64) -> Option<String> {
    let (day, month, hour, minute) = local_parts(unix)?;
    Some(if unix.abs_diff(now) < 86_400 {
        format!("{hour:02}:{minute:02}")
    } else {
        format!("{day}.{month:02} {hour:02}:{minute:02}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn impossible_calendar_dates_and_offsets_are_not_normalized() {
        for bad in [
            "2026-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2026-04-31T00:00:00Z",
            "2026-01-01T00:00:00+24:00",
            "2026-01-01T00:00:00+00:60",
            "2026-01-01T00:00:00+0:100",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
        assert!(parse_rfc3339("2000-02-29T00:00:00Z").is_some());
        assert_eq!(parse_http_date("Wed, 31 Apr 2026 07:28:00 GMT"), None);
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07-28-00 GMT"), None);
    }

    #[test]
    fn rfc3339_forms() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("2026-10-05T14:20:00Z"), Some(1_791_210_000));
        assert_eq!(parse_rfc3339("2026-10-05T17:20:00+03:00"), Some(1_791_210_000));
        assert_eq!(parse_rfc3339("2026-10-05T14:20:00.123456z"), Some(1_791_210_000));
        assert_eq!(parse_rfc3339("2026-10-05 14:20:00"), Some(1_791_210_000));
        assert_eq!(parse_rfc3339("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        for bad in [
            "",
            "2026-10-05",
            "2026-13-05T00:00:00Z",
            "2026-10-05T24:00:00Z",
            "2026-10-05T14:20:00+3",
            "2026-10-05T14:20:00.Z",
            "x026-10-05T14:20:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn http_dates_and_retry_after() {
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT"), Some(1_445_412_480));
        assert_eq!(parse_http_date("Wed, 21 Oct 2015 07:28:00 UTC"), None);
        assert_eq!(retry_after_secs("120", 0), Some(120));
        assert_eq!(retry_after_secs("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_400), Some(80));
        assert_eq!(retry_after_secs("Wed, 21 Oct 2015 07:28:00 GMT", 1_445_412_500), Some(0));
        assert_eq!(retry_after_secs("-5", 0), None);
        assert_eq!(retry_after_secs("soon", 0), None);
    }

    #[test]
    fn epochs_from_json() {
        use serde_json::json;
        assert_eq!(epoch_from_json(&json!(1_791_210_000)), Some(1_791_210_000));
        assert_eq!(epoch_from_json(&json!(1_791_210_000_123_i64)), Some(1_791_210_000));
        assert_eq!(epoch_from_json(&json!("2026-10-05T14:20:00Z")), Some(1_791_210_000));
        assert_eq!(epoch_from_json(&json!("1791210000")), Some(1_791_210_000));
        assert_eq!(epoch_from_json(&json!(42)), None, "not a plausible instant");
        assert_eq!(epoch_from_json(&json!(null)), None);
        assert_eq!(epoch_from_json(&json!("tomorrow")), None);
    }

    #[test]
    fn short_durations() {
        assert_eq!(format_left(2 * 3_600 + 5 * 60 + 59), "2ч5м");
        assert_eq!(format_left(45 * 60), "45м");
        assert_eq!(format_left(3 * 86_400 + 4 * 3_600), "3д4ч");
        assert_eq!(format_left(-10), "0м");
    }

    #[test]
    fn local_clock_is_available() {
        let now = 1_791_210_000;
        let near = format_clock(now + 3_600, now).unwrap();
        assert_eq!(near.len(), 5, "{near}");
        let far = format_clock(now + 3 * 86_400, now).unwrap();
        assert!(far.contains('.') && far.ends_with(&near[2..]), "{far} vs {near}");
    }
}
