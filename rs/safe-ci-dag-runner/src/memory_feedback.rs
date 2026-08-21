//! Turn persisted per-step profiles into memory-admission estimates, conservatively.
//!
//! The profile store records what each step's cgroup peaked at. It does NOT follow that the peak
//! is what the step wanted: a step whose `peak_bytes` equals the `memory.max` applied to it used
//! everything it was allowed, and a step the kernel killed at that ceiling wanted strictly more.
//! Both are CENSORED observations. Fitting a cap to them re-derives the cap that produced them and
//! freezes the mistake, which is why the default planner feedback in [`crate::estimates`] is left
//! alone and this is a separate, opt-in path a caller must ask for by name.
//!
//! The rules a caller can rely on:
//!
//! * A censored sample never lowers anything. It is used only as a FLOOR — proof that demand was
//!   at least that large — never as an estimate of the maximum.
//! * A sample whose censoring cannot be determined (no applied-cap column, no event counters, no
//!   peak) is not evidence at all. It is counted and reported, and never moves the estimate.
//! * With no uncensored evidence the static hint is retained, and the reason says how the evidence
//!   fell short.
//! * `hard_mem_max_bytes` is never touched. An explicit hard cap is an instruction, not a guess.
//!
//! The columns read here are the ones the writer records per step; see
//! [`crate::perflog::STEP_PROFILE_COLUMNS`].

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use crate::estimates::{feedback_identity, high_percentile, load_store, parse_int};
use crate::model::{DagConfig, Step};

/// Uncensored samples a step needs before its recorded peaks may replace the authored hint. One
/// sample is a measurement, not a distribution; a cap derived from it would move on every run.
pub const DEFAULT_MIN_UNCENSORED_SAMPLES: usize = 5;

/// Headroom added above the percentile, as a percentage. The percentile describes the samples that
/// were taken; the margin covers the run that has not happened yet.
pub const DEFAULT_MARGIN_PCT: i64 = 20;

const RSS_PCTL_NUM: i64 = 9;
const RSS_PCTL_DEN: i64 = 10;

/// What a single recorded peak proves about the step's memory demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Censoring {
    /// The step ran under a known ceiling it never reached, with no reclaim and no OOM. The peak
    /// is a genuine observation of demand.
    Uncensored,
    /// The step was held at, or killed at, a ceiling. The peak is a LOWER BOUND on demand.
    Censored,
    /// The row does not say: missing peak, missing applied cap, or missing event counters. It is
    /// not evidence in either direction.
    Unknown,
}

/// One recorded peak, with the verdict on whether it measured demand or a ceiling.
#[derive(Debug, Clone)]
pub struct PeakObservation {
    /// The step tag the row belongs to.
    pub step: String,
    /// Recorded peak resident bytes, or `None` when the row had none.
    pub peak_bytes: Option<i64>,
    /// The applied `memory.max` in bytes, or `None` when unbounded or unrecorded; `cap_known` and
    /// `cap_unbounded` disambiguate the two.
    pub applied_cap_bytes: Option<i64>,
    /// Whether the row recorded an applied cap at all.
    pub cap_known: bool,
    /// Whether the recorded cap was the literal `max`.
    pub cap_unbounded: bool,
    /// `memory.events` `max`: times the kernel held the step at its ceiling by reclaiming.
    pub reclaim_events: Option<i64>,
    /// `memory.events` `oom_kill`, falling back to the older `oom_kills` column.
    pub oom_kills: Option<i64>,
    /// `memory.events` `high`: times the step was throttled into direct reclaim by a SOFT
    /// ceiling. The runner clears `memory.high` to `max`, best-effort; a non-zero count means
    /// that write did not take and an inherited soft cap held the step.
    pub throttle_events: Option<i64>,
    /// `memory.events` `oom`: times the OOM killer was invoked in the cgroup. A step can record
    /// this with `oom_kill == 0` (nothing reapable), and it is still a ceiling hit.
    pub oom_events: Option<i64>,
    /// The verdict.
    pub verdict: Censoring,
}

fn cell<'a>(row: &'a HashMap<String, String>, name: &str) -> Option<&'a String> {
    row.get(name)
}

fn truthy(row: &HashMap<String, String>, name: &str) -> bool {
    matches!(
        row.get(name)
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref(),
        Some("true") | Some("1") | Some("yes")
    )
}

/// Classify one profile row's recorded peak.
///
/// `Unknown` whenever the row cannot answer, which is the safe direction: an unknown row is
/// excluded from the estimate rather than assumed comfortable. `Censored` when ANY of the four
/// pressure counters #34 records fired — `high`, `max`, `oom` or `oom_kill` — when a guard cut
/// the step short, or when the peak reached the applied cap: a `>=` and not a `==`, because a cap
/// the kernel rounded down to a page boundary still censors a peak sitting above it.
///
/// `memory_events_low` is deliberately NOT read: it counts reclaim that breached a `memory.low`
/// PROTECTION, which sets a floor rather than a ceiling, so it does not bound the peak.
pub fn peak_observation_from_row(row: &HashMap<String, String>) -> PeakObservation {
    let step = row
        .get("step")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let peak_bytes = parse_int(cell(row, "peak_bytes")).filter(|v| *v >= 0);
    let cap_cell = row
        .get("memory_max_bytes")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let cap_known = !cap_cell.is_empty();
    let cap_unbounded = cap_cell == "max";
    let applied_cap_bytes = if cap_unbounded {
        None
    } else {
        parse_int(Some(&cap_cell)).filter(|v| *v >= 0)
    };
    let reclaim_events = parse_int(cell(row, "memory_events_max")).filter(|v| *v >= 0);
    let oom_cell = match row.get("memory_events_oom_kill") {
        Some(v) if !v.trim().is_empty() => Some(v),
        // Fall back to the long-standing per-step OOM column, which predates the event counters.
        _ => row.get("oom_kills"),
    };
    let oom_kills = parse_int(oom_cell).filter(|v| *v >= 0);
    let throttle_events = parse_int(cell(row, "memory_events_high")).filter(|v| *v >= 0);
    let oom_events = parse_int(cell(row, "memory_events_oom")).filter(|v| *v >= 0);
    let verdict = verdict(
        peak_bytes,
        cap_known,
        cap_unbounded,
        applied_cap_bytes,
        reclaim_events,
        oom_kills,
        throttle_events,
        oom_events,
        truthy(row, "timed_out") || truthy(row, "cpu_timed_out"),
    );
    PeakObservation {
        step,
        peak_bytes,
        applied_cap_bytes,
        cap_known,
        cap_unbounded,
        reclaim_events,
        oom_kills,
        throttle_events,
        oom_events,
        verdict,
    }
}

// The classification rules, separated so they can be read and tested without a CSV row.
#[allow(clippy::too_many_arguments)]
fn verdict(
    peak_bytes: Option<i64>,
    cap_known: bool,
    cap_unbounded: bool,
    applied_cap_bytes: Option<i64>,
    reclaim_events: Option<i64>,
    oom_kills: Option<i64>,
    throttle_events: Option<i64>,
    oom_events: Option<i64>,
    timed_out: bool,
) -> Censoring {
    let Some(peak) = peak_bytes else {
        return Censoring::Unknown;
    };
    if oom_kills.is_some_and(|n| n > 0) {
        return Censoring::Censored;
    }
    if oom_events.is_some_and(|n| n > 0) {
        // The OOM killer was invoked even if it reaped nothing reapable: the step was at a
        // ceiling.
        return Censoring::Censored;
    }
    if timed_out {
        // The step was cut short, so its peak is where it had got to, not where it was going —
        // a lower bound on demand, exactly like a ceiling hit.
        return Censoring::Censored;
    }
    if !cap_known {
        return Censoring::Unknown;
    }
    let Some(reclaim) = reclaim_events else {
        return Censoring::Unknown;
    };
    if reclaim > 0 {
        return Censoring::Censored;
    }
    if throttle_events.is_some_and(|n| n > 0) {
        // A soft ceiling was throttling. The runner clears memory.high to "max" best-effort, so
        // a non-zero count means that write did not take and an inherited soft cap held the step.
        return Censoring::Censored;
    }
    if cap_unbounded {
        return Censoring::Uncensored;
    }
    let Some(cap) = applied_cap_bytes else {
        return Censoring::Unknown;
    };
    if peak >= cap {
        Censoring::Censored
    } else {
        Censoring::Uncensored
    }
}

/// A step's candidate `rss_baseline_bytes`, and everything needed to justify it.
///
/// `rss_baseline_bytes` is `None` when the evidence does not support replacing the authored hint;
/// `reason` then says how it fell short. When it is `Some` the caller may use it directly: it
/// already includes the margin and is already raised above every observed peak.
#[derive(Debug, Clone)]
pub struct MemoryAdmission {
    /// The step tag.
    pub step: String,
    /// The recommended baseline, or `None` to keep the authored hint.
    pub rss_baseline_bytes: Option<i64>,
    /// `"profile"` when the samples decided the number, `"hint"` when they did not.
    pub source: String,
    /// Why, in one human-readable clause.
    pub reason: String,
    /// Rows seen for this step, including any excluded by source revision.
    pub samples: usize,
    /// Rows whose peak measured demand.
    pub uncensored_samples: usize,
    /// Rows whose peak met a ceiling.
    pub censored_samples: usize,
    /// Rows that could not say.
    pub unknown_samples: usize,
    /// The percentile of the uncensored peaks used as the central estimate, as `num/den`.
    pub percentile: String,
    /// Headroom added above the percentile, as a percentage.
    pub margin_pct: i64,
    /// The largest CENSORED peak: a proven lower bound the estimate may not go below.
    pub censored_floor_bytes: Option<i64>,
    /// The largest UNCENSORED peak, which the estimate may also not go below.
    pub observed_peak_bytes: Option<i64>,
}

impl MemoryAdmission {
    /// Whether any sample was withheld from the central estimate because of censoring.
    pub fn censoring_excluded_samples(&self) -> bool {
        self.censored_samples > 0
    }
}

/// Aggregate profile `rows` into one [`MemoryAdmission`] per step.
///
/// `profile_base_sha`, when given, restricts the evidence to rows recorded against that source
/// revision; the count dropped for that reason is reported in the reason rather than absorbed.
///
/// The estimate for a step with enough uncensored evidence is
/// `max(percentile(uncensored) * (1 + margin), max uncensored, max censored)`. The percentile is
/// the central estimate; the two maxima are floors, so a censored peak can raise a cap and can
/// never lower one.
pub fn memory_admission_from_rows(
    rows: &[HashMap<String, String>],
    min_uncensored_samples: usize,
    margin_pct: i64,
    profile_base_sha: Option<&str>,
) -> BTreeMap<String, MemoryAdmission> {
    let mut observations: BTreeMap<String, Vec<PeakObservation>> = BTreeMap::new();
    let mut dropped_by_sha: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let observation = peak_observation_from_row(row);
        if observation.step.is_empty() {
            continue;
        }
        seen.insert(observation.step.clone());
        if let Some(want) = profile_base_sha {
            let recorded = row
                .get("profile_base_sha")
                .map(|s| s.trim())
                .unwrap_or_default();
            if recorded != want {
                *dropped_by_sha.entry(observation.step.clone()).or_insert(0) += 1;
                continue;
            }
        }
        observations
            .entry(observation.step.clone())
            .or_default()
            .push(observation);
    }
    let empty: Vec<PeakObservation> = Vec::new();
    seen.into_iter()
        .map(|step| {
            let admission = admission_for_step(
                &step,
                observations.get(&step).unwrap_or(&empty),
                dropped_by_sha.get(&step).copied().unwrap_or(0),
                min_uncensored_samples,
                margin_pct,
            );
            (step, admission)
        })
        .collect()
}

fn admission_for_step(
    step: &str,
    observations: &[PeakObservation],
    dropped_by_sha: usize,
    min_uncensored_samples: usize,
    margin_pct: i64,
) -> MemoryAdmission {
    let uncensored: Vec<i64> = observations
        .iter()
        .filter(|o| o.verdict == Censoring::Uncensored)
        .filter_map(|o| o.peak_bytes)
        .collect();
    let censored: Vec<i64> = observations
        .iter()
        .filter(|o| o.verdict == Censoring::Censored)
        .filter_map(|o| o.peak_bytes)
        .collect();
    let unknown = observations
        .iter()
        .filter(|o| o.verdict == Censoring::Unknown)
        .count();
    let floor = censored.iter().copied().max();
    let observed = uncensored.iter().copied().max();
    let sha_note = if dropped_by_sha > 0 {
        format!("; {dropped_by_sha} sample(s) excluded as recorded against another source revision")
    } else {
        String::new()
    };
    let percentile = format!("{RSS_PCTL_NUM}/{RSS_PCTL_DEN}");
    let mut admission = MemoryAdmission {
        step: step.to_string(),
        rss_baseline_bytes: None,
        source: "hint".to_string(),
        reason: String::new(),
        samples: observations.len() + dropped_by_sha,
        uncensored_samples: uncensored.len(),
        censored_samples: censored.len(),
        unknown_samples: unknown,
        percentile,
        margin_pct,
        censored_floor_bytes: floor,
        observed_peak_bytes: observed,
    };
    if uncensored.is_empty() {
        let why = if !censored.is_empty() && unknown == 0 {
            format!(
                "every one of {} recorded peak(s) was censored by its applied cap",
                censored.len()
            )
        } else if !censored.is_empty() {
            format!(
                "no uncensored peak: {} censored and {unknown} of unknown provenance",
                censored.len()
            )
        } else if unknown > 0 {
            format!(
                "{unknown} sample(s) carry no applied-cap or event provenance, so censoring \
                 cannot be ruled out"
            )
        } else {
            "no recorded peaks for this step".to_string()
        };
        admission.reason = format!("{why}{sha_note}");
        return admission;
    }
    if uncensored.len() < min_uncensored_samples {
        admission.reason = format!(
            "only {} uncensored sample(s); {min_uncensored_samples} required{sha_note}",
            uncensored.len()
        );
        return admission;
    }
    let central = high_percentile(&uncensored);
    let with_margin = central + (central * margin_pct) / 100;
    let estimate = with_margin
        .max(observed.unwrap_or(0))
        .max(floor.unwrap_or(0));
    let mut detail = format!(
        "{} uncensored sample(s), {} percentile +{margin_pct}%",
        uncensored.len(),
        admission.percentile
    );
    if !censored.is_empty() {
        detail.push_str(&format!(
            ", floored at the largest of {} censored peak(s)",
            censored.len()
        ));
    }
    if unknown > 0 {
        detail.push_str(&format!(
            ", {unknown} sample(s) of unknown provenance ignored"
        ));
    }
    admission.rss_baseline_bytes = Some(estimate);
    admission.source = "profile".to_string();
    admission.reason = format!("{detail}{sha_note}");
    admission
}

/// Read the store for one machine/container identity and aggregate its admissions.
///
/// Passing `None` for either identity component uses this host's, via
/// [`crate::estimates::feedback_identity`], because a cap learned on one container class does not
/// transfer to another. An empty result means "keep every authored hint", never "no memory is
/// needed".
pub fn load_memory_admissions(
    profile_dir: &Path,
    machine_id: Option<&str>,
    container_class: Option<&str>,
    min_uncensored_samples: usize,
    margin_pct: i64,
    profile_base_sha: Option<&str>,
) -> BTreeMap<String, MemoryAdmission> {
    let (host_machine, host_container) = feedback_identity();
    let machine = machine_id.unwrap_or(&host_machine);
    let container = container_class.unwrap_or(&host_container);
    let Some((rows, _affinity)) = load_store(profile_dir, machine, container) else {
        return BTreeMap::new();
    };
    memory_admission_from_rows(&rows, min_uncensored_samples, margin_pct, profile_base_sha)
}

/// Return a copy of `cfg` whose steps carry the profile-derived `rss_baseline_bytes`.
///
/// Only a step with an admission whose source is `"profile"` is changed; every other step keeps
/// its authored hint verbatim. `hard_mem_max_bytes` is carried through untouched in all cases: an
/// explicit hard cap is the caller's instruction and this path does not reinterpret it. An
/// intentionally skipped step is left alone, for the same reason
/// [`crate::estimates::apply_plan_to_config`] leaves it alone.
pub fn apply_memory_admissions(
    cfg: &DagConfig,
    admissions: &BTreeMap<String, MemoryAdmission>,
) -> DagConfig {
    let steps: Vec<Step> = cfg
        .steps
        .iter()
        .map(|step| {
            let tag = step.tag();
            let Some(admission) = admissions.get(&tag) else {
                return step.clone();
            };
            if admission.source != "profile" || step.skip_reason.is_some() {
                return step.clone();
            }
            let Some(bytes) = admission.rss_baseline_bytes else {
                return step.clone();
            };
            let mut out = step.clone();
            out.hint.rss_baseline_bytes = Some(bytes);
            out
        })
        .collect();
    cfg.with_steps(steps)
}

/// One human-readable line stating the decision AND the evidence behind it.
///
/// Every step the store knows about gets one, including the ones that did NOT move, because
/// "the cap did not change" and "the store had nothing usable to say" are otherwise
/// indistinguishable from the outside.
pub fn memory_admission_line(prog: &str, admission: &MemoryAdmission) -> String {
    let verdict = match admission.rss_baseline_bytes {
        Some(bytes) if admission.source == "profile" => format!("rss_baseline_bytes={bytes}"),
        _ => "keeping the authored hint".to_string(),
    };
    format!(
        "{prog}: --profile-memory-feedback: {}: {verdict} [{} uncensored, {} censored, \
         {} unprovenanced of {}; {}]",
        admission.step,
        admission.uncensored_samples,
        admission.censored_samples,
        admission.unknown_samples,
        admission.samples,
        admission.reason,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: i64 = 1024 * 1024 * 1024;

    fn mk(job: &str) -> Step {
        Step {
            group: "g".into(),
            job: job.into(),
            desc: String::new(),
            description: String::new(),
            cmd: "true".into(),
            deps: Vec::new(),
            env: BTreeMap::new(),
            hint: crate::model::ResourceHint::default(),
            networkonly: false,
            engine_only: false,
            timeout: 1800,
            cpu_timeout: 0,
            jobs_flag: None,
            skip_reason: None,
            write_domains: None,
            write_domain_guarantee: None,
        }
    }

    fn cfg_of(steps: Vec<Step>) -> DagConfig {
        DagConfig::default().with_steps(steps)
    }

    fn row(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = [
            ("step", "g.build"),
            ("peak_bytes", "1073741824"),
            ("memory_max_bytes", "8589934592"),
            ("memory_events_high", "0"),
            ("memory_events_max", "0"),
            ("memory_events_oom", "0"),
            ("memory_events_oom_kill", "0"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        for (k, v) in pairs {
            out.insert(k.to_string(), v.to_string());
        }
        out
    }

    #[test]
    fn a_peak_under_a_known_quiet_cap_is_a_real_observation() {
        let observed = peak_observation_from_row(&row(&[("peak_bytes", "2147483648")]));
        assert_eq!(observed.verdict, Censoring::Uncensored);
        assert_eq!(observed.peak_bytes, Some(2 * GIB));
        assert_eq!(observed.applied_cap_bytes, Some(8 * GIB));
    }

    #[test]
    fn a_peak_that_met_a_ceiling_is_censored() {
        for (why, pairs) in [
            (
                "peak reached the cap",
                vec![
                    ("peak_bytes", "8589934592"),
                    ("memory_max_bytes", "8589934592"),
                ],
            ),
            (
                "peak above a page-rounded cap",
                vec![
                    ("peak_bytes", "8589934592"),
                    ("memory_max_bytes", "8589930496"),
                ],
            ),
            (
                "held at the ceiling by reclaim",
                vec![("memory_events_max", "17")],
            ),
            (
                "killed at the ceiling",
                vec![("memory_events_oom_kill", "2")],
            ),
            (
                "throttled at a soft ceiling",
                vec![("memory_events_high", "3")],
            ),
            (
                "the oom killer was invoked without a recorded kill",
                vec![("memory_events_oom", "1")],
            ),
            ("cut short by the wall guard", vec![("timed_out", "true")]),
            (
                "cut short by the cpu guard",
                vec![("cpu_timed_out", "true")],
            ),
        ] {
            assert_eq!(
                peak_observation_from_row(&row(&pairs)).verdict,
                Censoring::Censored,
                "{why}"
            );
        }
    }

    #[test]
    fn a_row_that_cannot_answer_is_unknown_not_uncensored() {
        // Silence must not read as comfort: an unprovenanced row is excluded, never assumed safe.
        for (why, pairs) in [
            ("no applied cap recorded", vec![("memory_max_bytes", "")]),
            (
                "no event counters recorded",
                vec![("memory_events_max", "")],
            ),
            ("no peak recorded", vec![("peak_bytes", "")]),
            (
                "a cap cell that is neither max nor a number",
                vec![("memory_max_bytes", "unbounded")],
            ),
        ] {
            assert_eq!(
                peak_observation_from_row(&row(&pairs)).verdict,
                Censoring::Unknown,
                "{why}"
            );
        }
    }

    #[test]
    fn every_pressure_counter_the_writer_records_is_read() {
        // #34 persists five `memory.events` counters. A reader that consults only some of them
        // calls a throttled or OOM-invoked step comfortable, which is the exact failure this
        // path exists to prevent. `low` is deliberately NOT censoring: it counts reclaim that
        // breached a `memory.low` PROTECTION, which does not bound the cgroup's own peak.
        for column in [
            "memory_events_high",
            "memory_events_max",
            "memory_events_oom",
            "memory_events_oom_kill",
        ] {
            assert_eq!(
                peak_observation_from_row(&row(&[(column, "1")])).verdict,
                Censoring::Censored,
                "{column}"
            );
        }
        assert_eq!(
            peak_observation_from_row(&row(&[("memory_events_low", "9")])).verdict,
            Censoring::Uncensored,
        );
    }

    #[test]
    fn an_unbounded_step_that_never_reclaimed_is_a_real_observation() {
        let observed = peak_observation_from_row(&row(&[("memory_max_bytes", "max")]));
        assert!(observed.cap_known);
        assert!(observed.cap_unbounded);
        assert_eq!(observed.verdict, Censoring::Uncensored);
    }

    #[test]
    fn the_legacy_oom_column_still_censors_when_the_event_counter_is_absent() {
        let observed =
            peak_observation_from_row(&row(&[("memory_events_oom_kill", ""), ("oom_kills", "1")]));
        assert_eq!(observed.verdict, Censoring::Censored);
    }

    #[test]
    fn enough_quiet_samples_produce_a_percentile_estimate_with_margin() {
        // The expected bytes are written out LITERALLY. Recomputing them from
        // DEFAULT_MARGIN_PCT would pin the arithmetic to whatever the constant happens to be, so
        // changing 20 to 0 would leave the test green while every learned cap lost its headroom.
        let rows: Vec<_> = (0..6)
            .map(|_| row(&[("peak_bytes", "2147483648")]))
            .collect();
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let a = &admissions["g.build"];
        assert_eq!(a.source, "profile");
        assert_eq!(a.uncensored_samples, 6);
        assert_eq!(a.margin_pct, 20);
        // Every sample is exactly 2147483648 B, so the 9/10 percentile is 2147483648 B and the
        // whole difference is the 20% margin: 2147483648 + 429496729 = 2576980377.
        assert_eq!(a.rss_baseline_bytes, Some(2576980377));
        assert!(!a.censoring_excluded_samples());
    }

    #[test]
    fn the_default_margin_is_twenty_percent() {
        // Named once, here, so a change to the shipped headroom has to be made deliberately.
        assert_eq!(DEFAULT_MARGIN_PCT, 20);
    }

    #[test]
    fn the_estimate_never_falls_below_the_largest_censored_peak() {
        // Five quiet 1 GiB samples would justify 1.2 GiB. One sample that hit an 8 GiB ceiling
        // says the step has wanted 8 GiB at least once, so 1.2 GiB is a number it has exceeded.
        let mut rows: Vec<_> = (0..5).map(|_| row(&[])).collect();
        rows.push(row(&[
            ("peak_bytes", "8589934592"),
            ("memory_events_oom_kill", "1"),
        ]));
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let a = &admissions["g.build"];
        assert_eq!(a.uncensored_samples, 5);
        assert_eq!(a.censored_samples, 1);
        assert_eq!(a.censored_floor_bytes, Some(8 * GIB));
        assert_eq!(a.rss_baseline_bytes, Some(8 * GIB));
    }

    #[test]
    fn every_peak_censored_keeps_the_static_hint_and_says_so() {
        // The DeepScry case: a step whose whole history sits on its own ceiling.
        let rows: Vec<_> = (0..30)
            .map(|_| row(&[("peak_bytes", "8589934592")]))
            .collect();
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let a = &admissions["g.build"];
        assert_eq!(a.source, "hint");
        assert_eq!(a.rss_baseline_bytes, None);
        assert_eq!(a.censored_samples, 30);
        assert!(
            a.reason.contains("censored by its applied cap"),
            "{}",
            a.reason
        );
    }

    #[test]
    fn unprovenanced_samples_alone_keep_the_static_hint_and_say_so() {
        let rows: Vec<_> = (0..30)
            .map(|_| row(&[("memory_max_bytes", ""), ("memory_events_max", "")]))
            .collect();
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let a = &admissions["g.build"];
        assert_eq!(a.source, "hint");
        assert_eq!(a.unknown_samples, 30);
        assert!(
            a.reason.contains("no applied-cap or event provenance"),
            "{}",
            a.reason
        );
    }

    #[test]
    fn too_few_uncensored_samples_keeps_the_static_hint_and_says_how_many() {
        let rows: Vec<_> = (0..2).map(|_| row(&[])).collect();
        let admissions = memory_admission_from_rows(&rows, 5, DEFAULT_MARGIN_PCT, None);
        let a = &admissions["g.build"];
        assert_eq!(a.source, "hint");
        assert!(
            a.reason.contains("only 2 uncensored sample(s); 5 required"),
            "{}",
            a.reason
        );
    }

    #[test]
    fn samples_from_another_revision_are_excluded_and_counted() {
        let mut rows: Vec<_> = (0..5)
            .map(|_| row(&[("profile_base_sha", "new")]))
            .collect();
        rows.extend((0..20).map(|_| {
            row(&[
                ("peak_bytes", "68719476736"),
                ("memory_max_bytes", "137438953472"),
                ("profile_base_sha", "old"),
            ])
        }));
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            Some("new"),
        );
        let a = &admissions["g.build"];
        assert_eq!(a.source, "profile");
        assert_eq!(a.uncensored_samples, 5);
        assert!(a.rss_baseline_bytes.is_some_and(|v| v < 2 * GIB));
        assert!(
            a.reason
                .contains("20 sample(s) excluded as recorded against another source revision"),
            "{}",
            a.reason
        );
    }

    #[test]
    fn an_explicit_hard_cap_is_never_rewritten_by_the_profile_path() {
        let rows: Vec<_> = (0..6)
            .map(|_| row(&[("peak_bytes", "2147483648")]))
            .collect();
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let mut step = mk("build");
        step.hint.rss_baseline_bytes = Some(99);
        step.hint.hard_mem_max_bytes = Some(5 * GIB);
        step.timeout = 1234;
        let cfg = cfg_of(vec![step]);

        let applied = apply_memory_admissions(&cfg, &admissions);

        let out = &applied.steps[0];
        assert_eq!(out.hint.hard_mem_max_bytes, Some(5 * GIB));
        assert_ne!(out.hint.rss_baseline_bytes, Some(99));
        // Clone-and-override: no other field may reset here.
        assert_eq!(out.timeout, 1234);
    }

    #[test]
    fn only_a_profile_backed_admission_replaces_the_authored_hint() {
        let mut rows: Vec<_> = (0..6)
            .map(|_| row(&[("step", "g.learned"), ("peak_bytes", "2147483648")]))
            .collect();
        rows.extend((0..6).map(|_| row(&[("step", "g.pinned"), ("peak_bytes", "8589934592")])));
        let admissions = memory_admission_from_rows(
            &rows,
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        let mut learned = mk("learned");
        learned.hint.rss_baseline_bytes = Some(99);
        let mut pinned = mk("pinned");
        pinned.hint.rss_baseline_bytes = Some(99);
        let cfg = cfg_of(vec![learned, pinned]);

        let applied = apply_memory_admissions(&cfg, &admissions);

        assert_eq!(
            applied.steps[0].hint.rss_baseline_bytes,
            admissions["g.learned"].rss_baseline_bytes
        );
        assert_eq!(applied.steps[1].hint.rss_baseline_bytes, Some(99));
    }

    #[test]
    fn a_missing_store_means_keep_every_hint_rather_than_no_memory() {
        let dir = std::env::temp_dir().join(format!("scdr_memfb_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let admissions = load_memory_admissions(
            &dir,
            Some("nosuchhost"),
            Some("affinity4_cpu-max-max"),
            DEFAULT_MIN_UNCENSORED_SAMPLES,
            DEFAULT_MARGIN_PCT,
            None,
        );
        assert!(admissions.is_empty());
        let mut step = mk("build");
        step.hint.rss_baseline_bytes = Some(7 * GIB);
        let cfg = cfg_of(vec![step]);
        assert_eq!(
            apply_memory_admissions(&cfg, &admissions).steps[0]
                .hint
                .rss_baseline_bytes,
            Some(7 * GIB)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
