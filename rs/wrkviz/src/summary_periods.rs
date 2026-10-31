//! Calendar rollups at parity with `py/wrkviz/periods.py`.

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;

/// Rollup kinds in canonical order.
pub const ROLLUP_KINDS: [&str; 5] = ["hourly", "daily", "weekly", "monthly", "quarterly"];
/// Default rollup kinds.
pub const DEFAULT_ROLLUP_KINDS: [&str; 4] = ["daily", "weekly", "monthly", "quarterly"];

/// One timezone-aware calendar interval.
#[derive(Clone, Debug, PartialEq)]
pub struct Period {
    /// Kind.
    pub kind: String,
    /// Key.
    pub key: String,
    /// Label.
    pub label: String,
    /// Start ms.
    pub start_ms: i64,
    /// End ms.
    pub end_ms: i64,
    /// Relative path.
    pub relative_path: String,
    /// Partial flag.
    pub partial: bool,
}

fn parse_zone(name: &str) -> Result<Tz, String> {
    name.parse::<Tz>()
        .map_err(|_| format!("unknown display timezone {}", crate::json::py_repr_str(name)))
}

fn midnight(day: NaiveDate, zone: Tz) -> i64 {
    use chrono::Offset as _;
    let naive = day.and_hms_opt(0, 0, 0).unwrap();
    match zone.from_local_datetime(&naive) {
        chrono::LocalResult::Single(item) => item.timestamp_millis(),
        // Python ZoneInfo fold=0: the earlier instant.
        chrono::LocalResult::Ambiguous(earlier, _) => earlier.timestamp_millis(),
        // Midnight fell in a spring-forward gap: fold=0 applies the
        // offset in effect before the transition.
        chrono::LocalResult::None => {
            let guess = naive.and_utc() - chrono::Duration::hours(24);
            let offset = guess.with_timezone(&zone).offset().fix();
            naive.and_utc().timestamp_millis() - offset.local_minus_utc() as i64 * 1000
        }
    }
}

fn day_of(ms: i64, zone: Tz) -> NaiveDate {
    zone.timestamp_millis_opt(ms)
        .single()
        .map(|item| item.date_naive())
        .unwrap_or_else(|| {
            chrono::Utc
                .timestamp_millis_opt(ms)
                .single()
                .unwrap()
                .date_naive()
        })
}

fn iso_week(day: NaiveDate) -> (i32, u32) {
    let iso = day.iso_week();
    (iso.year(), iso.week())
}

fn daily(day: NaiveDate, zone: Tz, team: &str, last_ms: i64) -> Period {
    let start = midnight(day, zone);
    let end = midnight(day.succ_opt().unwrap(), zone);
    let (week_year, week_number) = iso_week(day);
    let key = day.format("%Y-%m-%d").to_string();
    Period {
        kind: "daily".into(),
        key: key.clone(),
        label: format!(
            "{} {} {}",
            day.format("%a"),
            day.format("%b"),
            day.day()
        ),
        start_ms: start,
        end_ms: end,
        relative_path: format!(
            "teams/{team}/summaries/daily/{week_year}-W{week_number:02}/{key}-{team}-daily.md"
        ),
        partial: last_ms + 1 < end,
    }
}

fn hourly(start_ms: i64, zone: Tz, team: &str, first_ms: i64, last_ms: i64) -> Period {
    let end_ms = start_ms + 3_600_000;
    let utc = chrono::Utc.timestamp_millis_opt(start_ms).single().unwrap();
    let local = utc.with_timezone(&zone);
    let key = utc.format("%Y-%m-%dT%HZ").to_string();
    let offset = local.format("%z").to_string();
    let offset_label = format!("UTC{}:{}", &offset[..3], &offset[3..]);
    Period {
        kind: "hourly".into(),
        key: key.clone(),
        label: format!(
            "{} {} {} · {:02}:00 {offset_label}",
            local.format("%a"),
            local.format("%b"),
            local.day(),
            local.hour()
        ),
        start_ms,
        end_ms,
        relative_path: format!(
            "teams/{team}/summaries/hourly/{}/{key}-{team}-hourly.md",
            utc.format("%Y-%m-%d")
        ),
        partial: first_ms > start_ms || last_ms + 1 < end_ms,
    }
}

fn weekly(day: NaiveDate, zone: Tz, team: &str, last_ms: i64) -> Period {
    let monday = day - chrono::Duration::days(day.weekday().num_days_from_monday() as i64);
    let end_day = monday + chrono::Duration::days(7);
    let (year, number) = iso_week(monday);
    let start = midnight(monday, zone);
    let end = midnight(end_day, zone);
    let key = format!("{year}-W{number:02}");
    Period {
        kind: "weekly".into(),
        key: key.clone(),
        label: format!(
            "Week {number} · {} {}",
            monday.format("%b"),
            monday.day()
        ),
        start_ms: start,
        end_ms: end,
        relative_path: format!("teams/{team}/summaries/weekly/{year}/{key}-{team}-weekly.md"),
        partial: last_ms + 1 < end,
    }
}

fn monthly(day: NaiveDate, zone: Tz, team: &str, last_ms: i64) -> Period {
    let first = day.with_day(1).unwrap();
    let next = if first.month() == 12 {
        NaiveDate::from_ymd_opt(first.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1).unwrap()
    };
    let start = midnight(first, zone);
    let end = midnight(next, zone);
    let key = first.format("%Y-%m").to_string();
    Period {
        kind: "monthly".into(),
        key: key.clone(),
        label: first.format("%B %Y").to_string(),
        start_ms: start,
        end_ms: end,
        relative_path: format!(
            "teams/{team}/summaries/monthly/{}/{key}-{team}-monthly.md",
            first.year()
        ),
        partial: last_ms + 1 < end,
    }
}

fn quarterly(day: NaiveDate, zone: Tz, team: &str, last_ms: i64) -> Period {
    let quarter = (day.month() - 1) / 3 + 1;
    let first_month = 3 * (quarter - 1) + 1;
    let first = NaiveDate::from_ymd_opt(day.year(), first_month, 1).unwrap();
    let next = if quarter == 4 {
        NaiveDate::from_ymd_opt(day.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(day.year(), first_month + 3, 1).unwrap()
    };
    let start = midnight(first, zone);
    let end = midnight(next, zone);
    let key = format!("{}-Q{quarter}", day.year());
    Period {
        kind: "quarterly".into(),
        key: key.clone(),
        label: format!("Q{quarter} {}", day.year()),
        start_ms: start,
        end_ms: end,
        relative_path: format!(
            "teams/{team}/summaries/quarterly/{}/{key}-{team}-quarterly.md",
            day.year()
        ),
        partial: last_ms + 1 < end,
    }
}

/// Each requested calendar interval touched by the inclusive range.
pub fn periods_for_range(
    first_ms: i64,
    last_ms: i64,
    display_timezone: &str,
    team_slug: &str,
    kinds: &[String],
) -> Result<Vec<Period>, String> {
    if last_ms < first_ms {
        return Err("timeline range ends before it starts".to_string());
    }
    if kinds.is_empty() || {
        let mut sorted = kinds.to_vec();
        sorted.sort();
        sorted.dedup();
        sorted.len() != kinds.len()
    } {
        return Err("rollup kinds must be a non-empty unique sequence".to_string());
    }
    let mut unsupported: Vec<&str> = kinds
        .iter()
        .map(String::as_str)
        .filter(|kind| !ROLLUP_KINDS.contains(kind))
        .collect();
    if !unsupported.is_empty() {
        unsupported.sort_unstable();
        return Err(format!(
            "unsupported rollup kind(s): {}",
            unsupported.join(", ")
        ));
    }
    let zone = parse_zone(display_timezone)?;
    let first_day = day_of(first_ms, zone);
    let last_day = day_of(last_ms, zone);
    let mut days = Vec::new();
    let mut cursor = first_day;
    while cursor <= last_day {
        days.push(cursor);
        cursor = cursor.succ_opt().unwrap();
    }
    let mut result: Vec<Period> = Vec::new();
    if kinds.iter().any(|kind| kind == "hourly") {
        let hour_ms = 3_600_000i64;
        let mut cursor_ms = (first_ms.div_euclid(hour_ms)) * hour_ms;
        while cursor_ms <= last_ms {
            result.push(hourly(cursor_ms, zone, team_slug, first_ms, last_ms));
            cursor_ms += hour_ms;
        }
    }
    if kinds.iter().any(|kind| kind == "daily") {
        for day in &days {
            result.push(daily(*day, zone, team_slug, last_ms));
        }
    }
    if kinds.iter().any(|kind| kind == "weekly") {
        let mut mondays: Vec<NaiveDate> = days
            .iter()
            .map(|day| *day - chrono::Duration::days(day.weekday().num_days_from_monday() as i64))
            .collect();
        mondays.sort_unstable();
        mondays.dedup();
        for day in mondays {
            result.push(weekly(day, zone, team_slug, last_ms));
        }
    }
    if kinds.iter().any(|kind| kind == "monthly") {
        let mut months: Vec<NaiveDate> = days.iter().map(|day| day.with_day(1).unwrap()).collect();
        months.sort_unstable();
        months.dedup();
        for day in months {
            result.push(monthly(day, zone, team_slug, last_ms));
        }
    }
    if kinds.iter().any(|kind| kind == "quarterly") {
        let mut quarters: Vec<(i32, u32)> = days
            .iter()
            .map(|day| (day.year(), (day.month() - 1) / 3 + 1))
            .collect();
        quarters.sort_unstable();
        quarters.dedup();
        for (year, quarter) in quarters {
            let day = NaiveDate::from_ymd_opt(year, 3 * (quarter - 1) + 1, 1).unwrap();
            result.push(quarterly(day, zone, team_slug, last_ms));
        }
    }
    let order = |kind: &str| ROLLUP_KINDS.iter().position(|item| *item == kind).unwrap_or(9);
    result.sort_by(|a, b| {
        a.start_ms
            .cmp(&b.start_ms)
            .then(order(&a.kind).cmp(&order(&b.kind)))
            .then(a.key.cmp(&b.key))
    });
    Ok(result)
}

/// Human-readable period heading.
pub fn period_heading(period: &Period, display_timezone: &str) -> String {
    let Ok(zone) = parse_zone(display_timezone) else {
        return period.label.clone();
    };
    let start = zone
        .timestamp_millis_opt(period.start_ms)
        .single()
        .unwrap();
    let end = zone.timestamp_millis_opt(period.end_ms).single().unwrap();
    let end_inclusive = end - chrono::Duration::milliseconds(1);
    let suffix = if period.partial { " (partial)" } else { "" };
    match period.kind.as_str() {
        "hourly" => format!("{}{suffix}", start.format("%Y-%m-%d %a %H:%M %Z")),
        "daily" => format!("{} {}{suffix}", start.format("%Y-%m-%d"), start.format("%a")),
        "weekly" => format!(
            "{} · {} to {}{suffix}",
            period.key,
            start.format("%Y-%m-%d"),
            end_inclusive.format("%Y-%m-%d")
        ),
        _ => format!("{}{suffix}", period.label),
    }
}
