//! The harness-neutral events a transcript decodes to.

/// How a prompt reached the agent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Via {
    /// It started a turn.
    Typed,
    /// It arrived while a turn ran, and the harness folded it into that turn.
    Queued,
}

/// One thing a transcript records. Times are Unix seconds, when the line carries one.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A prompt the agent received: from its user, a tool, or another agent.
    Prompt {
        /// When it was recorded.
        at: Option<f64>,
        /// Its text.
        text: String,
        /// Whether it started a turn.
        via: Via,
    },
    /// Text the model wrote for its user, exactly as written: not its reasoning, not a tool
    /// call's arguments.
    Text {
        /// When it was recorded.
        at: Option<f64>,
        /// The text.
        text: String,
    },
    /// The model called a tool.
    ToolCall {
        /// When it was recorded.
        at: Option<f64>,
        /// The harness's id for the call.
        id: String,
        /// The tool's name.
        name: String,
    },
    /// A turn ended.
    TurnEnd {
        /// When it was recorded.
        at: Option<f64>,
    },
}

impl Event {
    /// When the event was recorded.
    #[must_use]
    pub fn at(&self) -> Option<f64> {
        match self {
            Self::Prompt { at, .. }
            | Self::Text { at, .. }
            | Self::ToolCall { at, .. }
            | Self::TurnEnd { at } => *at,
        }
    }
}

/// Unix seconds of an RFC 3339 UTC timestamp such as `2026-10-27T12:35:26.720Z` (an offset of
/// `+00:00` is accepted too).
#[must_use]
pub(crate) fn parse_utc(text: &str) -> Option<f64> {
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut clock = clock.splitn(3, ':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.parse().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0.0
    } else {
        format!("0.{fraction}").parse::<f64>().ok()?
    };
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) as f64 + fraction)
}

/// Days from 1970-01-01 to the proleptic Gregorian date `year-month-day`.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_parse_with_and_without_fractions() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_utc("2026-10-27T12:35:26.500Z"), Some(1_793_104_526.5));
        assert_eq!(
            parse_utc("2026-10-27T12:35:26+00:00"),
            Some(1_793_104_526.0)
        );
        assert_eq!(parse_utc("2000-02-29T00:00:00Z"), Some(951_782_400.0));
        assert_eq!(parse_utc("2026-10-27T12:35:26"), None, "no zone");
        assert_eq!(parse_utc("2026-13-27T12:35:26Z"), None);
    }

    #[test]
    fn every_event_reports_its_time() {
        let events = [
            Event::Prompt {
                at: Some(1.0),
                text: String::new(),
                via: Via::Typed,
            },
            Event::Text {
                at: Some(2.0),
                text: String::new(),
            },
            Event::ToolCall {
                at: Some(3.0),
                id: String::new(),
                name: String::new(),
            },
            Event::TurnEnd { at: None },
        ];
        let times: Vec<_> = events.iter().map(Event::at).collect();
        assert_eq!(times, [Some(1.0), Some(2.0), Some(3.0), None]);
    }
}
