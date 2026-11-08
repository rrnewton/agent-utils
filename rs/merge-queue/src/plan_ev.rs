//! The expected-value planner (SubmitQueue, EuroSys '19, §4/§7.1): choose
//! builds to maximise expected landed changes per validate-hour. A build's
//! value is `expected landed / expected total cost`, where the numerator is
//! the paper's `benefit x P_needed` (benefit = changes landed when the build
//! is green and needed) and the denominator prices the serial-split fallback
//! a red batch triggers (the consumer's measured cost model: batch cost is
//! flat in width, so `E[cost] = C x (1 + (1 - P_pass) x width)` with solo
//! cost `C`). At per-change p ~0.5 singles win and the planner emits k
//! independent roots; at high p, batches and stacks win. Width, depth and
//! roots from the existing knobs (including the k-roots pass-rate gates) are
//! CAPS here, never targets. Every choice carries its rationale string for
//! the run report and `stats`. Live use is behind `Config::ev_planner`
//! (default off); `merge-queue simulate` compares policies offline.

use std::collections::{BTreeMap, BTreeSet};

use crate::predict::Model;

/// One pending change as the planner sees it.
#[derive(Clone, Debug)]
pub struct Candidate {
    /// Change number.
    pub pr: i64,
    /// Predicted pass probability on a clean base (model, or a rate fallback).
    pub p: f64,
    /// Changes it textually conflicts with (cannot share a root set freely).
    pub conflicts: BTreeSet<i64>,
}

/// Caps the planner may not exceed (from the existing knobs and gates).
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// Maximum changes per batch (`max_batch`, batch-gate result applied).
    pub max_batch: usize,
    /// Maximum concurrent builds (the window).
    pub window: usize,
    /// Maximum base-rooted builds (`roots`).
    pub roots: usize,
    /// Whether stacked builds are allowed at all (stack-gate result).
    pub stack_allowed: bool,
    /// Expected solo validation wall seconds (`C`).
    pub solo_cost_s: f64,
    /// In [`simulate`] only: model base-move root discards - a root whose
    /// jumped-over build lands first in its epoch wastes its validation
    /// and rebuilds (#1071 disjoint-root-stacking). Off keeps the
    /// historical simulate semantics (roots always usable) exactly.
    pub root_discards: bool,
    /// With `root_discards`, the predicted pass percent at or above which
    /// a landed build rescues a later root in [`simulate`] by stacking it
    /// (0: never rescued). Mirrors `Config::root_stack_min_pass_percent`.
    pub root_stack_min_pass_percent: u32,
}

/// One chosen build and why.
#[derive(Clone, Debug)]
pub struct Build {
    /// Changes in combine order.
    pub prs: Vec<i64>,
    /// Built on the base itself (a root), not on the stack.
    pub rooted: bool,
    /// Probability this build is ever needed (§4.2.1).
    pub p_needed: f64,
    /// Probability it validates green given it runs.
    pub p_pass: f64,
    /// Expected changes landed per validate-hour (the value).
    pub value: f64,
    /// Human rationale with the numbers, for reports.
    pub rationale: String,
}

/// Probability a batch passes: product of per-change p, discounted by each
/// known pair conflict `(1 - P_conf)` (paper Eq. 5 shape).
pub fn batch_p_pass(prs: &[i64], p: &BTreeMap<i64, f64>, model: Option<&Model>) -> f64 {
    let mut pass: f64 = prs
        .iter()
        .map(|pr| p.get(pr).copied().unwrap_or(0.5))
        .product();
    if let Some(model) = model {
        for i in 0..prs.len() {
            for j in (i + 1)..prs.len() {
                if let Some(conf) = model.p_conf(prs[i], prs[j]) {
                    pass *= 1.0 - conf;
                }
            }
        }
    }
    pass.clamp(0.01, 0.99)
}

/// Why an in-flight batch counts as likely to land first for the disjoint
/// root choice (#1071 disjoint-root-stacking), or `None` when a would-be
/// root keeps today's behaviour: the feature is off while
/// `min_pass_percent` is 0, and otherwise a batch that is prioritized,
/// already green awaiting decision, or predicted to pass at or above the
/// threshold qualifies. Shared by the run loop and unit tests so the
/// live decision has one definition.
pub fn likely_landing_reason(
    p_pass: f64,
    prioritized: bool,
    already_green: bool,
    min_pass_percent: u32,
) -> Option<&'static str> {
    if min_pass_percent == 0 {
        return None;
    }
    if already_green {
        Some("already green")
    } else if prioritized {
        Some("prioritized")
    } else if p_pass * 100.0 >= f64::from(min_pass_percent) {
        Some("predicted pass")
    } else {
        None
    }
}

/// Expected total validate seconds to land a batch's changes, pricing the
/// serial-split fallback after a red (consumer cost model §4.2).
pub fn expected_cost_s(p_pass: f64, width: usize, solo_cost_s: f64) -> f64 {
    solo_cost_s * (1.0 + (1.0 - p_pass) * width.max(1) as f64)
}

/// Value of one build: expected landed changes per validate-hour.
pub fn value_of(
    prs: &[i64],
    rooted: bool,
    p_needed: f64,
    p: &BTreeMap<i64, f64>,
    model: Option<&Model>,
    solo_cost_s: f64,
) -> Build {
    let p_pass = batch_p_pass(prs, p, model);
    let expected_landed = prs.len() as f64 * p_pass * p_needed;
    let cost_h = expected_cost_s(p_pass, prs.len(), solo_cost_s) / 3600.0;
    let value = if cost_h > 0.0 {
        expected_landed / cost_h
    } else {
        0.0
    };
    Build {
        prs: prs.to_vec(),
        rooted,
        p_needed,
        p_pass,
        value,
        rationale: format!(
            "{} {}: p_pass {:.2} x P_needed {:.2} = {:.2} expected landed / {:.2}h expected cost = {:.2}/h",
            if rooted { "root" } else { "stack" },
            prs.iter().map(|pr| format!("#{pr}")).collect::<Vec<_>>().join("+"),
            p_pass,
            p_needed,
            expected_landed,
            cost_h,
            value
        ),
    }
}

fn batch_conflict_free(prs: &[i64], remaining: &[Candidate]) -> bool {
    let set: BTreeSet<i64> = prs.iter().copied().collect();
    remaining
        .iter()
        .filter(|c| set.contains(&c.pr))
        .all(|c| c.conflicts.intersection(&set).all(|other| *other == c.pr))
}

/// Greedy best-first over the speculation graph (paper §7.1): repeatedly
/// take the highest-value build the caps still allow. Enumeration is the
/// honest small space: root builds are width-1..=max_batch prefixes of the
/// remaining disjoint-compatible candidates in queue order; stacked builds
/// are prefixes on the current stack (P_needed = product of the p_pass of
/// every build below). Ties break toward narrower builds (less waste on red).
pub fn plan(candidates: &[Candidate], caps: &Caps, model: Option<&Model>) -> Vec<Build> {
    let p: BTreeMap<i64, f64> = candidates
        .iter()
        .map(|c| (c.pr, c.p.clamp(0.01, 0.99)))
        .collect();
    let mut remaining: Vec<Candidate> = candidates.to_vec();
    let mut builds: Vec<Build> = Vec::new();
    let mut roots_used = 0usize;
    let mut stack_p_needed = 1.0f64; // P_needed of the next stacked build.
    while builds.len() < caps.window.max(1) && !remaining.is_empty() {
        let mut options: Vec<Build> = Vec::new();
        // Root builds (any slot while roots remain).
        if roots_used < caps.roots.max(1) {
            for width in 1..=caps.max_batch.max(1).min(remaining.len()) {
                let prs: Vec<i64> = remaining.iter().take(width).map(|c| c.pr).collect();
                // A root batch must be internally conflict-free to be a batch.
                if batch_conflict_free(&prs, &remaining) {
                    options.push(value_of(&prs, true, 1.0, &p, model, caps.solo_cost_s));
                }
            }
        }
        // One stacked build on top of everything chosen so far.
        if caps.stack_allowed {
            for width in 1..=caps.max_batch.max(1).min(remaining.len()) {
                let prs: Vec<i64> = remaining.iter().take(width).map(|c| c.pr).collect();
                options.push(value_of(
                    &prs,
                    false,
                    stack_p_needed,
                    &p,
                    model,
                    caps.solo_cost_s,
                ));
            }
        }
        options.sort_by(|a, b| {
            b.value
                .partial_cmp(&a.value)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.prs.len().cmp(&b.prs.len()))
        });
        let Some(best) = options.into_iter().next() else {
            break;
        };
        if best.rooted {
            roots_used += 1;
        } else {
            stack_p_needed *= best.p_pass;
        }
        let taken: BTreeSet<i64> = best.prs.iter().copied().collect();
        remaining.retain(|c| !taken.contains(&c.pr));
        builds.push(best);
    }
    builds
}

/// Whether the best queued build should abort the lowest-value in-flight
/// build (the planner move, §6): strictly better value wins the slot.
pub fn should_abort(inflight_values: &[f64], queued_best: f64) -> Option<usize> {
    let (index, lowest) = inflight_values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    (queued_best > *lowest).then_some(index)
}

// --- Offline policy simulation (replay) -------------------------------------

/// Which scheduler a simulation runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Today's shape: one stacked path, fixed width/depth.
    Optimistic,
    /// k independent single-change roots (the k-roots baseline shape).
    Kroots,
    /// This planner.
    Ev,
}

/// Outcome of one simulation.
#[derive(Clone, Debug, Default)]
pub struct SimResult {
    /// Changes landed.
    pub landed: usize,
    /// Validate wall hours spent (including wasted).
    pub validate_hours: f64,
    /// Validations started.
    pub validations: usize,
    /// Validate hours spent on builds whose verdict was never usable.
    pub wasted_hours: f64,
    /// Landed per validate-hour.
    pub landed_per_hour: f64,
    /// Roots discarded because a jumped-over build landed first and moved
    /// their base; only counted with `Caps::root_discards`, else 0.
    pub discarded_roots: usize,
}

struct Rng(u64);
impl Rng {
    fn next_f64(&mut self) -> f64 {
        // xorshift64*: deterministic across runs and platforms.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        let v = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Simulate one policy over a fixed pending set with known per-change pass
/// probabilities (ground truth for the Monte Carlo, distinct from the
/// planner's estimates). Sequential epoch simulation: each epoch starts up
/// to `window` builds; a stacked build's verdict is usable only if every
/// build below it lands, otherwise its hours are wasted (the optimistic
/// failure mode); roots are always usable. Red batches fall back to solo
/// splits. Costs are flat per build (`solo_cost_s`), per the measured model.
pub fn simulate(
    truth_p: &BTreeMap<i64, f64>,
    order: &[i64],
    policy: Policy,
    caps: &Caps,
    estimates: &BTreeMap<i64, f64>,
    model: Option<&Model>,
    seed: u64,
) -> SimResult {
    let mut rng = Rng(seed.max(1));
    let mut pending: Vec<i64> = order.to_vec();
    let mut result = SimResult::default();
    let cost_h = caps.solo_cost_s / 3600.0;
    let mut guard = 0usize;
    while !pending.is_empty() && guard < 100_000 {
        guard += 1;
        // Choose this epoch's builds.
        let builds: Vec<Build> = match policy {
            Policy::Optimistic => {
                let width = caps.max_batch.max(1);
                let mut out = Vec::new();
                let mut needed = 1.0;
                let mut rest = pending.clone();
                while out.len() < caps.window.max(1) && !rest.is_empty() {
                    let prs: Vec<i64> = rest.drain(..width.min(rest.len())).collect();
                    let b = value_of(&prs, false, needed, truth_p, None, caps.solo_cost_s);
                    needed *= b.p_pass;
                    out.push(b);
                }
                out
            }
            Policy::Kroots => pending
                .iter()
                .take(caps.window.max(1))
                .map(|pr| value_of(&[*pr], true, 1.0, truth_p, None, caps.solo_cost_s))
                .collect(),
            Policy::Ev => {
                let candidates: Vec<Candidate> = pending
                    .iter()
                    .map(|pr| Candidate {
                        pr: *pr,
                        p: estimates.get(pr).copied().unwrap_or(0.5),
                        conflicts: BTreeSet::new(),
                    })
                    .collect();
                plan(&candidates, caps, model)
            }
        };
        if builds.is_empty() {
            break;
        }
        // Resolve in order: a stacked build is wasted when any build below
        // it in this epoch did not land. With `root_discards`, a root is
        // likewise wasted when an earlier build already landed in this
        // epoch (its base moved under it), unless a landed build at or
        // above the stack threshold rescues it by stacking it.
        let mut below_landed = true;
        let mut landed_any = false;
        let mut landed_best_p = 0.0f64;
        let mut landed_this_epoch: Vec<i64> = Vec::new();
        for build in &builds {
            result.validations += 1;
            result.validate_hours += cost_h;
            let rescued = caps.root_discards
                && build.rooted
                && landed_any
                && caps.root_stack_min_pass_percent > 0
                && landed_best_p * 100.0 >= f64::from(caps.root_stack_min_pass_percent);
            if caps.root_discards && build.rooted && landed_any && !rescued {
                result.wasted_hours += cost_h;
                result.discarded_roots += 1;
                continue;
            }
            let usable = build.rooted || below_landed || rescued;
            if !usable {
                result.wasted_hours += cost_h;
                continue;
            }
            let landed_before = landed_this_epoch.len();
            let green = rng.next_f64() < batch_p_pass(&build.prs, truth_p, None);
            if green {
                landed_this_epoch.extend(build.prs.iter().copied());
            } else {
                below_landed = false;
                // Serial-split fallback: each member solo, flat cost each.
                for pr in &build.prs {
                    result.validations += 1;
                    result.validate_hours += cost_h;
                    if rng.next_f64() < truth_p.get(pr).copied().unwrap_or(0.5) {
                        landed_this_epoch.push(*pr);
                    }
                }
            }
            if landed_this_epoch.len() > landed_before {
                landed_any = true;
                landed_best_p = landed_best_p.max(build.p_pass);
            }
        }
        let landed_set: BTreeSet<i64> = landed_this_epoch.iter().copied().collect();
        result.landed += landed_set.len();
        pending.retain(|pr| !landed_set.contains(pr));
        // Changes that failed solo twice are ejected (bounded loop safety):
        // drop the head when nothing landed this epoch.
        if landed_set.is_empty() && !pending.is_empty() {
            pending.remove(0);
        }
    }
    if result.validate_hours > 0.0 {
        result.landed_per_hour = result.landed as f64 / result.validate_hours;
    }
    result
}

#[cfg(test)]
mod unit {
    use super::*;

    fn caps(max_batch: usize) -> Caps {
        Caps {
            max_batch,
            window: 6,
            roots: 6,
            stack_allowed: true,
            solo_cost_s: 1500.0,
            root_discards: false,
            root_stack_min_pass_percent: 0,
        }
    }

    fn candidates(ps: &[f64]) -> Vec<Candidate> {
        ps.iter()
            .enumerate()
            .map(|(i, p)| Candidate {
                pr: i as i64 + 1,
                p: *p,
                conflicts: BTreeSet::new(),
            })
            .collect()
    }

    #[test]
    fn at_p_half_it_chooses_independent_singles() {
        let builds = plan(&candidates(&[0.5; 6]), &caps(4), None);
        assert_eq!(builds.len(), 6);
        assert!(
            builds.iter().all(|b| b.prs.len() == 1 && b.rooted),
            "at p~0.5 every build must be a single-change root: {builds:?}"
        );
    }

    #[test]
    fn at_high_p_it_batches() {
        let builds = plan(&candidates(&[0.95; 6]), &caps(4), None);
        assert!(
            builds.iter().any(|b| b.prs.len() > 1),
            "at high p batching must win: {builds:?}"
        );
    }

    #[test]
    fn abort_only_when_strictly_better_queues() {
        assert_eq!(should_abort(&[0.4, 0.9], 0.5), Some(0));
        assert_eq!(should_abort(&[0.4, 0.9], 0.4), None);
        assert_eq!(should_abort(&[], 1.0), None);
    }

    #[test]
    fn likely_landing_needs_the_feature_on() {
        assert_eq!(likely_landing_reason(1.0, true, true, 0), None);
        assert_eq!(
            likely_landing_reason(0.1, false, true, 80),
            Some("already green")
        );
        assert_eq!(
            likely_landing_reason(0.1, true, false, 80),
            Some("prioritized")
        );
        assert_eq!(
            likely_landing_reason(0.8, false, false, 80),
            Some("predicted pass"),
            "the threshold is inclusive"
        );
        assert_eq!(likely_landing_reason(0.79, false, false, 80), None);
    }

    #[test]
    fn simulate_reproduces_two_root_discards_and_stacking_rescues_them() {
        // Three certain changes as single-change roots, window 3: the
        // first lands and moves the base under the other two; the second
        // then lands and moves it under the third again - the third
        // change is discarded twice before it lands, as in #1071.
        let truth: BTreeMap<i64, f64> = (1..=3).map(|pr| (pr, 1.0)).collect();
        let order: Vec<i64> = (1..=3).collect();
        let mut base_caps = caps(1);
        base_caps.window = 3;
        base_caps.root_discards = true;
        let rooted = simulate(&truth, &order, Policy::Kroots, &base_caps, &truth, None, 7);
        assert_eq!(rooted.landed, 3);
        assert_eq!(
            rooted.discarded_roots, 3,
            "epoch 1 discards roots 2+3, epoch 2 discards root 3 again"
        );
        assert!(rooted.wasted_hours > 0.0);
        let mut stacked_caps = base_caps;
        stacked_caps.root_stack_min_pass_percent = 50;
        let stacked = simulate(
            &truth,
            &order,
            Policy::Kroots,
            &stacked_caps,
            &truth,
            None,
            7,
        );
        assert_eq!(stacked.landed, 3);
        assert_eq!(stacked.discarded_roots, 0);
        assert_eq!(stacked.wasted_hours, 0.0);
        assert!(
            stacked.validate_hours < rooted.validate_hours,
            "stacking {}h must beat rooting {}h",
            stacked.validate_hours,
            rooted.validate_hours
        );
        // Gated off, simulate keeps its historical semantics exactly:
        // roots are usable and nothing is counted as discarded.
        let historical = simulate(&truth, &order, Policy::Kroots, &caps(1), &truth, None, 7);
        assert_eq!(historical.discarded_roots, 0);
        assert_eq!(historical.validate_hours, stacked.validate_hours);
    }

    #[test]
    fn ev_beats_optimistic_at_p_half_in_simulation() {
        let truth: BTreeMap<i64, f64> = (1..=12).map(|pr| (pr, 0.5)).collect();
        let order: Vec<i64> = (1..=12).collect();
        let mut ev_total = 0.0;
        let mut opt_total = 0.0;
        for seed in 1..=40u64 {
            let ev = simulate(&truth, &order, Policy::Ev, &caps(4), &truth, None, seed);
            let opt = simulate(
                &truth,
                &order,
                Policy::Optimistic,
                &caps(4),
                &truth,
                None,
                seed,
            );
            ev_total += ev.landed_per_hour;
            opt_total += opt.landed_per_hour;
        }
        assert!(
            ev_total > opt_total,
            "EV {ev_total} must beat optimistic {opt_total} at p=0.5"
        );
    }
}
