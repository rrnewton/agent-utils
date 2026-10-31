//! Token-free activity bins at parity with `py/wrkviz/activity_bins.py`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use chrono::{Datelike, TimeZone};
use chrono_tz::Tz;
use serde_json::Value;

use crate::summary_phases::PhaseWindow;

const HOUR_MS: i64 = 3_600_000;
const EVIDENCE_HALF_WINDOW_MS: i64 = 150_000;
const ROLES: [&str; 2] = ["coordinator", "workers"];
const RESOLUTIONS: [&str; 3] = ["hourly", "daily", "weekly"];

/// One non-empty aggregate activity interval for a team role.
#[derive(Clone, Debug)]
pub struct ActivityBin {
    /// Team slug.
    pub team: String,
    /// Role.
    pub role: String,
    /// Resolution.
    pub resolution: String,
    /// Start ms.
    pub start_ms: i64,
    /// End ms.
    pub end_ms: i64,
    /// Average active concurrency.
    pub avg_active_concurrency: f64,
    /// Peak concurrency.
    pub peak_concurrency: i64,
    /// Activity coverage fraction.
    pub activity_coverage_fraction: f64,
    /// Distinct active agents.
    pub distinct_active_agents: i64,
    /// Average present concurrency.
    pub avg_present_concurrency: f64,
    /// Peak present concurrency.
    pub peak_present_concurrency: i64,
    /// Activity evidence fraction.
    pub activity_evidence_fraction: f64,
    /// Activity evidence events.
    pub activity_evidence_events: i64,
    /// Timing quality.
    pub timing_quality: String,
}

fn round6(value: f64) -> f64 {
    (value * 1_000_000.0).round_ties_even() / 1_000_000.0
}

impl ActivityBin {
    /// Stable JSON representation.
    pub fn to_json_obj(&self) -> Value {
        let num = |value: f64| {
            serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
        };
        let mut out = serde_json::Map::new();
        out.insert("team".into(), Value::String(self.team.clone()));
        out.insert("role".into(), Value::String(self.role.clone()));
        out.insert(
            "resolution".into(),
            Value::String(self.resolution.clone()),
        );
        out.insert("start_ms".into(), self.start_ms.into());
        out.insert("end_ms".into(), self.end_ms.into());
        out.insert(
            "avg_active_concurrency".into(),
            num(self.avg_active_concurrency),
        );
        out.insert("peak_concurrency".into(), self.peak_concurrency.into());
        out.insert(
            "activity_coverage_fraction".into(),
            num(self.activity_coverage_fraction),
        );
        out.insert(
            "distinct_active_agents".into(),
            self.distinct_active_agents.into(),
        );
        out.insert(
            "avg_present_concurrency".into(),
            num(self.avg_present_concurrency),
        );
        out.insert(
            "peak_present_concurrency".into(),
            self.peak_present_concurrency.into(),
        );
        out.insert(
            "activity_evidence_fraction".into(),
            num(self.activity_evidence_fraction),
        );
        out.insert(
            "activity_evidence_events".into(),
            self.activity_evidence_events.into(),
        );
        out.insert(
            "timing_quality".into(),
            Value::String(self.timing_quality.clone()),
        );
        Value::Object(out)
    }
}

type Interval = (i64, i64);

fn merge_intervals(intervals: &[Interval]) -> Vec<Interval> {
    let mut ordered: Vec<Interval> = intervals
        .iter()
        .copied()
        .filter(|(start, end)| end > start)
        .collect();
    ordered.sort();
    let mut merged: Vec<Interval> = Vec::new();
    for (start, end) in ordered {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                last.1 = last.1.max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn aligned_start(timestamp_ms: i64, size_ms: i64, offset_ms: i64) -> i64 {
    ((timestamp_ms - offset_ms).div_euclid(size_ms)) * size_ms + offset_ms
}

fn parse_zone(name: &str) -> Result<Tz, String> {
    name.parse::<Tz>()
        .map_err(|_| format!("unknown display timezone {}", crate::json::py_repr_str(name)))
}

fn midnight_ms(day: chrono::NaiveDate, zone: Tz) -> i64 {
    use chrono::Offset as _;
    let naive = day.and_hms_opt(0, 0, 0).unwrap();
    match zone.from_local_datetime(&naive) {
        chrono::LocalResult::Single(item) => item.timestamp_millis(),
        chrono::LocalResult::Ambiguous(earlier, _) => earlier.timestamp_millis(),
        chrono::LocalResult::None => {
            let guess = naive.and_utc() - chrono::Duration::hours(24);
            let offset = guess.with_timezone(&zone).offset().fix();
            naive.and_utc().timestamp_millis() - offset.local_minus_utc() as i64 * 1000
        }
    }
}

fn bucket_bounds(timestamp_ms: i64, resolution: &str, zone: Tz) -> Interval {
    if resolution == "hourly" {
        let start = aligned_start(timestamp_ms, HOUR_MS, 0);
        return (start, start + HOUR_MS);
    }
    let local_day = zone
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|item| item.date_naive())
        .unwrap_or_else(|| {
            chrono::Utc
                .timestamp_millis_opt(timestamp_ms)
                .single()
                .unwrap()
                .date_naive()
        });
    let (first, next) = if resolution == "weekly" {
        let monday =
            local_day - chrono::Duration::days(local_day.weekday().num_days_from_monday() as i64);
        (monday, monday + chrono::Duration::days(7))
    } else {
        (local_day, local_day + chrono::Duration::days(1))
    };
    (midnight_ms(first, zone), midnight_ms(next, zone))
}

fn peak_concurrency(intervals_by_agent: &HashMap<String, Vec<Interval>>) -> i64 {
    let mut deltas: BTreeMap<i64, i64> = BTreeMap::new();
    for intervals in intervals_by_agent.values() {
        for (start, end) in intervals {
            *deltas.entry(*start).or_default() += 1;
            *deltas.entry(*end).or_default() -= 1;
        }
    }
    let mut current = 0i64;
    let mut peak = 0i64;
    for delta in deltas.values() {
        current += delta;
        peak = peak.max(current);
    }
    peak
}

type RoleMap = HashMap<String, HashMap<String, Vec<Interval>>>;
type BinKey = (String, i64, i64);

fn split_intervals(
    intervals_by_role: &RoleMap,
    resolution: &str,
    zone: Tz,
    observed_start_ms: i64,
    observed_end_ms: i64,
) -> HashMap<BinKey, HashMap<String, Vec<Interval>>> {
    let mut bins: HashMap<BinKey, HashMap<String, Vec<Interval>>> = HashMap::new();
    for role in ROLES {
        let Some(by_agent) = intervals_by_role.get(role) else {
            continue;
        };
        for (agent_id, intervals) in by_agent {
            for (start, end) in intervals {
                let mut cursor = (*start).max(observed_start_ms);
                let effective_end = (*end).min(observed_end_ms);
                while cursor < effective_end {
                    let (calendar_start, calendar_end) =
                        bucket_bounds(cursor, resolution, zone);
                    let bucket_start = calendar_start.max(observed_start_ms);
                    let bucket_end = calendar_end.min(observed_end_ms);
                    let clipped_start = (*start).max(bucket_start);
                    let clipped_end = (*end).min(bucket_end);
                    if clipped_end > clipped_start {
                        bins.entry((role.to_string(), bucket_start, bucket_end))
                            .or_default()
                            .entry(agent_id.clone())
                            .or_default()
                            .push((clipped_start, clipped_end));
                    }
                    cursor = calendar_end;
                }
            }
        }
    }
    bins
}

fn merged_agents(
    bins: &HashMap<BinKey, HashMap<String, Vec<Interval>>>,
    key: &BinKey,
) -> HashMap<String, Vec<Interval>> {
    bins.get(key)
        .map(|by_agent| {
            by_agent
                .iter()
                .map(|(agent, intervals)| (agent.clone(), merge_intervals(intervals)))
                .collect()
        })
        .unwrap_or_default()
}

fn total_agent_ms(intervals_by_agent: &HashMap<String, Vec<Interval>>) -> i64 {
    intervals_by_agent
        .values()
        .flat_map(|intervals| intervals.iter())
        .map(|(start, end)| end - start)
        .sum()
}

#[allow(clippy::too_many_arguments)]
fn build_resolution(
    team: &str,
    active_by_role: &RoleMap,
    present_by_role: &RoleMap,
    evidence_by_role: &RoleMap,
    evidence_events_by_role: &HashMap<String, Vec<i64>>,
    resolution: &str,
    zone: Tz,
    observed_start_ms: i64,
    observed_end_ms: i64,
) -> Vec<ActivityBin> {
    let active_bins = split_intervals(
        active_by_role,
        resolution,
        zone,
        observed_start_ms,
        observed_end_ms,
    );
    let present_bins = split_intervals(
        present_by_role,
        resolution,
        zone,
        observed_start_ms,
        observed_end_ms,
    );
    let evidence_bins = split_intervals(
        evidence_by_role,
        resolution,
        zone,
        observed_start_ms,
        observed_end_ms,
    );
    let mut evidence_counts: HashMap<BinKey, i64> = HashMap::new();
    for role in ROLES {
        for timestamp_ms in evidence_events_by_role.get(role).cloned().unwrap_or_default() {
            if timestamp_ms < observed_start_ms || timestamp_ms >= observed_end_ms {
                continue;
            }
            let (calendar_start, calendar_end) = bucket_bounds(timestamp_ms, resolution, zone);
            let key = (
                role.to_string(),
                calendar_start.max(observed_start_ms),
                calendar_end.min(observed_end_ms),
            );
            *evidence_counts.entry(key).or_default() += 1;
        }
    }
    let mut result = Vec::new();
    for role in ROLES {
        let mut bounds: BTreeSet<(i64, i64)> = BTreeSet::new();
        for key in active_bins
            .keys()
            .chain(present_bins.keys())
            .chain(evidence_bins.keys())
        {
            if key.0 == role {
                bounds.insert((key.1, key.2));
            }
        }
        for (start_ms, end_ms) in bounds {
            let key = (role.to_string(), start_ms, end_ms);
            let intervals_by_agent = merged_agents(&active_bins, &key);
            let present_by_agent = merged_agents(&present_bins, &key);
            let evidence_by_agent = merged_agents(&evidence_bins, &key);
            let active_agent_ms = total_agent_ms(&intervals_by_agent);
            let present_agent_ms = total_agent_ms(&present_by_agent);
            let all: Vec<Interval> = intervals_by_agent
                .values()
                .flat_map(|items| items.iter().copied())
                .collect();
            let covered_ms: i64 = merge_intervals(&all)
                .iter()
                .map(|(start, end)| end - start)
                .sum();
            let duration_ms = end_ms - start_ms;
            if duration_ms <= 0 {
                continue;
            }
            let evidence_all: Vec<Interval> = evidence_by_agent
                .values()
                .flat_map(|items| items.iter().copied())
                .collect();
            let evidence_covered_ms: i64 = merge_intervals(&evidence_all)
                .iter()
                .map(|(start, end)| end - start)
                .sum();
            result.push(ActivityBin {
                team: team.to_string(),
                role: role.to_string(),
                resolution: resolution.to_string(),
                start_ms,
                end_ms,
                avg_active_concurrency: round6(active_agent_ms as f64 / duration_ms as f64),
                peak_concurrency: peak_concurrency(&intervals_by_agent),
                activity_coverage_fraction: round6(covered_ms as f64 / duration_ms as f64),
                distinct_active_agents: intervals_by_agent.len() as i64,
                avg_present_concurrency: round6(present_agent_ms as f64 / duration_ms as f64),
                peak_present_concurrency: peak_concurrency(&present_by_agent),
                activity_evidence_fraction: round6(
                    evidence_covered_ms as f64 / duration_ms as f64,
                ),
                activity_evidence_events: *evidence_counts.get(&key).unwrap_or(&0),
                timing_quality: "inferred".to_string(),
            });
        }
    }
    result
}

/// Aggregate active/tool intervals inside the observed archive range.
pub fn build_activity_bins(
    team: &str,
    coordinator_agent_id: &str,
    phases: &[PhaseWindow],
    display_timezone: &str,
    observed_start_ms: i64,
    observed_end_ms: i64,
) -> Result<Vec<ActivityBin>, String> {
    if observed_end_ms <= observed_start_ms {
        return Err("observed activity range must have positive duration".to_string());
    }
    let zone = parse_zone(display_timezone)?;
    let mut raw_active: RoleMap = HashMap::new();
    let mut raw_present: RoleMap = HashMap::new();
    let mut raw_evidence: RoleMap = HashMap::new();
    let mut evidence_events: HashMap<String, Vec<i64>> = HashMap::new();
    for role in ROLES {
        raw_active.insert(role.to_string(), HashMap::new());
        raw_present.insert(role.to_string(), HashMap::new());
        raw_evidence.insert(role.to_string(), HashMap::new());
        evidence_events.insert(role.to_string(), Vec::new());
    }
    for phase in phases {
        let role = if phase.agent_id == coordinator_agent_id {
            "coordinator"
        } else {
            "workers"
        };
        let phase_active: Vec<Interval> = phase
            .states
            .iter()
            .filter(|state| {
                matches!(state.kind.to_lowercase().as_str(), "active" | "tool")
                    && state.end_ms > state.start_ms
            })
            .map(|state| (state.start_ms, state.end_ms))
            .collect();
        raw_active
            .get_mut(role)
            .unwrap()
            .entry(phase.agent_id.clone())
            .or_default()
            .extend(phase_active.iter().copied());
        let mut phase_evidence: Vec<Interval> = Vec::new();
        for entry in &phase.transcript {
            evidence_events
                .get_mut(role)
                .unwrap()
                .push(entry.at_ms);
            phase_evidence.push((
                entry.at_ms - EVIDENCE_HALF_WINDOW_MS,
                entry.at_ms + EVIDENCE_HALF_WINDOW_MS,
            ));
        }
        raw_evidence
            .get_mut(role)
            .unwrap()
            .entry(phase.agent_id.clone())
            .or_default()
            .extend(phase_evidence.iter().copied());
        let present_source = if phase_evidence.is_empty() {
            phase_active
        } else {
            phase_evidence
        };
        raw_present
            .get_mut(role)
            .unwrap()
            .entry(phase.agent_id.clone())
            .or_default()
            .extend(present_source);
    }
    let merge_roles = |source: &RoleMap| -> RoleMap {
        let mut out: RoleMap = HashMap::new();
        for role in ROLES {
            let mut inner: HashMap<String, Vec<Interval>> = HashMap::new();
            for (agent, intervals) in source.get(role).cloned().unwrap_or_default() {
                let merged = merge_intervals(&intervals);
                if !merged.is_empty() {
                    inner.insert(agent, merged);
                }
            }
            out.insert(role.to_string(), inner);
        }
        out
    };
    let active_by_role = merge_roles(&raw_active);
    let present_by_role = merge_roles(&raw_present);
    let evidence_by_role = merge_roles(&raw_evidence);
    let mut result = Vec::new();
    for resolution in RESOLUTIONS {
        result.extend(build_resolution(
            team,
            &active_by_role,
            &present_by_role,
            &evidence_by_role,
            &evidence_events,
            resolution,
            zone,
            observed_start_ms,
            observed_end_ms,
        ));
    }
    Ok(result)
}
