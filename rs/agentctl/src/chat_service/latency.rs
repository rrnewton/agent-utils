//! `chat latency`: how long each request took to reach each stage, from the bridge's own records
//! (#677 chat-bridge-repoint). Chat providers such as Slack record no time on a reaction, so the
//! bridge's request and reply records are the source: posted (the provider's message time),
//! admitted, acknowledged (the first reaction confirmed), delivered (the prompt verified typed, or
//! a headless model started it, when the second reaction is queued), and the first reply sent.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{json, Value};

use crate::chat_runtime::{self, BridgeState, RequestTimes};

use super::ChatServiceError;

/// `chat latency` settings.
#[derive(Clone, Copy, Debug)]
pub struct LatencyOptions {
    /// Only requests admitted at or after this Unix millisecond time.
    pub since_millis: Option<u64>,
    /// Requests the text listing shows, newest first.
    pub last: usize,
}

/// One stage interval: its name, and the request times it runs between.
struct Stage {
    name: &'static str,
    label: &'static str,
    interval: fn(&RequestTimes) -> Option<u64>,
}

fn between(from: Option<u64>, to: Option<u64>) -> Option<u64> {
    Some(to?.saturating_sub(from?))
}

const STAGES: [Stage; 5] = [
    Stage {
        name: "posted_to_admitted",
        label: "posted -> admitted",
        interval: |request| between(request.posted_at_millis, Some(request.admitted_at_millis)),
    },
    Stage {
        name: "admitted_to_acknowledged",
        label: "admitted -> acknowledged (1st reaction)",
        interval: |request| {
            between(
                Some(request.admitted_at_millis),
                request.acknowledged_at_millis,
            )
        },
    },
    Stage {
        name: "admitted_to_delivered",
        label: "admitted -> delivered (2nd reaction)",
        interval: |request| {
            between(
                Some(request.admitted_at_millis),
                request.delivered_at_millis,
            )
        },
    },
    Stage {
        name: "delivered_to_first_reply",
        label: "delivered -> first reply sent",
        interval: |request| {
            between(
                request.delivered_at_millis,
                request.first_reply_sent_at_millis,
            )
        },
    },
    Stage {
        name: "posted_to_first_reply",
        label: "posted -> first reply sent",
        interval: |request| {
            between(
                request
                    .posted_at_millis
                    .or(Some(request.admitted_at_millis)),
                request.first_reply_sent_at_millis,
            )
        },
    },
];

/// The nearest-rank `percentile` of `sorted`, which is in ascending order and not empty.
fn percentile(sorted: &[u64], percentile: u64) -> u64 {
    let count = u64::try_from(sorted.len()).unwrap_or(u64::MAX);
    let rank = (percentile.saturating_mul(count)).div_ceil(100).max(1);
    sorted[usize::try_from(rank - 1).unwrap_or(0).min(sorted.len() - 1)]
}

fn seconds(millis: Option<u64>) -> String {
    millis.map_or_else(
        || "-".to_owned(),
        |millis| format!("{:.1}", millis as f64 / 1_000.0),
    )
}

/// Summarize `values` in milliseconds: count, p50, p90, p99 and max, or only the count when empty.
fn summary(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    if values.is_empty() {
        return json!({"count": 0});
    }
    json!({
        "count": values.len(),
        "p50_millis": percentile(&values, 50),
        "p90_millis": percentile(&values, 90),
        "p99_millis": percentile(&values, 99),
        "max_millis": values[values.len() - 1],
    })
}

/// Build the `chat latency` report for the bridge state at `state_root`: one JSON document and
/// its text form.
pub fn latency(
    state_root: &Path,
    options: LatencyOptions,
) -> Result<(Value, String), ChatServiceError> {
    let state = BridgeState::inspect(state_root)?;
    let times: Vec<RequestTimes> = state
        .request_times()?
        .into_iter()
        .filter(|request| {
            options
                .since_millis
                .is_none_or(|since| request.admitted_at_millis >= since)
        })
        .collect();
    let mut stages = serde_json::Map::new();
    let mut text = format!(
        "chat latency: {} request(s){}, from the bridge's records (seconds; reactions carry no \
         provider time)\n",
        times.len(),
        options
            .since_millis
            .map(|since| format!(" admitted since {}", chat_runtime::utc_timestamp(since)))
            .unwrap_or_default()
    );
    let _ = writeln!(
        text,
        "{:<42} {:>5} {:>8} {:>8} {:>8} {:>8}",
        "stage", "n", "p50", "p90", "p99", "max"
    );
    for stage in &STAGES {
        let values: Vec<u64> = times.iter().filter_map(stage.interval).collect();
        let summarized = summary(values);
        let field = |name: &str| seconds(summarized[name].as_u64());
        let _ = writeln!(
            text,
            "{:<42} {:>5} {:>8} {:>8} {:>8} {:>8}",
            stage.label,
            summarized["count"].as_u64().unwrap_or(0),
            field("p50_millis"),
            field("p90_millis"),
            field("p99_millis"),
            field("max_millis")
        );
        stages.insert(stage.name.to_owned(), summarized);
    }
    if !times.is_empty() {
        let _ = writeln!(text, "\nnewest first:");
        let _ = writeln!(
            text,
            "{:<20} {:<12} {:>8} {:>8} {:>8} {:>8} {:>8}  {}",
            "admitted", "request", "intake", "ack", "deliver", "reply", "total", "phase"
        );
        for request in times.iter().rev().take(options.last) {
            let _ = writeln!(
                text,
                "{:<20} {:<12} {:>8} {:>8} {:>8} {:>8} {:>8}  {}",
                chat_runtime::utc_timestamp(request.admitted_at_millis),
                request.key.get(..12).unwrap_or(&request.key),
                seconds((STAGES[0].interval)(request)),
                seconds((STAGES[1].interval)(request)),
                seconds((STAGES[2].interval)(request)),
                seconds((STAGES[3].interval)(request)),
                seconds((STAGES[4].interval)(request)),
                request.phase
            );
        }
    }
    let requests: Vec<Value> = times
        .iter()
        .map(|request| {
            let mut row = json!(request);
            for stage in &STAGES {
                row[format!("{}_millis", stage.name)] = json!((stage.interval)(request));
            }
            row
        })
        .collect();
    let document = json!({
        "schema": "agentctl-chat-latency/v1",
        "since_millis": options.since_millis,
        "stages": stages,
        "requests": requests,
    });
    Ok((document, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile(&values, 50), 50);
        assert_eq!(percentile(&values, 90), 90);
        assert_eq!(percentile(&values, 99), 99);
        assert_eq!(percentile(&[7], 50), 7);
        assert_eq!(percentile(&[1, 2, 3], 99), 3);
        assert_eq!(summary(Vec::new()), json!({"count": 0}));
        assert_eq!(summary(vec![3, 1, 2])["max_millis"], 3);
    }

    fn request(posted: Option<u64>, delivered: Option<u64>, reply: Option<u64>) -> RequestTimes {
        RequestTimes {
            key: "k".repeat(64),
            thread_id: "spaces/example/threads/1".to_owned(),
            posted_at_millis: posted,
            admitted_at_millis: 1_000,
            acknowledged_at_millis: Some(1_500),
            delivered_at_millis: delivered,
            first_reply_sent_at_millis: reply,
            last_reply_sent_at_millis: reply,
            phase: "delivered",
            delivery_error: None,
            replies_captured: u32::from(reply.is_some()),
            replies_closed: false,
        }
    }

    #[test]
    fn stages_skip_what_a_request_has_not_reached() {
        let done = request(Some(400), Some(3_000), Some(9_000));
        let intervals: Vec<Option<u64>> =
            STAGES.iter().map(|stage| (stage.interval)(&done)).collect();
        assert_eq!(
            intervals,
            vec![Some(600), Some(500), Some(2_000), Some(6_000), Some(8_600)]
        );
        let waiting = request(None, None, None);
        let intervals: Vec<Option<u64>> = STAGES
            .iter()
            .map(|stage| (stage.interval)(&waiting))
            .collect();
        assert_eq!(intervals, vec![None, Some(500), None, None, None]);
        // Without a provider time, the total starts at admission.
        let unposted = request(None, Some(2_000), Some(5_000));
        assert_eq!((STAGES[4].interval)(&unposted), Some(4_000));
    }
}
