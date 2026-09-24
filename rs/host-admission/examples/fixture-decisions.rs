//! Emit canonical decisions for the shared fixture corpus.

use host_admission::{decide, AdmissionPolicy, HostSnapshot, OwnerState, ResourceRequest};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;

#[derive(Deserialize)]
struct Fixtures {
    schema: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    now_unix_ms: u64,
    snapshot: HostSnapshot,
    policy: AdmissionPolicy,
    request: ResourceRequest,
    leases: Vec<LeaseInput>,
    queue: Vec<QueueInput>,
    owner_states: BTreeMap<String, String>,
    expect: Expect,
}

#[derive(Deserialize)]
struct LeaseInput {
    lease_id: String,
    request: ResourceRequest,
}

#[derive(Deserialize)]
struct QueueInput {
    sequence: u64,
    request: ResourceRequest,
}

#[derive(Deserialize)]
struct Expect {
    verdict: String,
    code: String,
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: fixture-decisions FIXTURES.json");
    let text = fs::read_to_string(path).expect("read fixture corpus");
    let fixtures: Fixtures = serde_json::from_str(&text).expect("parse fixture corpus");
    assert_eq!(fixtures.schema, "host-admission-fixtures/v1");
    for case in fixtures.cases {
        let leases: Vec<_> = case
            .leases
            .into_iter()
            .map(|lease| (lease.lease_id, lease.request))
            .collect();
        let queue: Vec<_> = case
            .queue
            .into_iter()
            .map(|entry| (entry.sequence, entry.request))
            .collect();
        let states = case
            .owner_states
            .into_iter()
            .map(|(key, value)| {
                let state = match value.as_str() {
                    "alive" => OwnerState::Alive,
                    "absent" => OwnerState::Absent,
                    "unknown" => OwnerState::Unknown,
                    other => panic!("unknown owner state {other}"),
                };
                (key, state)
            })
            .collect();
        let decision = decide(
            &case.request,
            &case.snapshot,
            &case.policy,
            &leases,
            &queue,
            case.now_unix_ms,
            &states,
        )
        .expect("decide fixture");
        assert_eq!(decision.verdict.as_ref(), case.expect.verdict);
        assert_eq!(decision.code, case.expect.code);
        print!("{}\t{}", case.name, decision.canonical_json());
    }
}

trait VerdictText {
    fn as_ref(&self) -> &'static str;
}

impl VerdictText for host_admission::Verdict {
    fn as_ref(&self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Queue => "queue",
            Self::Refuse => "refuse",
            Self::Unknown => "unknown",
        }
    }
}
