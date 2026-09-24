//! Validate a Python-written ledger and emit the same state from Rust.

use host_admission::{
    AdmissionPolicy, HostAdmissionLedger, HostSnapshot, OwnerState, ProcessOwner, ResourceRequest,
    SwapMode, Verdict,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let python_path = PathBuf::from(arguments.next().expect("python ledger path"));
    let rust_path = PathBuf::from(arguments.next().expect("rust ledger path"));
    assert!(arguments.next().is_none(), "unexpected argument");
    HostAdmissionLedger::new(python_path)
        .validate()
        .expect("validate Python-written ledger");

    let owner = ProcessOwner {
        host_id: "host-a".to_owned(),
        boot_id: "boot-a".to_owned(),
        pid: 10,
        start_ticks: 100,
    };
    let request = ResourceRequest {
        request_id: "cross-edition".to_owned(),
        caller: "parity-test".to_owned(),
        owner: owner.clone(),
        memory_bytes: 123,
        named_tokens: BTreeMap::from([("build".to_owned(), 1)]),
        priority: 7,
        metadata_digest: "fixture".to_owned(),
    };
    let snapshot = HostSnapshot {
        captured_at_unix_ms: 1000,
        host_id: owner.host_id.clone(),
        boot_id: owner.boot_id.clone(),
        mem_total_bytes: Some(1000),
        mem_available_bytes: Some(1000),
        swap_total_bytes: Some(0),
        swap_free_bytes: Some(0),
        memory_psi_some_avg10_micros: Some(0),
        memory_psi_full_avg10_micros: Some(0),
        errors: Vec::new(),
    };
    let policy = AdmissionPolicy {
        memory_budget_bytes: Some(700),
        memory_reserve_bytes: 0,
        token_capacities: BTreeMap::from([("build".to_owned(), 1)]),
        swap_mode: SwapMode::Disabled,
        swap_floor_fraction_ppm: 150_000,
        largest_leaf_swap_bytes: 0,
        no_swap_extra_memory_reserve_bytes: 0,
        psi_some_max_micros: None,
        psi_full_max_micros: None,
        max_snapshot_age_ms: 100,
    };
    let (decision, _lease) = HostAdmissionLedger::new(rust_path)
        .request(&request, &snapshot, &policy, 1010, |_, _| OwnerState::Alive)
        .expect("write Rust ledger");
    assert_eq!(decision.verdict, Verdict::Grant);
}
