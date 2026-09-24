//! Non-authoritative comparison bridge for the current memory-admission gate.
//!
//! The current Dagrun ledger remains authoritative. This bridge performs no
//! I/O and acquires no generic lease; it only exposes a shadow verdict.

use crate::admission::Verdict as LegacyVerdict;
use host_admission::{
    decide, AdmissionPolicy, HostSnapshot, OwnerState, ProcessOwner, ResourceRequest, SwapMode,
    Verdict,
};
use std::collections::BTreeMap;

/// Current authority beside a non-mutating generic shadow result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowComparison {
    /// Existing Dagrun verdict; this remains authoritative.
    pub authoritative_verdict: LegacyVerdict,
    /// Generic shadow verdict.
    pub shadow_verdict: Verdict,
    /// Generic machine-readable reason.
    pub shadow_code: String,
}

/// Preserve the old call surface without inventing missing host measurements.
///
/// The authoritative decision remains meaningful; the shadow result is
/// `UNKNOWN` until the caller uses
/// [`compare_legacy_memory_decision_with_snapshot`] with a real observation.
pub fn compare_legacy_memory_decision(
    requested_bytes: u64,
    budget_bytes: Option<u64>,
    headroom_bytes: Option<u64>,
    reserved_bytes: u64,
) -> Result<ShadowComparison, host_admission::AdmissionError> {
    compare_legacy_memory_decision_impl(
        requested_bytes,
        budget_bytes,
        headroom_bytes,
        reserved_bytes,
        None,
    )
}

/// Compare using a real timestamped host observation without changing authority.
pub fn compare_legacy_memory_decision_with_snapshot(
    requested_bytes: u64,
    budget_bytes: Option<u64>,
    headroom_bytes: Option<u64>,
    reserved_bytes: u64,
    snapshot: &HostSnapshot,
    observed_at_unix_ms: u64,
) -> Result<ShadowComparison, host_admission::AdmissionError> {
    compare_legacy_memory_decision_impl(
        requested_bytes,
        budget_bytes,
        headroom_bytes,
        reserved_bytes,
        Some((snapshot, observed_at_unix_ms)),
    )
}

fn compare_legacy_memory_decision_impl(
    requested_bytes: u64,
    budget_bytes: Option<u64>,
    headroom_bytes: Option<u64>,
    reserved_bytes: u64,
    observation: Option<(&HostSnapshot, u64)>,
) -> Result<ShadowComparison, host_admission::AdmissionError> {
    let authoritative_verdict = if budget_bytes.is_some_and(|budget| requested_bytes > budget) {
        LegacyVerdict::Refuse
    } else if budget_bytes
        .is_some_and(|budget| reserved_bytes.saturating_add(requested_bytes) > budget)
        || headroom_bytes.is_some_and(|headroom| requested_bytes > headroom)
    {
        LegacyVerdict::Queue
    } else {
        LegacyVerdict::Grant
    };
    let effective_snapshot = observation.map_or_else(
        || HostSnapshot {
            captured_at_unix_ms: 1,
            host_id: "shadow-host".to_owned(),
            boot_id: "shadow-boot".to_owned(),
            mem_total_bytes: None,
            mem_available_bytes: None,
            swap_total_bytes: None,
            swap_free_bytes: None,
            memory_psi_some_avg10_micros: None,
            memory_psi_full_avg10_micros: None,
            errors: Vec::new(),
        },
        |(snapshot, _)| snapshot.clone(),
    );
    let observed_at_unix_ms = observation.map_or(1, |(_, observed_at)| observed_at);
    let owner = ProcessOwner {
        host_id: effective_snapshot.host_id.clone(),
        boot_id: effective_snapshot.boot_id.clone(),
        pid: 1,
        start_ticks: 1,
    };
    let policy = AdmissionPolicy {
        memory_budget_bytes: budget_bytes,
        memory_reserve_bytes: 0,
        token_capacities: BTreeMap::new(),
        swap_mode: SwapMode::Disabled,
        swap_floor_fraction_ppm: 150_000,
        largest_leaf_swap_bytes: 0,
        no_swap_extra_memory_reserve_bytes: 0,
        psi_some_max_micros: None,
        psi_full_max_micros: None,
        max_snapshot_age_ms: 30_000,
    };
    let request = ResourceRequest {
        request_id: "shadow-request".to_owned(),
        caller: "dagrun-shadow".to_owned(),
        owner: owner.clone(),
        memory_bytes: requested_bytes,
        named_tokens: BTreeMap::new(),
        priority: 0,
        metadata_digest: "none".to_owned(),
    };
    let leases = if reserved_bytes == 0 {
        Vec::new()
    } else {
        vec![(
            "shadow-peer".to_owned(),
            ResourceRequest {
                request_id: "shadow-peer".to_owned(),
                caller: "dagrun-shadow".to_owned(),
                owner,
                memory_bytes: reserved_bytes,
                named_tokens: BTreeMap::new(),
                priority: 0,
                metadata_digest: "none".to_owned(),
            },
        )]
    };
    let owner_states = leases
        .iter()
        .map(|(id, _)| (id.clone(), OwnerState::Alive))
        .collect();
    let shadow = decide(
        &request,
        &effective_snapshot,
        &policy,
        &leases,
        &[],
        observed_at_unix_ms,
        &owner_states,
    )?;
    Ok(ShadowComparison {
        authoritative_verdict,
        shadow_verdict: shadow.verdict,
        shadow_code: shadow.code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_authority_is_unchanged_and_unknown_is_visible_only_in_shadow() {
        let cases = [
            (701, Some(700), Some(1000), 0, LegacyVerdict::Refuse),
            (400, Some(700), Some(1000), 400, LegacyVerdict::Queue),
            (400, Some(700), Some(399), 0, LegacyVerdict::Queue),
            (400, Some(700), Some(400), 0, LegacyVerdict::Grant),
            (1 << 40, None, None, 0, LegacyVerdict::Grant),
        ];
        for (requested, budget, headroom, reserved, expected) in cases {
            let observation = budget.zip(headroom).map(|(budget, headroom)| HostSnapshot {
                captured_at_unix_ms: 1000,
                host_id: "host-a".to_owned(),
                boot_id: "boot-a".to_owned(),
                mem_total_bytes: Some(budget.max(headroom)),
                mem_available_bytes: Some(headroom),
                swap_total_bytes: Some(0),
                swap_free_bytes: Some(0),
                memory_psi_some_avg10_micros: Some(0),
                memory_psi_full_avg10_micros: Some(0),
                errors: Vec::new(),
            });
            let comparison = observation
                .as_ref()
                .map_or_else(
                    || compare_legacy_memory_decision(requested, budget, headroom, reserved),
                    |snapshot| {
                        compare_legacy_memory_decision_with_snapshot(
                            requested, budget, headroom, reserved, snapshot, 1010,
                        )
                    },
                )
                .expect("shadow comparison");
            assert_eq!(comparison.authoritative_verdict, expected);
            assert_eq!(
                comparison.shadow_verdict,
                match (observation.is_none(), expected) {
                    (true, _) => Verdict::Unknown,
                    (false, LegacyVerdict::Grant) => Verdict::Grant,
                    (false, LegacyVerdict::Queue) => Verdict::Queue,
                    (false, LegacyVerdict::Refuse) => Verdict::Refuse,
                }
            );
        }
        assert_eq!(
            compare_legacy_memory_decision(1 << 40, None, None, 0)
                .expect("shadow comparison")
                .shadow_verdict,
            Verdict::Unknown
        );
        let stale = HostSnapshot {
            captured_at_unix_ms: 1,
            host_id: "host-a".to_owned(),
            boot_id: "boot-a".to_owned(),
            mem_total_bytes: Some(1000),
            mem_available_bytes: Some(1000),
            swap_total_bytes: Some(0),
            swap_free_bytes: Some(0),
            memory_psi_some_avg10_micros: Some(0),
            memory_psi_full_avg10_micros: Some(0),
            errors: Vec::new(),
        };
        let stale_comparison = compare_legacy_memory_decision_with_snapshot(
            1,
            Some(700),
            Some(700),
            0,
            &stale,
            30_002,
        )
        .expect("stale shadow comparison");
        assert_eq!(stale_comparison.authoritative_verdict, LegacyVerdict::Grant);
        assert_eq!(stale_comparison.shadow_verdict, Verdict::Unknown);
    }
}
