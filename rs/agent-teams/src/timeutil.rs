//! Small UTC time helpers: RFC 3339 formatting and parsing without a date crate.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix time in seconds.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Current Unix time in milliseconds.
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Civil date from days since the Unix epoch.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Format Unix seconds as `2026-10-24T13:00:00Z`.
pub fn format_rfc3339(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let secs = unix.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

/// Format Unix milliseconds as `2026-10-24T13:00:00.250Z`.
pub fn format_rfc3339_ms(unix_ms: i64) -> String {
    let base = format_rfc3339(unix_ms.div_euclid(1000));
    format!(
        "{}.{:03}Z",
        base.trim_end_matches('Z'),
        unix_ms.rem_euclid(1000)
    )
}

/// Parse an RFC 3339 UTC timestamp (`...Z`, fractional seconds allowed) to Unix milliseconds.
pub fn parse_rfc3339_ms(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.ends_with('Z') {
        return None;
    }
    let num = |from: usize, to: usize| -> Option<i64> { text.get(from..to)?.parse().ok() };
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return None;
    }
    let days = days_from_civil(num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let secs = days * 86400 + num(11, 13)? * 3600 + num(14, 16)? * 60 + num(17, 19)?;
    let mut ms = secs * 1000;
    if bytes.get(19) == Some(&b'.') {
        let frac = text.get(20..text.len() - 1)?;
        let mut digits = String::from(frac);
        while digits.len() < 3 {
            digits.push('0');
        }
        ms += digits.get(..3)?.parse::<i64>().ok()?;
    }
    Some(ms)
}

/// Parse a duration: bare seconds, or a number with `s`, `m`, `h` or `d`.
pub fn parse_duration_seconds(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (digits, mult) = match text.chars().last()? {
        's' => (&text[..text.len() - 1], 1),
        'm' => (&text[..text.len() - 1], 60),
        'h' => (&text[..text.len() - 1], 3600),
        'd' => (&text[..text.len() - 1], 86400),
        _ => (text, 1),
    };
    digits.parse::<i64>().ok().map(|n| n * mult)
}

/// Host boot ID from `/proc/sys/kernel/random/boot_id`, or `"unknown"`.
pub fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_round_trips() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        let ms = parse_rfc3339_ms("2026-10-24T13:00:00.250Z").expect("parse");
        assert_eq!(format_rfc3339_ms(ms), "2026-10-24T13:00:00.250Z");
        assert_eq!(parse_rfc3339_ms("2026-10-24T13:00:00Z"), Some(ms - 250));
        assert_eq!(parse_rfc3339_ms("nonsense"), None);
    }

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration_seconds("90"), Some(90));
        assert_eq!(parse_duration_seconds("15m"), Some(900));
        assert_eq!(parse_duration_seconds("2h"), Some(7200));
        assert_eq!(parse_duration_seconds("7d"), Some(604800));
        assert_eq!(parse_duration_seconds("x"), None);
    }
}
