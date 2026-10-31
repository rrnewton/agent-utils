//! Provider-neutral calendar-date and exact-time windows:
//! `py/wrkviz/window.py` at parity.

use chrono::{Datelike as _, NaiveDate, NaiveDateTime, Offset as _, TimeZone as _};
use serde_json::{Map, Value};

use crate::json::py_repr_str;
use crate::team_data::TeamData;

/// A half-open UTC interval derived from calendar dates or
/// exact instants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DateWindow {
    /// Start date (YYYY-MM-DD).
    pub start_date: Option<String>,
    /// End date (YYYY-MM-DD).
    pub end_date: Option<String>,
    /// Start instant in ms.
    pub start_ms: Option<i64>,
    /// End instant in ms.
    pub end_ms: Option<i64>,
    /// Canonical start instant.
    pub start_time: Option<String>,
    /// Canonical end instant.
    pub end_time: Option<String>,
}

impl DateWindow {
    /// The manifest metadata for this selection.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "start_date".into(),
            self.start_date.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "end_date".into(),
            self.end_date.clone().map_or(Value::Null, Value::String),
        );
        out.insert(
            "start_ms".into(),
            self.start_ms.map_or(Value::Null, Into::into),
        );
        out.insert("end_ms".into(), self.end_ms.map_or(Value::Null, Into::into));
        if self.start_time.is_some() || self.end_time.is_some() {
            out.insert(
                "start_time".into(),
                self.start_time.clone().map_or(Value::Null, Value::String),
            );
            out.insert(
                "end_time".into(),
                self.end_time.clone().map_or(Value::Null, Value::String),
            );
        }
        Value::Object(out)
    }

    /// Whether a point timestamp is inside this interval.
    pub fn contains(&self, timestamp_ms: i64) -> bool {
        if let Some(start) = self.start_ms {
            if timestamp_ms < start {
                return false;
            }
        }
        self.end_ms.is_none_or(|end| timestamp_ms < end)
    }

    /// Whether a non-empty lifetime intersects the interval.
    pub fn overlaps(&self, start_ms: i64, end_ms: Option<i64>) -> bool {
        let mut effective_end = end_ms.unwrap_or(start_ms + 1);
        if effective_end <= start_ms {
            effective_end = start_ms + 1;
        }
        if let Some(end) = self.end_ms {
            if start_ms >= end {
                return false;
            }
        }
        self.start_ms.is_none_or(|start| effective_end > start)
    }
}

/// CPython's `zoneinfo` refuses absolute and traversing keys
/// with a plain `ValueError` that ingest does not wrap (its
/// handler catches only `ZoneInfoNotFoundError`), so that
/// wording reaches the CLI unchanged.
pub fn tz_refusal(name: &str) -> Option<String> {
    if name.starts_with('/') || name.split('/').any(|part| part == "..") {
        return Some(format!(
            "ZoneInfo keys must refer to subdirectories of TZPATH, got: {name}"
        ));
    }
    None
}

fn parse_tz(name: &str) -> Result<chrono_tz::Tz, String> {
    if let Some(refusal) = tz_refusal(name) {
        return Err(refusal);
    }
    name.parse::<chrono_tz::Tz>()
        .map_err(|_| format!("unknown display timezone {}", py_repr_str(name)))
}

fn parse_date(raw: &str, label: &str) -> Result<NaiveDate, String> {
    let err = || format!("{label} must be a calendar date in YYYY-MM-DD form");
    let parsed = NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| err())?;
    if parsed.format("%Y-%m-%d").to_string() != raw || parsed.year() < 1 {
        return Err(err());
    }
    Ok(parsed)
}

fn midnight_ms(value: NaiveDate, timezone_name: &str) -> Result<i64, String> {
    let tz = parse_tz(timezone_name)?;
    let naive = value
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "invalid date".to_string())?;
    Ok(local_midnight_ms(tz, naive))
}

fn local_midnight_ms(tz: chrono_tz::Tz, naive: NaiveDateTime) -> i64 {
    use chrono::LocalResult;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt.timestamp_millis(),
        LocalResult::Ambiguous(first, _) => first.timestamp_millis(),
        LocalResult::None => {
            // A midnight inside a transition gap: CPython's
            // ZoneInfo reads it with the pre-transition offset
            // (fold=0). Approximate with the offset in effect
            // twelve hours earlier.
            let guess = naive - chrono::Duration::hours(12);
            let offset_seconds = i64::from(
                tz.from_utc_datetime(&guess)
                    .offset()
                    .fix()
                    .local_minus_utc(),
            );
            (naive.and_utc().timestamp() - offset_seconds) * 1000
        }
    }
}

fn parse_time(raw: &str, label: &str) -> Result<(String, i64), String> {
    let err = || format!("{label} must be an RFC3339 timestamp with an explicit offset or Z");
    // ^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,6})?(Z|[+-]\d{2}:\d{2})$
    let bytes = raw.as_bytes();
    let shape_ok = raw.len() >= 20
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes.get(10) == Some(&b'T')
        && bytes.get(13) == Some(&b':')
        && bytes.get(16) == Some(&b':')
        && [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .iter()
            .all(|i| bytes.get(*i).is_some_and(u8::is_ascii_digit));
    if !shape_ok {
        return Err(err());
    }
    let rest = &raw[19..];
    let (fraction_digits, zone) = if let Some(stripped) = rest.strip_prefix('.') {
        let digits: String = stripped.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 6 {
            return Err(err());
        }
        let zone = &stripped[digits.len()..];
        (digits, zone)
    } else {
        (String::new(), rest)
    };
    let zone_ok = zone == "Z"
        || (zone.len() == 6
            && (zone.starts_with('+') || zone.starts_with('-'))
            && zone.as_bytes().get(3) == Some(&b':')
            && zone
                .chars()
                .enumerate()
                .all(|(i, c)| i == 0 || i == 3 || c.is_ascii_digit()));
    if !zone_ok {
        return Err(err());
    }
    let normalized = if zone == "Z" {
        format!("{}+00:00", &raw[..raw.len() - 1])
    } else {
        raw.to_string()
    };
    let _ = fraction_digits;
    let parsed = chrono::DateTime::parse_from_rfc3339(&normalized).map_err(|_| err())?;
    // CPython: int(timestamp() * 1000) truncates toward zero.
    let timestamp_ms = ((parsed.timestamp_micros() as f64) / 1000.0).trunc() as i64;
    let utc = chrono::Utc
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .ok_or_else(err)?;
    let canonical = format!(
        "{}.{:03}Z",
        utc.format("%Y-%m-%dT%H:%M:%S"),
        utc.timestamp_subsec_millis()
    );
    Ok((canonical, timestamp_ms))
}

/// Parse an inclusive start and exclusive end as dates or
/// exact instants, in `timezone_name`.
pub fn parse_date_window(
    start_date: Option<&str>,
    end_date: Option<&str>,
    timezone_name: &str,
    start_time: Option<&str>,
    end_time: Option<&str>,
) -> Result<Option<DateWindow>, String> {
    parse_tz(timezone_name)?;
    if start_date.is_some() && start_time.is_some() {
        return Err("choose either start date or start time, not both".to_string());
    }
    if end_date.is_some() && end_time.is_some() {
        return Err("choose either end date or end time, not both".to_string());
    }
    if start_date.is_none() && end_date.is_none() && start_time.is_none() && end_time.is_none() {
        return Ok(None);
    }
    let parsed_start = start_date
        .map(|raw| parse_date(raw, "start date"))
        .transpose()?;
    let parsed_end = end_date
        .map(|raw| parse_date(raw, "end date"))
        .transpose()?;
    let (canonical_start_time, start_ms) = if let Some(raw) = start_time {
        let (canonical, ms) = parse_time(raw, "start time")?;
        (Some(canonical), Some(ms))
    } else if let Some(date) = parsed_start {
        (None, Some(midnight_ms(date, timezone_name)?))
    } else {
        (None, None)
    };
    let (canonical_end_time, end_ms) = if let Some(raw) = end_time {
        let (canonical, ms) = parse_time(raw, "end time")?;
        (Some(canonical), Some(ms))
    } else if let Some(date) = parsed_end {
        (None, Some(midnight_ms(date, timezone_name)?))
    } else {
        (None, None)
    };
    if let (Some(start), Some(end)) = (start_ms, end_ms) {
        if start >= end {
            return Err("start bound must be earlier than end bound".to_string());
        }
    }
    Ok(Some(DateWindow {
        start_date: start_date.map(str::to_string),
        end_date: end_date.map(str::to_string),
        start_ms,
        end_ms,
        start_time: canonical_start_time,
        end_time: canonical_end_time,
    }))
}

/// Attach a display/summarization window while retaining
/// earlier context.
pub fn apply_date_window(mut team: TeamData, window: Option<&DateWindow>) -> TeamData {
    match window {
        None => {
            team.window_start_ms = None;
            team.window_end_ms = None;
        }
        Some(window) => {
            team.window_start_ms = window.start_ms;
            team.window_end_ms = window.end_ms;
        }
    }
    team
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_window_parses_and_bounds() {
        let window = parse_date_window(
            Some("2026-10-26"),
            Some("2026-10-27"),
            "America/New_York",
            None,
            None,
        )
        .unwrap()
        .unwrap();
        // 2026-10-26 is EDT (UTC-4): midnight is 04:00Z.
        assert_eq!(window.start_ms, Some(1_792_987_200_000));
        assert!(window.contains(1_792_987_200_000));
        assert!(!window.contains(1_792_987_199_999));
        assert_eq!(
            parse_date_window(Some("2026-13-01"), None, "UTC", None, None).unwrap_err(),
            "start date must be a calendar date in YYYY-MM-DD form"
        );
        assert_eq!(
            parse_date_window(None, None, "Nowhere/Special", None, None).unwrap_err(),
            "unknown display timezone 'Nowhere/Special'"
        );
    }
}
