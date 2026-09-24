//! Generic shared-host resource admission.
//!
//! This crate owns only host observation, exact process-owner liveness, queue
//! ordering, and atomic memory/named-token leases. Callers retain repository,
//! validation, agent lifecycle, and evidence policy.

use fs2::FileExt;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Ledger schema written by this crate.
pub const LEDGER_SCHEMA: &str = "host-admission-ledger/v1";
/// Snapshot schema understood by paired implementations.
pub const SNAPSHOT_SCHEMA: &str = "host-admission-snapshot/v1";
/// Decision schema emitted by paired implementations.
pub const DECISION_SCHEMA: &str = "host-admission-decision/v1";
/// Parts per million used for ratios and PSI percentages.
pub const PPM: u64 = 1_000_000;
const MAX_LEDGER_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 16_384;
const MAX_BYTES: u64 = 1 << 62;
static TEMP_NONCE: AtomicU64 = AtomicU64::new(0);

fn deserialize_token_map<'de, D>(deserializer: D) -> Result<BTreeMap<String, u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct TokenMapVisitor;

    impl<'de> Visitor<'de> for TokenMapVisitor {
        type Value = BTreeMap<String, u32>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a named-token map with unique keys")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut result = BTreeMap::new();
            while let Some((name, count)) = access.next_entry::<String, u32>()? {
                if result.insert(name, count).is_some() {
                    return Err(serde::de::Error::custom("duplicate named-token key"));
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_map(TokenMapVisitor)
}

/// A durable-ledger or measurement error.
#[derive(Debug)]
pub struct AdmissionError(String);

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AdmissionError {}

/// Four distinct admission outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The complete request was leased atomically.
    Grant,
    /// The request can fit later but is currently blocked.
    Queue,
    /// Waiting can never make this request valid under the policy.
    Refuse,
    /// A required observation could not be established.
    Unknown,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Queue => "queue",
            Self::Refuse => "refuse",
            Self::Unknown => "unknown",
        }
    }
}

/// Observation of one exact process owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerState {
    /// The exact owner is alive.
    Alive,
    /// Positive evidence proves that exact owner absent.
    Absent,
    /// Owner liveness could not be established.
    Unknown,
}

/// Explicit swap-policy branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwapMode {
    /// Require free swap at the configured computed floor.
    Floor,
    /// Require a host with zero swap and reserve extra memory.
    NoSwap,
    /// Deliberately do not enforce swap; measurements remain telemetry.
    Disabled,
}

/// Exact process ownership bound to one host and boot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessOwner {
    /// Stable host identity.
    pub host_id: String,
    /// Kernel boot identity.
    pub boot_id: String,
    /// Process identifier.
    pub pid: u32,
    /// `/proc/PID/stat` start-time ticks.
    pub start_ticks: u64,
}

/// One fresh host measurement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostSnapshot {
    /// Capture time in Unix milliseconds.
    pub captured_at_unix_ms: u64,
    /// Stable host identity.
    pub host_id: String,
    /// Kernel boot identity.
    pub boot_id: String,
    /// Physical memory total.
    pub mem_total_bytes: Option<u64>,
    /// Kernel MemAvailable estimate.
    pub mem_available_bytes: Option<u64>,
    /// Configured swap total.
    pub swap_total_bytes: Option<u64>,
    /// Currently free swap.
    pub swap_free_bytes: Option<u64>,
    /// Memory PSI `some avg10`, in millionths of one percent.
    pub memory_psi_some_avg10_micros: Option<u64>,
    /// Memory PSI `full avg10`, in millionths of one percent.
    pub memory_psi_full_avg10_micros: Option<u64>,
    /// Sanitized measurement error codes.
    pub errors: Vec<String>,
}

/// Memory and named tokens acquired as one indivisible request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceRequest {
    /// Stable idempotency identifier.
    pub request_id: String,
    /// Sanitized caller class.
    pub caller: String,
    /// Exact process owner.
    pub owner: ProcessOwner,
    /// Requested committed memory.
    pub memory_bytes: u64,
    /// Named token counts, acquired atomically with memory.
    #[serde(deserialize_with = "deserialize_token_map")]
    pub named_tokens: BTreeMap<String, u32>,
    /// Higher numeric values run before lower values; FIFO within a value.
    pub priority: i32,
    /// Digest of caller-private metadata, not the metadata itself.
    pub metadata_digest: String,
}

/// Caller-owned resource policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionPolicy {
    /// Explicit memory budget, or a conservative host-derived default.
    pub memory_budget_bytes: Option<u64>,
    /// Memory retained outside admitted work.
    pub memory_reserve_bytes: u64,
    /// Named-token capacities.
    #[serde(deserialize_with = "deserialize_token_map")]
    pub token_capacities: BTreeMap<String, u32>,
    /// Explicit swap branch.
    pub swap_mode: SwapMode,
    /// Free-swap fraction in parts per million.
    pub swap_floor_fraction_ppm: u64,
    /// Largest measured leaf swap use considered by the floor formula.
    pub largest_leaf_swap_bytes: u64,
    /// Extra memory reserve required on an explicitly swapless host.
    pub no_swap_extra_memory_reserve_bytes: u64,
    /// Optional PSI-some enforcement threshold.
    pub psi_some_max_micros: Option<u64>,
    /// Optional PSI-full enforcement threshold.
    pub psi_full_max_micros: Option<u64>,
    /// Maximum accepted snapshot age.
    pub max_snapshot_age_ms: u64,
}

/// One sanitized structured admission decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Decision {
    /// Four-way verdict.
    pub verdict: Verdict,
    /// Stable machine-readable reason code.
    pub code: String,
    /// Opaque request identifier.
    pub request_id: String,
    /// Live admitted memory before this request.
    pub reserved_memory_bytes: u64,
    /// Effective aggregate memory budget.
    pub memory_budget_bytes: Option<u64>,
    /// Effective current live headroom.
    pub memory_headroom_bytes: Option<u64>,
    /// Effective swap floor, when enabled.
    pub swap_floor_bytes: Option<u64>,
    /// Available named tokens before this request.
    pub tokens_available: BTreeMap<String, u32>,
    /// Bounded, sanitized blocker identifiers.
    pub blockers: Vec<String>,
    /// Durable FIFO sequence when queued.
    pub queue_sequence: Option<u64>,
    /// Lease identifier when granted.
    pub lease_id: Option<String>,
}

impl Decision {
    /// Emit canonical sorted compact JSON plus one final newline.
    pub fn canonical_json(&self) -> String {
        let mut object = BTreeMap::<String, Value>::new();
        object.insert(
            "blockers".to_owned(),
            serde_json::to_value(&self.blockers).unwrap(),
        );
        object.insert("code".to_owned(), Value::String(self.code.clone()));
        object.insert(
            "lease_id".to_owned(),
            serde_json::to_value(&self.lease_id).unwrap(),
        );
        object.insert(
            "memory_budget_bytes".to_owned(),
            serde_json::to_value(self.memory_budget_bytes).unwrap(),
        );
        object.insert(
            "memory_headroom_bytes".to_owned(),
            serde_json::to_value(self.memory_headroom_bytes).unwrap(),
        );
        object.insert(
            "queue_sequence".to_owned(),
            serde_json::to_value(self.queue_sequence).unwrap(),
        );
        object.insert(
            "request_id".to_owned(),
            Value::String(self.request_id.clone()),
        );
        object.insert(
            "reserved_memory_bytes".to_owned(),
            Value::from(self.reserved_memory_bytes),
        );
        object.insert(
            "schema".to_owned(),
            Value::String(DECISION_SCHEMA.to_owned()),
        );
        object.insert(
            "swap_floor_bytes".to_owned(),
            serde_json::to_value(self.swap_floor_bytes).unwrap(),
        );
        object.insert(
            "tokens_available".to_owned(),
            serde_json::to_value(&self.tokens_available).unwrap(),
        );
        object.insert(
            "verdict".to_owned(),
            Value::String(self.verdict.as_str().to_owned()),
        );
        format!("{}\n", serde_json::to_string(&object).unwrap())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LeaseRecord {
    granted_at_unix_ms: u64,
    lease_id: String,
    request: ResourceRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueueRecord {
    request: ResourceRequest,
    sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LedgerData {
    leases: Vec<LeaseRecord>,
    next_sequence: u64,
    queue: Vec<QueueRecord>,
    schema: String,
}

impl LedgerData {
    fn empty() -> Self {
        Self {
            leases: Vec::new(),
            next_sequence: 0,
            queue: Vec::new(),
            schema: LEDGER_SCHEMA.to_owned(),
        }
    }
}

fn canonical_owner_value(owner: &ProcessOwner) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("boot_id".to_owned(), Value::String(owner.boot_id.clone()));
    object.insert("host_id".to_owned(), Value::String(owner.host_id.clone()));
    object.insert("pid".to_owned(), Value::from(owner.pid));
    object.insert("start_ticks".to_owned(), Value::from(owner.start_ticks));
    Value::Object(object)
}

fn canonical_request_value(request: &ResourceRequest) -> Value {
    let mut tokens = serde_json::Map::new();
    for (name, count) in &request.named_tokens {
        tokens.insert(name.clone(), Value::from(*count));
    }
    let mut object = serde_json::Map::new();
    object.insert("caller".to_owned(), Value::String(request.caller.clone()));
    object.insert("memory_bytes".to_owned(), Value::from(request.memory_bytes));
    object.insert(
        "metadata_digest".to_owned(),
        Value::String(request.metadata_digest.clone()),
    );
    object.insert("named_tokens".to_owned(), Value::Object(tokens));
    object.insert("owner".to_owned(), canonical_owner_value(&request.owner));
    object.insert("priority".to_owned(), Value::from(request.priority));
    object.insert(
        "request_id".to_owned(),
        Value::String(request.request_id.clone()),
    );
    Value::Object(object)
}

fn canonical_ledger_value(data: &LedgerData) -> Value {
    let leases = data
        .leases
        .iter()
        .map(|lease| {
            let mut object = serde_json::Map::new();
            object.insert(
                "granted_at_unix_ms".to_owned(),
                Value::from(lease.granted_at_unix_ms),
            );
            object.insert("lease_id".to_owned(), Value::String(lease.lease_id.clone()));
            object.insert(
                "request".to_owned(),
                canonical_request_value(&lease.request),
            );
            Value::Object(object)
        })
        .collect();
    let queue = data
        .queue
        .iter()
        .map(|entry| {
            let mut object = serde_json::Map::new();
            object.insert(
                "request".to_owned(),
                canonical_request_value(&entry.request),
            );
            object.insert("sequence".to_owned(), Value::from(entry.sequence));
            Value::Object(object)
        })
        .collect();
    let mut object = serde_json::Map::new();
    object.insert("leases".to_owned(), Value::Array(leases));
    object.insert("next_sequence".to_owned(), Value::from(data.next_sequence));
    object.insert("queue".to_owned(), Value::Array(queue));
    object.insert("schema".to_owned(), Value::String(data.schema.clone()));
    Value::Object(object)
}

fn safe_id(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && value.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

fn validate_request(request: &ResourceRequest) -> Result<(), AdmissionError> {
    if !safe_id(&request.request_id)
        || !safe_id(&request.caller)
        || !safe_id(&request.metadata_digest)
        || !safe_id(&request.owner.host_id)
        || !safe_id(&request.owner.boot_id)
        || request.owner.pid == 0
        || request.owner.start_ticks == 0
        || request.memory_bytes > MAX_BYTES
        || request
            .named_tokens
            .iter()
            .any(|(name, count)| !safe_id(name) || *count == 0)
    {
        return Err(AdmissionError("invalid resource request".to_owned()));
    }
    Ok(())
}

fn validate_policy(policy: &AdmissionPolicy) -> Result<(), AdmissionError> {
    if policy
        .memory_budget_bytes
        .is_some_and(|value| value > MAX_BYTES)
        || policy.memory_reserve_bytes > MAX_BYTES
        || policy.largest_leaf_swap_bytes > MAX_BYTES
        || policy.no_swap_extra_memory_reserve_bytes > MAX_BYTES
        || policy.swap_floor_fraction_ppm > PPM
        || policy
            .psi_some_max_micros
            .is_some_and(|value| value > 100 * PPM)
        || policy
            .psi_full_max_micros
            .is_some_and(|value| value > 100 * PPM)
        || policy
            .token_capacities
            .iter()
            .any(|(name, _)| !safe_id(name))
    {
        return Err(AdmissionError("invalid admission policy".to_owned()));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &HostSnapshot) -> Result<(), AdmissionError> {
    let bounded_measurements = [
        snapshot.mem_total_bytes,
        snapshot.mem_available_bytes,
        snapshot.swap_total_bytes,
        snapshot.swap_free_bytes,
    ];
    if !safe_id(&snapshot.host_id)
        || !safe_id(&snapshot.boot_id)
        || bounded_measurements
            .iter()
            .flatten()
            .any(|value| *value > MAX_BYTES)
        || snapshot
            .memory_psi_some_avg10_micros
            .is_some_and(|value| value > 100 * PPM)
        || snapshot
            .memory_psi_full_avg10_micros
            .is_some_and(|value| value > 100 * PPM)
        || snapshot.errors.iter().any(|error| !safe_id(error))
        || matches!(
            (snapshot.mem_total_bytes, snapshot.mem_available_bytes),
            (Some(total), Some(available)) if available > total
        )
        || matches!(
            (snapshot.swap_total_bytes, snapshot.swap_free_bytes),
            (Some(total), Some(free)) if free > total
        )
    {
        return Err(AdmissionError("invalid host snapshot".to_owned()));
    }
    Ok(())
}

fn unknown(request: &ResourceRequest, code: &str, blockers: Vec<String>) -> Decision {
    Decision {
        verdict: Verdict::Unknown,
        code: code.to_owned(),
        request_id: request.request_id.clone(),
        reserved_memory_bytes: 0,
        memory_budget_bytes: None,
        memory_headroom_bytes: None,
        swap_floor_bytes: None,
        tokens_available: BTreeMap::new(),
        blockers,
        queue_sequence: None,
        lease_id: None,
    }
}

struct DecisionResources {
    reserved: u64,
    budget: Option<u64>,
    headroom: Option<u64>,
    swap_floor: Option<u64>,
    tokens_available: BTreeMap<String, u32>,
}

fn resources(
    reserved: u64,
    budget: Option<u64>,
    headroom: Option<u64>,
    swap_floor: Option<u64>,
    tokens_available: BTreeMap<String, u32>,
) -> DecisionResources {
    DecisionResources {
        reserved,
        budget,
        headroom,
        swap_floor,
        tokens_available,
    }
}

fn answer(
    verdict: Verdict,
    code: &str,
    request: &ResourceRequest,
    resources: DecisionResources,
    blockers: Vec<String>,
    queue_sequence: Option<u64>,
    lease_id: Option<String>,
) -> Decision {
    Decision {
        verdict,
        code: code.to_owned(),
        request_id: request.request_id.clone(),
        reserved_memory_bytes: resources.reserved,
        memory_budget_bytes: resources.budget,
        memory_headroom_bytes: resources.headroom,
        swap_floor_bytes: resources.swap_floor,
        tokens_available: resources.tokens_available,
        blockers,
        queue_sequence,
        lease_id,
    }
}

/// Decide one request from already observed state without mutating it.
pub fn decide(
    request: &ResourceRequest,
    snapshot: &HostSnapshot,
    policy: &AdmissionPolicy,
    leases: &[(String, ResourceRequest)],
    queue: &[(u64, ResourceRequest)],
    now_unix_ms: u64,
    owner_states: &BTreeMap<String, OwnerState>,
) -> Result<Decision, AdmissionError> {
    validate_request(request)?;
    validate_policy(policy)?;
    validate_snapshot(snapshot)?;
    let mut ledger_ids = BTreeSet::new();
    let mut queue_sequences = BTreeSet::new();
    for (lease_id, lease) in leases {
        validate_request(lease)?;
        if !safe_id(lease_id)
            || lease_id != &lease.request_id
            || !ledger_ids.insert(lease_id.clone())
        {
            return Err(AdmissionError("invalid lease input".to_owned()));
        }
    }
    for (sequence, queued) in queue {
        validate_request(queued)?;
        if !ledger_ids.insert(queued.request_id.clone()) || !queue_sequences.insert(*sequence) {
            return Err(AdmissionError("invalid queue input".to_owned()));
        }
    }
    let mut missing = BTreeSet::<String>::new();
    if now_unix_ms < snapshot.captured_at_unix_ms
        || now_unix_ms - snapshot.captured_at_unix_ms > policy.max_snapshot_age_ms
    {
        missing.insert("snapshot_stale".to_owned());
    }
    for error in &snapshot.errors {
        if error == "memory_psi_unreadable"
            && policy.psi_some_max_micros.is_none()
            && policy.psi_full_max_micros.is_none()
            || error.starts_with("memory_psi_some") && policy.psi_some_max_micros.is_none()
            || error.starts_with("memory_psi_full") && policy.psi_full_max_micros.is_none()
            || error.starts_with("swap_") && policy.swap_mode == SwapMode::Disabled
        {
            continue;
        }
        missing.insert(format!("snapshot_error:{error}"));
    }
    if snapshot.mem_total_bytes.is_none() {
        missing.insert("mem_total_missing".to_owned());
    }
    if snapshot.mem_available_bytes.is_none() {
        missing.insert("mem_available_missing".to_owned());
    }
    if policy.swap_mode != SwapMode::Disabled {
        if snapshot.swap_total_bytes.is_none() {
            missing.insert("swap_total_missing".to_owned());
        }
        if snapshot.swap_free_bytes.is_none() {
            missing.insert("swap_free_missing".to_owned());
        }
    }
    if policy.psi_some_max_micros.is_some() && snapshot.memory_psi_some_avg10_micros.is_none() {
        missing.insert("psi_some_missing".to_owned());
    }
    if policy.psi_full_max_micros.is_some() && snapshot.memory_psi_full_avg10_micros.is_none() {
        missing.insert("psi_full_missing".to_owned());
    }
    if request.owner.host_id != snapshot.host_id || request.owner.boot_id != snapshot.boot_id {
        missing.insert("request_owner_host_or_boot_mismatch".to_owned());
    }
    if leases.iter().any(|(id, _)| {
        owner_states.get(id).copied().unwrap_or(OwnerState::Unknown) == OwnerState::Unknown
    }) || queue.iter().any(|(_, queued)| {
        queued.request_id != request.request_id
            && owner_states
                .get(&queued.request_id)
                .copied()
                .unwrap_or(OwnerState::Unknown)
                == OwnerState::Unknown
    }) {
        missing.insert("owner_unknown".to_owned());
    }
    if !missing.is_empty() {
        return Ok(unknown(
            request,
            "required_observation_unknown",
            missing.into_iter().collect(),
        ));
    }
    let live: Vec<_> = leases
        .iter()
        .filter(|(id, _)| owner_states.get(id) == Some(&OwnerState::Alive))
        .collect();
    let mem_total = snapshot.mem_total_bytes.unwrap();
    let mem_available = snapshot.mem_available_bytes.unwrap();
    let budget = policy.memory_budget_bytes.unwrap_or_else(|| {
        let margin = (8 * 1024_u64.pow(3)).min(mem_total / 8);
        ((u128::from(mem_total) * u128::from(850_000_u64)) / u128::from(PPM)) as u64 - margin
    });
    let mut reserve = policy.memory_reserve_bytes;
    let mut swap_floor = None;
    match policy.swap_mode {
        SwapMode::NoSwap => {
            let total = snapshot.swap_total_bytes.unwrap();
            let free = snapshot.swap_free_bytes.unwrap();
            if total != 0 || free != 0 {
                return Ok(answer(
                    Verdict::Refuse,
                    "no_swap_policy_host_has_swap",
                    request,
                    resources(0, Some(budget), None, None, BTreeMap::new()),
                    Vec::new(),
                    None,
                    None,
                ));
            }
            reserve = reserve.saturating_add(policy.no_swap_extra_memory_reserve_bytes);
        }
        SwapMode::Floor => {
            let total = snapshot.swap_total_bytes.unwrap();
            if total == 0 {
                return Ok(answer(
                    Verdict::Refuse,
                    "swap_required_but_absent",
                    request,
                    resources(0, Some(budget), None, Some(0), BTreeMap::new()),
                    Vec::new(),
                    None,
                    None,
                ));
            }
            let numerator = u128::from(total) * u128::from(policy.swap_floor_fraction_ppm)
                + u128::from(PPM - 1);
            let fraction = (numerator / u128::from(PPM)) as u64;
            swap_floor = Some(fraction.max(policy.largest_leaf_swap_bytes));
        }
        SwapMode::Disabled => {}
    }
    let headroom = mem_available.saturating_sub(reserve);
    let reserved_wide = live
        .iter()
        .map(|(_, lease)| u128::from(lease.memory_bytes))
        .sum::<u128>();
    if reserved_wide > u128::from(MAX_BYTES) {
        return Ok(unknown(
            request,
            "required_observation_unknown",
            vec!["ledger_capacity_invalid".to_owned()],
        ));
    }
    let reserved = reserved_wide as u64;
    let mut used = BTreeMap::<String, u32>::new();
    for name in policy.token_capacities.keys() {
        used.insert(name.clone(), 0);
    }
    for (_, lease) in &live {
        for (name, count) in &lease.named_tokens {
            if let Some(total) = used.get_mut(name) {
                *total = total.saturating_add(*count);
            }
        }
    }
    let available: BTreeMap<_, _> = policy
        .token_capacities
        .iter()
        .map(|(name, capacity)| {
            (
                name.clone(),
                capacity.saturating_sub(*used.get(name).unwrap_or(&0)),
            )
        })
        .collect();
    if request.memory_bytes > budget {
        return Ok(answer(
            Verdict::Refuse,
            "request_exceeds_memory_budget",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            None,
            None,
        ));
    }
    for (name, count) in &request.named_tokens {
        let Some(capacity) = policy.token_capacities.get(name) else {
            return Ok(answer(
                Verdict::Refuse,
                "unknown_named_token",
                request,
                resources(
                    reserved,
                    Some(budget),
                    Some(headroom),
                    swap_floor,
                    available,
                ),
                vec![name.clone()],
                None,
                None,
            ));
        };
        if count > capacity {
            return Ok(answer(
                Verdict::Refuse,
                "request_exceeds_token_capacity",
                request,
                resources(
                    reserved,
                    Some(budget),
                    Some(headroom),
                    swap_floor,
                    available,
                ),
                vec![name.clone()],
                None,
                None,
            ));
        }
    }
    let sequence = queue
        .iter()
        .find(|(_, queued)| queued.request_id == request.request_id)
        .map(|(sequence, _)| *sequence);
    if queue
        .iter()
        .any(|(_, queued)| queued.request_id == request.request_id && queued != request)
    {
        return Ok(unknown(
            request,
            "request_id_conflict",
            vec!["queued_request_differs".to_owned()],
        ));
    }
    let ahead: Vec<String> = queue
        .iter()
        .filter(|(prior_sequence, queued)| {
            queued.request_id != request.request_id
                && owner_states.get(&queued.request_id) == Some(&OwnerState::Alive)
                && (queued.priority > request.priority
                    || queued.priority == request.priority
                        && sequence.is_none_or(|current| *prior_sequence < current))
        })
        .map(|(_, queued)| queued.request_id.clone())
        .collect();
    if !ahead.is_empty() {
        return Ok(answer(
            Verdict::Queue,
            "queued_behind_prior_request",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            ahead,
            sequence,
            None,
        ));
    }
    if reserved.saturating_add(request.memory_bytes) > budget {
        return Ok(answer(
            Verdict::Queue,
            "aggregate_memory_busy",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            sequence,
            None,
        ));
    }
    if request.memory_bytes > headroom {
        return Ok(answer(
            Verdict::Queue,
            "live_memory_busy",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            sequence,
            None,
        ));
    }
    if swap_floor.is_some_and(|floor| snapshot.swap_free_bytes.unwrap() < floor) {
        return Ok(answer(
            Verdict::Queue,
            "swap_floor_not_met",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            sequence,
            None,
        ));
    }
    if policy
        .psi_some_max_micros
        .is_some_and(|threshold| snapshot.memory_psi_some_avg10_micros.unwrap() > threshold)
    {
        return Ok(answer(
            Verdict::Queue,
            "psi_some_above_threshold",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            sequence,
            None,
        ));
    }
    if policy
        .psi_full_max_micros
        .is_some_and(|threshold| snapshot.memory_psi_full_avg10_micros.unwrap() > threshold)
    {
        return Ok(answer(
            Verdict::Queue,
            "psi_full_above_threshold",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            Vec::new(),
            sequence,
            None,
        ));
    }
    let blocked: Vec<String> = request
        .named_tokens
        .iter()
        .filter(|(name, count)| **count > *available.get(*name).unwrap_or(&0))
        .map(|(name, _)| name.clone())
        .collect();
    if !blocked.is_empty() {
        return Ok(answer(
            Verdict::Queue,
            "named_tokens_busy",
            request,
            resources(
                reserved,
                Some(budget),
                Some(headroom),
                swap_floor,
                available,
            ),
            blocked,
            sequence,
            None,
        ));
    }
    Ok(answer(
        Verdict::Grant,
        "resources_available",
        request,
        resources(
            reserved,
            Some(budget),
            Some(headroom),
            swap_floor,
            available,
        ),
        Vec::new(),
        sequence,
        Some(request.request_id.clone()),
    ))
}

fn read_identity(path: &Path, label: &str) -> Result<String, AdmissionError> {
    let value = fs::read_to_string(path)
        .map_err(|_| AdmissionError(format!("could not read {label}")))?
        .trim()
        .to_owned();
    if !safe_id(&value) {
        return Err(AdmissionError(format!("invalid {label}")));
    }
    Ok(value)
}

fn parse_meminfo(text: &str) -> (BTreeMap<String, u64>, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut errors = Vec::new();
    for wanted in ["MemTotal", "MemAvailable", "SwapTotal", "SwapFree"] {
        let rows: Vec<_> = text
            .lines()
            .filter(|line| line.split(':').next() == Some(wanted))
            .collect();
        if rows.len() != 1 {
            errors.push(format!("{}_missing_or_duplicate", camel_to_snake(wanted)));
            continue;
        }
        let parts: Vec<_> = rows[0]
            .split_once(':')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        match parts.as_slice() {
            [raw, "kB"] => {
                let unsigned = raw
                    .strip_prefix('+')
                    .or_else(|| raw.strip_prefix('-'))
                    .unwrap_or(raw);
                let numeric = !unsigned.is_empty()
                    && unsigned.chars().all(|character| character.is_ascii_digit());
                if !numeric {
                    errors.push(format!("{}_malformed", camel_to_snake(wanted)));
                } else {
                    match raw
                        .parse::<u64>()
                        .ok()
                        .and_then(|value| value.checked_mul(1024))
                    {
                        Some(value) if value <= MAX_BYTES => {
                            values.insert(wanted.to_owned(), value);
                        }
                        _ => {
                            errors.push(format!("{}_out_of_range", camel_to_snake(wanted)));
                        }
                    }
                }
            }
            _ => errors.push(format!("{}_malformed", camel_to_snake(wanted))),
        }
    }
    (values, errors)
}

fn camel_to_snake(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn parse_psi(text: &str) -> (Option<u64>, Option<u64>, Vec<String>) {
    let mut values = BTreeMap::new();
    let mut errors = Vec::new();
    for kind in ["some", "full"] {
        let rows: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with(&format!("{kind} ")))
            .collect();
        if rows.len() != 1 {
            errors.push(format!("memory_psi_{kind}_missing_or_duplicate"));
            continue;
        }
        let avg10_fields: Vec<_> = rows[0]
            .split_whitespace()
            .filter_map(|field| field.strip_prefix("avg10="))
            .collect();
        let raw = if avg10_fields.len() == 1 {
            avg10_fields.first().copied()
        } else {
            None
        };
        match raw.and_then(parse_percent_micros) {
            Some(value) if value <= 100 * PPM => {
                values.insert(kind, value);
            }
            _ => errors.push(format!("memory_psi_{kind}_malformed")),
        }
    }
    (
        values.get("some").copied(),
        values.get("full").copied(),
        errors,
    )
}

fn parse_percent_micros(raw: &str) -> Option<u64> {
    let (whole, fraction, had_separator) = match raw.split_once('.') {
        Some((whole, fraction)) => (whole, fraction, true),
        None => (raw, "", false),
    };
    if whole.is_empty()
        || !whole.chars().all(|character| character.is_ascii_digit())
        || had_separator && fraction.is_empty()
    {
        return None;
    }
    let whole = whole.parse::<u64>().ok()?;
    if !fraction.chars().all(|ch| ch.is_ascii_digit())
        || fraction.chars().skip(6).any(|character| character != '0')
    {
        return None;
    }
    let mut padded = fraction.chars().take(6).collect::<String>();
    while padded.len() < 6 {
        padded.push('0');
    }
    whole.checked_mul(PPM)?.checked_add(if padded.is_empty() {
        0
    } else {
        padded.parse().ok()?
    })
}

/// Sample memory, swap, PSI, host identity, and boot identity once.
pub fn sample_host(now_unix_ms: Option<u64>) -> Result<HostSnapshot, AdmissionError> {
    let now = now_unix_ms.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    });
    let (mem, mut errors) = match fs::read_to_string("/proc/meminfo") {
        Ok(text) => parse_meminfo(&text),
        Err(_) => (BTreeMap::new(), vec!["meminfo_unreadable".to_owned()]),
    };
    let (some, full, psi_errors) = match fs::read_to_string("/proc/pressure/memory") {
        Ok(text) => parse_psi(&text),
        Err(_) => (None, None, vec!["memory_psi_unreadable".to_owned()]),
    };
    errors.extend(psi_errors);
    errors.sort();
    let snapshot = HostSnapshot {
        captured_at_unix_ms: now,
        host_id: read_identity(Path::new("/etc/machine-id"), "host_id")?,
        boot_id: read_identity(Path::new("/proc/sys/kernel/random/boot_id"), "boot_id")?,
        mem_total_bytes: mem.get("MemTotal").copied(),
        mem_available_bytes: mem.get("MemAvailable").copied(),
        swap_total_bytes: mem.get("SwapTotal").copied(),
        swap_free_bytes: mem.get("SwapFree").copied(),
        memory_psi_some_avg10_micros: some,
        memory_psi_full_avg10_micros: full,
        errors,
    };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn proc_start_ticks(pid: u32) -> Result<Option<u64>, AdmissionError> {
    let path = format!("/proc/{pid}/stat");
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(AdmissionError("process owner is unobservable".to_owned())),
    };
    let Some(index) = text.rfind(')') else {
        return Err(AdmissionError("process stat is malformed".to_owned()));
    };
    let fields: Vec<_> = text[index + 1..].split_whitespace().collect();
    let raw = fields
        .get(19)
        .ok_or_else(|| AdmissionError("process stat is malformed".to_owned()))?;
    raw.parse::<u64>()
        .map(Some)
        .map_err(|_| AdmissionError("process stat is malformed".to_owned()))
}

/// Prove one exact process owner alive, absent, or unobservable.
pub fn probe_process_owner(owner: &ProcessOwner, snapshot: &HostSnapshot) -> OwnerState {
    if owner.host_id != snapshot.host_id {
        return OwnerState::Unknown;
    }
    if owner.boot_id != snapshot.boot_id {
        return OwnerState::Absent;
    }
    match proc_start_ticks(owner.pid) {
        Ok(Some(start)) if start == owner.start_ticks => OwnerState::Alive,
        Ok(Some(_)) | Ok(None) => OwnerState::Absent,
        Err(_) => OwnerState::Unknown,
    }
}

fn name_cstring(name: &OsStr) -> Result<CString, AdmissionError> {
    CString::new(name.as_bytes()).map_err(|_| AdmissionError("path contains NUL".to_owned()))
}

fn openat_file(
    directory: &File,
    name: &OsStr,
    flags: libc::c_int,
    mode: libc::mode_t,
) -> Result<File, io::Error> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode,
        )
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn open_pinned_parent(path: &Path) -> Result<(File, OsString), AdmissionError> {
    let name = path
        .file_name()
        .ok_or_else(|| AdmissionError("admission ledger has no leaf name".to_owned()))?
        .to_os_string();
    if name.as_bytes().is_empty() || name.as_bytes().contains(&b'/') {
        return Err(AdmissionError(
            "admission ledger has an invalid leaf name".to_owned(),
        ));
    }
    let parent = path.parent().filter(|value| !value.as_os_str().is_empty());
    let absolute = path.is_absolute();
    let start = if absolute {
        Path::new("/")
    } else {
        Path::new(".")
    };
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(start)
        .map_err(|_| AdmissionError("could not open admission path root".to_owned()))?;
    if let Some(parent) = parent {
        for component in parent.components() {
            match component {
                Component::RootDir | Component::CurDir => {}
                Component::Normal(part) => {
                    directory =
                        openat_file(&directory, part, libc::O_RDONLY | libc::O_DIRECTORY, 0)
                            .map_err(|_| {
                                AdmissionError("could not pin admission parent".to_owned())
                            })?;
                }
                Component::ParentDir | Component::Prefix(_) => {
                    return Err(AdmissionError(
                        "admission parent contains an unsafe component".to_owned(),
                    ));
                }
            }
        }
    }
    let metadata = directory
        .metadata()
        .map_err(|_| AdmissionError("could not inspect admission parent".to_owned()))?;
    if !metadata.is_dir() || metadata.uid() != current_uid()? || metadata.mode() & 0o022 != 0 {
        return Err(AdmissionError(
            "admission parent is not an owned non-writable directory".to_owned(),
        ));
    }
    Ok((directory, name))
}

struct LockedLedgerDirectory {
    directory: File,
    ledger_name: OsString,
    lock_name: OsString,
    lock: File,
    directory_identity: (u64, u64),
    lock_identity: (u64, u64),
}

impl LockedLedgerDirectory {
    fn open(path: &Path) -> Result<Self, AdmissionError> {
        let (directory, ledger_name) = open_pinned_parent(path)?;
        let mut lock_name = ledger_name.clone();
        lock_name.push(".lock");
        let lock = openat_file(&directory, &lock_name, libc::O_RDWR | libc::O_CREAT, 0o600)
            .map_err(|_| AdmissionError("could not open admission lock".to_owned()))?;
        let directory_metadata = directory
            .metadata()
            .map_err(|_| AdmissionError("could not inspect admission parent".to_owned()))?;
        let lock_metadata = lock
            .metadata()
            .map_err(|_| AdmissionError("could not inspect admission lock".to_owned()))?;
        if !lock_metadata.is_file()
            || lock_metadata.uid() != current_uid()?
            || lock_metadata.nlink() != 1
            || lock_metadata.mode() & 0o077 != 0
        {
            return Err(AdmissionError(
                "admission lock is not an owned single-link private file".to_owned(),
            ));
        }
        lock.lock_exclusive()
            .map_err(|_| AdmissionError("could not lock admission ledger".to_owned()))?;
        let pinned = Self {
            directory,
            ledger_name,
            lock_name,
            lock,
            directory_identity: (directory_metadata.dev(), directory_metadata.ino()),
            lock_identity: (lock_metadata.dev(), lock_metadata.ino()),
        };
        pinned.verify()?;
        Ok(pinned)
    }

    fn verify(&self) -> Result<(), AdmissionError> {
        let directory = self
            .directory
            .metadata()
            .map_err(|_| AdmissionError("could not reinspect admission parent".to_owned()))?;
        if self.directory_identity != (directory.dev(), directory.ino())
            || !directory.is_dir()
            || directory.uid() != current_uid()?
            || directory.mode() & 0o022 != 0
        {
            return Err(AdmissionError("pinned admission parent changed".to_owned()));
        }
        let held = self
            .lock
            .metadata()
            .map_err(|_| AdmissionError("could not reinspect admission lock".to_owned()))?;
        let named = openat_file(&self.directory, &self.lock_name, libc::O_RDONLY, 0)
            .map_err(|_| AdmissionError("admission lock name changed".to_owned()))?;
        let named = named
            .metadata()
            .map_err(|_| AdmissionError("could not inspect named admission lock".to_owned()))?;
        if self.lock_identity != (held.dev(), held.ino())
            || self.lock_identity != (named.dev(), named.ino())
            || !held.is_file()
            || !named.is_file()
            || held.uid() != current_uid()?
            || named.uid() != current_uid()?
            || held.nlink() != 1
            || named.nlink() != 1
            || held.mode() & 0o077 != 0
            || named.mode() & 0o077 != 0
        {
            return Err(AdmissionError("admission lock identity changed".to_owned()));
        }
        Ok(())
    }

    fn temporary(&self) -> Result<(OsString, File), AdmissionError> {
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..128_u64 {
            let nonce = TEMP_NONCE.fetch_add(1, Ordering::Relaxed);
            let name = OsString::from(format!(
                ".host-admission-{}-{epoch}-{nonce}-{attempt}.tmp",
                std::process::id()
            ));
            match openat_file(
                &self.directory,
                &name,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                0o600,
            ) {
                Ok(file) => return Ok((name, file)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(AdmissionError(
                        "could not create temporary ledger".to_owned(),
                    ));
                }
            }
        }
        Err(AdmissionError(
            "could not create unique temporary ledger".to_owned(),
        ))
    }

    fn rename(&self, source: &OsStr) -> Result<(), AdmissionError> {
        let source = name_cstring(source)?;
        let target = name_cstring(&self.ledger_name)?;
        let result = unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                source.as_ptr(),
                self.directory.as_raw_fd(),
                target.as_ptr(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(AdmissionError(
                "could not replace admission ledger".to_owned(),
            ))
        }
    }

    fn unlink(&self, name: &OsStr) {
        if let Ok(name) = name_cstring(name) {
            unsafe {
                libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0);
            }
        }
    }
}

/// One flock-serialized durable admission ledger.
#[derive(Clone, Debug)]
pub struct HostAdmissionLedger {
    path: PathBuf,
}

impl HostAdmissionLedger {
    /// Bind the ledger to one explicit path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load(&self, locked: &LockedLedgerDirectory) -> Result<LedgerData, AdmissionError> {
        locked.verify()?;
        let file = match openat_file(&locked.directory, &locked.ledger_name, libc::O_RDONLY, 0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(LedgerData::empty()),
            Err(_) => return Err(AdmissionError("could not open admission ledger".to_owned())),
        };
        let metadata = file
            .metadata()
            .map_err(|_| AdmissionError("could not inspect admission ledger".to_owned()))?;
        if !metadata.file_type().is_file()
            || metadata.uid() != current_uid()?
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.len() > MAX_LEDGER_BYTES
        {
            return Err(AdmissionError(
                "admission ledger is not a safe bounded regular file".to_owned(),
            ));
        }
        let mut text = String::new();
        file.take(MAX_LEDGER_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|_| AdmissionError("admission ledger is unreadable".to_owned()))?;
        let data: LedgerData = serde_json::from_str(&text)
            .map_err(|_| AdmissionError("admission ledger is malformed".to_owned()))?;
        if data.schema != LEDGER_SCHEMA || data.leases.len() + data.queue.len() > MAX_RECORDS {
            return Err(AdmissionError(
                "admission ledger has an unsupported schema or size".to_owned(),
            ));
        }
        let mut ids = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut leased_memory = 0_u128;
        for lease in &data.leases {
            validate_request(&lease.request)?;
            if lease.lease_id != lease.request.request_id || !ids.insert(lease.lease_id.clone()) {
                return Err(AdmissionError("duplicate or invalid lease id".to_owned()));
            }
            leased_memory += u128::from(lease.request.memory_bytes);
            if leased_memory > u128::from(MAX_BYTES) {
                return Err(AdmissionError(
                    "admission ledger exceeds the aggregate memory bound".to_owned(),
                ));
            }
        }
        for queued in &data.queue {
            validate_request(&queued.request)?;
            if !ids.insert(queued.request.request_id.clone())
                || !sequences.insert(queued.sequence)
                || queued.sequence >= data.next_sequence
            {
                return Err(AdmissionError(
                    "duplicate or invalid queue record".to_owned(),
                ));
            }
        }
        Ok(data)
    }

    fn store(
        &self,
        locked: &LockedLedgerDirectory,
        data: &LedgerData,
    ) -> Result<(), AdmissionError> {
        locked.verify()?;
        if data.leases.len() + data.queue.len() > MAX_RECORDS {
            return Err(AdmissionError(
                "admission ledger would exceed its record bound".to_owned(),
            ));
        }
        let mut normalized = data.clone();
        normalized
            .leases
            .sort_by(|a, b| a.lease_id.cmp(&b.lease_id));
        normalized.queue.sort_by_key(|entry| entry.sequence);
        let value = canonical_ledger_value(&normalized);
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|_| AdmissionError("could not encode admission ledger".to_owned()))?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(AdmissionError(
                "admission ledger would exceed its size bound".to_owned(),
            ));
        }
        let (temp, mut file) = locked.temporary()?;
        let result = (|| -> Result<(), AdmissionError> {
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| {
                    AdmissionError("could not durably write admission ledger".to_owned())
                })?;
            locked.verify()?;
            locked.rename(&temp)?;
            locked.directory.sync_all().map_err(|_| {
                AdmissionError("could not sync admission ledger directory".to_owned())
            })?;
            Ok(())
        })();
        if result.is_err() {
            locked.unlink(&temp);
        }
        result
    }

    /// Validate the current ledger without modifying it.
    pub fn validate(&self) -> Result<(), AdmissionError> {
        let locked = LockedLedgerDirectory::open(&self.path)?;
        self.load(&locked).map(|_| ())
    }

    /// Atomically sweep, queue, decide, and commit one request.
    pub fn request<F>(
        &self,
        request: &ResourceRequest,
        snapshot: &HostSnapshot,
        policy: &AdmissionPolicy,
        now_unix_ms: u64,
        owner_probe: F,
    ) -> Result<(Decision, Option<Lease>), AdmissionError>
    where
        F: Fn(&ProcessOwner, &HostSnapshot) -> OwnerState,
    {
        validate_request(request)?;
        let locked = LockedLedgerDirectory::open(&self.path)?;
        let mut data = self.load(&locked)?;
        if data
            .leases
            .iter()
            .any(|lease| lease.lease_id == request.request_id && lease.request != *request)
            || data.queue.iter().any(|entry| {
                entry.request.request_id == request.request_id && entry.request != *request
            })
        {
            return Ok((
                unknown(
                    request,
                    "request_id_conflict",
                    vec!["existing_request_differs".to_owned()],
                ),
                None,
            ));
        }
        let request_state = owner_probe(&request.owner, snapshot);
        if request_state != OwnerState::Alive {
            let blocker = match request_state {
                OwnerState::Alive => unreachable!(),
                OwnerState::Absent => "absent",
                OwnerState::Unknown => "unknown",
            };
            return Ok((
                unknown(
                    request,
                    "request_owner_unconfirmed",
                    vec![blocker.to_owned()],
                ),
                None,
            ));
        }
        let mut states: BTreeMap<_, _> = data
            .leases
            .iter()
            .map(|lease| {
                (
                    lease.lease_id.clone(),
                    owner_probe(&lease.request.owner, snapshot),
                )
            })
            .collect();
        states.extend(data.queue.iter().map(|entry| {
            (
                entry.request.request_id.clone(),
                if entry.request.request_id == request.request_id {
                    request_state
                } else {
                    owner_probe(&entry.request.owner, snapshot)
                },
            )
        }));
        let original_counts = (data.leases.len(), data.queue.len());
        data.leases
            .retain(|lease| states.get(&lease.lease_id) != Some(&OwnerState::Absent));
        data.queue
            .retain(|entry| states.get(&entry.request.request_id) != Some(&OwnerState::Absent));
        let swept = original_counts != (data.leases.len(), data.queue.len());
        if states.values().any(|state| *state == OwnerState::Unknown) {
            let leases = data
                .leases
                .iter()
                .map(|lease| (lease.lease_id.clone(), lease.request.clone()))
                .collect::<Vec<_>>();
            let queue = data
                .queue
                .iter()
                .map(|entry| (entry.sequence, entry.request.clone()))
                .collect::<Vec<_>>();
            if swept {
                self.store(&locked, &data)?;
            }
            return Ok((
                decide(
                    request,
                    snapshot,
                    policy,
                    &leases,
                    &queue,
                    now_unix_ms,
                    &states,
                )?,
                None,
            ));
        }
        if let Some(existing) = data
            .leases
            .iter()
            .find(|lease| lease.lease_id == request.request_id)
        {
            if swept {
                self.store(&locked, &data)?;
            }
            let decision = answer(
                Verdict::Grant,
                "already_granted",
                request,
                resources(
                    data.leases
                        .iter()
                        .filter(|lease| lease.lease_id != existing.lease_id)
                        .map(|lease| lease.request.memory_bytes)
                        .sum(),
                    policy.memory_budget_bytes,
                    None,
                    None,
                    BTreeMap::new(),
                ),
                Vec::new(),
                None,
                Some(existing.lease_id.clone()),
            );
            return Ok((
                decision,
                Some(Lease {
                    lease_id: existing.lease_id.clone(),
                    request: request.clone(),
                    ledger: self.clone(),
                    released: false,
                }),
            ));
        }
        if !data
            .queue
            .iter()
            .any(|entry| entry.request.request_id == request.request_id)
        {
            if data.leases.len() + data.queue.len() >= MAX_RECORDS {
                if swept {
                    self.store(&locked, &data)?;
                }
                return Ok((
                    unknown(
                        request,
                        "record_capacity_exhausted",
                        vec!["ledger_full".to_owned()],
                    ),
                    None,
                ));
            }
            let Some(next_sequence) = data.next_sequence.checked_add(1) else {
                if swept {
                    self.store(&locked, &data)?;
                }
                return Ok((
                    unknown(
                        request,
                        "queue_sequence_exhausted",
                        vec!["ledger_counter_exhausted".to_owned()],
                    ),
                    None,
                ));
            };
            data.queue.push(QueueRecord {
                request: request.clone(),
                sequence: data.next_sequence,
            });
            data.next_sequence = next_sequence;
        }
        let leases = data
            .leases
            .iter()
            .map(|lease| (lease.lease_id.clone(), lease.request.clone()))
            .collect::<Vec<_>>();
        let queue = data
            .queue
            .iter()
            .map(|entry| (entry.sequence, entry.request.clone()))
            .collect::<Vec<_>>();
        let live_states = leases
            .iter()
            .map(|(id, _)| (id.clone(), OwnerState::Alive))
            .chain(
                queue
                    .iter()
                    .map(|(_, queued)| (queued.request_id.clone(), OwnerState::Alive)),
            )
            .collect();
        let decision = decide(
            request,
            snapshot,
            policy,
            &leases,
            &queue,
            now_unix_ms,
            &live_states,
        )?;
        match decision.verdict {
            Verdict::Grant => {
                data.queue
                    .retain(|entry| entry.request.request_id != request.request_id);
                data.leases.push(LeaseRecord {
                    granted_at_unix_ms: now_unix_ms,
                    lease_id: request.request_id.clone(),
                    request: request.clone(),
                });
                self.store(&locked, &data)?;
                Ok((
                    decision,
                    Some(Lease {
                        lease_id: request.request_id.clone(),
                        request: request.clone(),
                        ledger: self.clone(),
                        released: false,
                    }),
                ))
            }
            Verdict::Refuse => {
                data.queue
                    .retain(|entry| entry.request.request_id != request.request_id);
                self.store(&locked, &data)?;
                Ok((decision, None))
            }
            Verdict::Queue => {
                self.store(&locked, &data)?;
                Ok((decision, None))
            }
            Verdict::Unknown => {
                if swept {
                    self.store(&locked, &data)?;
                }
                Ok((decision, None))
            }
        }
    }

    fn release(&self, lease_id: &str, owner: &ProcessOwner) -> Result<(), AdmissionError> {
        let locked = LockedLedgerDirectory::open(&self.path)?;
        let mut data = self.load(&locked)?;
        let before = data.leases.len();
        data.leases
            .retain(|lease| lease.lease_id != lease_id || lease.request.owner != *owner);
        if before != data.leases.len() {
            self.store(&locked, &data)?;
        }
        Ok(())
    }
}

/// A granted lease that releases its exact owner once.
#[derive(Debug)]
pub struct Lease {
    lease_id: String,
    request: ResourceRequest,
    ledger: HostAdmissionLedger,
    released: bool,
}

impl Lease {
    /// Return this exact lease idempotently.
    pub fn release(&mut self) -> Result<(), AdmissionError> {
        if !self.released {
            self.ledger.release(&self.lease_id, &self.request.owner)?;
            self.released = true;
        }
        Ok(())
    }
}

fn current_uid() -> Result<u32, AdmissionError> {
    fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(|_| AdmissionError("could not establish current uid".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

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

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "host-admission-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn owner(pid: u32, start_ticks: u64) -> ProcessOwner {
        ProcessOwner {
            host_id: "host-a".to_owned(),
            boot_id: "boot-a".to_owned(),
            pid,
            start_ticks,
        }
    }

    fn snapshot() -> HostSnapshot {
        HostSnapshot {
            captured_at_unix_ms: 1000,
            host_id: "host-a".to_owned(),
            boot_id: "boot-a".to_owned(),
            mem_total_bytes: Some(1000),
            mem_available_bytes: Some(1000),
            swap_total_bytes: Some(1000),
            swap_free_bytes: Some(1000),
            memory_psi_some_avg10_micros: Some(0),
            memory_psi_full_avg10_micros: Some(0),
            errors: Vec::new(),
        }
    }

    fn policy(tokens: BTreeMap<String, u32>) -> AdmissionPolicy {
        AdmissionPolicy {
            memory_budget_bytes: Some(700),
            memory_reserve_bytes: 0,
            token_capacities: tokens,
            swap_mode: SwapMode::Disabled,
            swap_floor_fraction_ppm: 150_000,
            largest_leaf_swap_bytes: 0,
            no_swap_extra_memory_reserve_bytes: 0,
            psi_some_max_micros: None,
            psi_full_max_micros: None,
            max_snapshot_age_ms: 100,
        }
    }

    fn request(id: &str, pid: u32, start_ticks: u64) -> ResourceRequest {
        ResourceRequest {
            request_id: id.to_owned(),
            caller: "test".to_owned(),
            owner: owner(pid, start_ticks),
            memory_bytes: 1,
            named_tokens: BTreeMap::new(),
            priority: 0,
            metadata_digest: "none".to_owned(),
        }
    }

    #[test]
    fn shared_fixture_corpus_pins_twenty_four_canonical_decisions() {
        let text = include_str!("../fixtures-v1.json");
        let fixtures: Fixtures = serde_json::from_str(text).expect("parse fixture corpus");
        assert_eq!(fixtures.schema, "host-admission-fixtures/v1");
        assert_eq!(fixtures.cases.len(), 24);
        let mut names = BTreeSet::new();
        for case in fixtures.cases {
            assert!(names.insert(case.name.clone()));
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
                .map(|(id, state)| {
                    let state = match state.as_str() {
                        "alive" => OwnerState::Alive,
                        "absent" => OwnerState::Absent,
                        "unknown" => OwnerState::Unknown,
                        other => panic!("unknown owner state {other}"),
                    };
                    (id, state)
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
            .expect("fixture decision");
            assert_eq!(
                decision.verdict.as_str(),
                case.expect.verdict,
                "{}",
                case.name
            );
            assert_eq!(decision.code, case.expect.code, "{}", case.name);
            let canonical = decision.canonical_json();
            assert!(canonical.ends_with('\n'));
            assert!(!canonical.contains(" "));
        }
    }

    #[test]
    fn canonical_ledger_bytes_are_feature_independent() {
        let record = request("cross-edition", 10, 100);
        let data = LedgerData {
            leases: vec![LeaseRecord {
                granted_at_unix_ms: 1010,
                lease_id: "cross-edition".to_owned(),
                request: record,
            }],
            next_sequence: 1,
            queue: Vec::new(),
            schema: LEDGER_SCHEMA.to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&canonical_ledger_value(&data)).unwrap(),
            r#"{"leases":[{"granted_at_unix_ms":1010,"lease_id":"cross-edition","request":{"caller":"test","memory_bytes":1,"metadata_digest":"none","named_tokens":{},"owner":{"boot_id":"boot-a","host_id":"host-a","pid":10,"start_ticks":100},"priority":0,"request_id":"cross-edition"}}],"next_sequence":1,"queue":[],"schema":"host-admission-ledger/v1"}"#
        );
    }

    #[test]
    fn ledger_preserves_fifo_and_acquires_memory_and_tokens_atomically() {
        let root = temp_path("atomic");
        fs::create_dir_all(&root).unwrap();
        let ledger_path = root.join("ledger.json");
        let ledger = HostAdmissionLedger::new(&ledger_path);
        let mut tokens = BTreeMap::new();
        tokens.insert("build".to_owned(), 1);
        let policy = policy(tokens);
        let mut first = request("first", 10, 100);
        first.memory_bytes = 100;
        first.named_tokens.insert("build".to_owned(), 1);
        let mut second = request("second", 11, 101);
        second.memory_bytes = 100;
        second.named_tokens.insert("build".to_owned(), 1);
        let (granted, mut first_lease) = ledger
            .request(&first, &snapshot(), &policy, 1010, |_, _| OwnerState::Alive)
            .unwrap();
        assert_eq!(granted.verdict, Verdict::Grant);
        let (queued, no_lease) = ledger
            .request(&second, &snapshot(), &policy, 1010, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        assert_eq!(queued.code, "named_tokens_busy");
        assert!(no_lease.is_none());
        let first_bytes = fs::read(&ledger_path).unwrap();
        let (queued_again, no_lease) = ledger
            .request(&second, &snapshot(), &policy, 1011, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        assert_eq!(queued.queue_sequence, queued_again.queue_sequence);
        assert!(no_lease.is_none());
        assert_eq!(fs::read(&ledger_path).unwrap(), first_bytes);
        first_lease.as_mut().unwrap().release().unwrap();
        let (admitted, mut second_lease) = ledger
            .request(&second, &snapshot(), &policy, 1012, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        assert_eq!(admitted.verdict, Verdict::Grant);
        second_lease.as_mut().unwrap().release().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_ledger_is_never_replaced_or_granted() {
        let root = temp_path("malformed");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.json");
        let original = b"{\"schema\":\"host-admission-ledger/v1\",\"schema\":\"wrong\"}\n";
        fs::write(&path, original).unwrap();
        let ledger = HostAdmissionLedger::new(&path);
        assert!(ledger
            .request(
                &request("request", 10, 100),
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive
            )
            .is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parent_and_final_component_attacks_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = temp_path("path-attacks");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        let linked = root.join("linked");
        symlink(&target, &linked).unwrap();
        let candidate = request("request", 10, 100);
        assert!(HostAdmissionLedger::new(linked.join("ledger.json"))
            .request(
                &candidate,
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .is_err());

        let writable = root.join("writable");
        fs::create_dir(&writable).unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(HostAdmissionLedger::new(writable.join("ledger.json"))
            .request(
                &candidate,
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .is_err());

        let final_parent = root.join("final");
        fs::create_dir(&final_parent).unwrap();
        fs::set_permissions(&final_parent, fs::Permissions::from_mode(0o700)).unwrap();
        let payload = final_parent.join("payload");
        fs::write(&payload, b"{}").unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&payload, final_parent.join("symlink.json")).unwrap();
        assert!(HostAdmissionLedger::new(final_parent.join("symlink.json"))
            .request(
                &candidate,
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .is_err());
        fs::hard_link(&payload, final_parent.join("hardlink.json")).unwrap();
        assert!(HostAdmissionLedger::new(final_parent.join("hardlink.json"))
            .request(
                &candidate,
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .is_err());
        let lock_target = final_parent.join("lock-target");
        fs::write(&lock_target, b"").unwrap();
        fs::set_permissions(&lock_target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&lock_target, final_parent.join("lock-hardlink.json.lock")).unwrap();
        assert!(
            HostAdmissionLedger::new(final_parent.join("lock-hardlink.json"))
                .request(
                    &candidate,
                    &snapshot(),
                    &policy(BTreeMap::new()),
                    1010,
                    |_, _| OwnerState::Alive,
                )
                .is_err()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn descriptor_binding_detects_lock_unlink_and_contains_parent_retarget() {
        use std::os::unix::fs::symlink;

        let root = temp_path("descriptor-races");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let parent = root.join("parent");
        let moved = root.join("moved");
        let attacker = root.join("attacker");
        fs::create_dir(&parent).unwrap();
        fs::create_dir(&attacker).unwrap();
        let ledger = HostAdmissionLedger::new(parent.join("ledger.json"));
        assert!(ledger
            .request(
                &request("unlink", 10, 100),
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| {
                    fs::remove_file(parent.join("ledger.json.lock")).unwrap();
                    OwnerState::Alive
                },
            )
            .is_err());
        assert!(!parent.join("ledger.json").exists());

        let (decision, lease) = ledger
            .request(
                &request("retarget", 10, 100),
                &snapshot(),
                &policy(BTreeMap::new()),
                1011,
                |_, _| {
                    fs::rename(&parent, &moved).unwrap();
                    symlink(&attacker, &parent).unwrap();
                    OwnerState::Alive
                },
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Grant);
        assert!(lease.is_some());
        assert!(moved.join("ledger.json").is_file());
        assert!(!attacker.join("ledger.json").exists());
        HostAdmissionLedger::new(moved.join("ledger.json"))
            .release("retarget", &owner(10, 100))
            .unwrap();
        fs::remove_file(&parent).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "launched as an isolated subprocess by the parent test"]
    fn bare_relative_path_child() {
        if std::env::var_os("HOST_ADMISSION_RELATIVE_CHILD").is_none() {
            return;
        }
        let ledger = HostAdmissionLedger::new("ledger.json");
        let (decision, _lease) = ledger
            .request(
                &request("relative", 10, 100),
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Grant);
        ledger.validate().unwrap();
    }

    #[test]
    fn bare_relative_and_non_utf8_paths_use_the_exact_lock_name() {
        let root = temp_path("relative-parent");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("tests::bare_relative_path_child")
            .arg("--ignored")
            .env("HOST_ADMISSION_RELATIVE_CHILD", "1")
            .current_dir(&root)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(root.join("ledger.json").is_file());

        let leaf = OsString::from_vec(b"ledger-\xff.json".to_vec());
        let path = root.join(&leaf);
        let ledger = HostAdmissionLedger::new(&path);
        let (decision, _lease) = ledger
            .request(
                &request("non-utf8", 11, 101),
                &snapshot(),
                &policy(BTreeMap::new()),
                1011,
                |_, _| OwnerState::Alive,
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Grant);
        let mut lock = leaf;
        lock.push(".lock");
        assert!(root.join(lock).is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_queue_sequences_and_duplicate_tokens_are_rejected() {
        let root = temp_path("malformed-records");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.json");
        let queued = request("queued", 10, 100);
        let invalid_sequence = serde_json::json!({
            "leases": [],
            "next_sequence": 1,
            "queue": [{"request": queued, "sequence": 1}],
            "schema": LEDGER_SCHEMA,
        });
        let invalid_sequence = format!("{}\n", serde_json::to_string(&invalid_sequence).unwrap());
        let duplicate_token = invalid_sequence
            .replace(
                "\"named_tokens\":{}",
                "\"named_tokens\":{\"build\":1,\"build\":2}",
            )
            .replace("\"sequence\":1", "\"sequence\":0");
        for original in [invalid_sequence, duplicate_token] {
            fs::write(&path, original.as_bytes()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let ledger = HostAdmissionLedger::new(&path);
            assert!(ledger
                .request(
                    &request("current", 20, 200),
                    &snapshot(),
                    &policy(BTreeMap::new()),
                    1010,
                    |_, _| OwnerState::Alive,
                )
                .is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn queue_sequence_exhaustion_is_unknown_without_mutation() {
        let root = temp_path("sequence-exhaustion");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.json");
        let payload = LedgerData {
            leases: Vec::new(),
            next_sequence: u64::MAX,
            queue: Vec::new(),
            schema: LEDGER_SCHEMA.to_owned(),
        };
        let original = format!("{}\n", serde_json::to_string(&payload).unwrap());
        fs::write(&path, original.as_bytes()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let ledger = HostAdmissionLedger::new(&path);
        let (decision, lease) = ledger
            .request(
                &request("current", 10, 100),
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Unknown);
        assert_eq!(decision.code, "queue_sequence_exhausted");
        assert!(lease.is_none());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_dir_all(root).unwrap();
    }

    fn write_ledger(path: &Path, data: &LedgerData) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&canonical_ledger_value(data)).unwrap();
        bytes.push(b'\n');
        fs::write(path, &bytes).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        bytes
    }

    #[test]
    fn record_capacity_is_non_mutating_and_recovers_after_sweep() {
        let root = temp_path("record-capacity");
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let path = root.join("ledger.json");
        let queue = (0..MAX_RECORDS)
            .map(|index| QueueRecord {
                request: request(
                    &format!("queued-{index}"),
                    100 + u32::try_from(index).unwrap(),
                    1000 + u64::try_from(index).unwrap(),
                ),
                sequence: u64::try_from(index).unwrap(),
            })
            .collect();
        let data = LedgerData {
            leases: Vec::new(),
            next_sequence: u64::try_from(MAX_RECORDS).unwrap(),
            queue,
            schema: LEDGER_SCHEMA.to_owned(),
        };
        let original = write_ledger(&path, &data);
        assert!((original.len() as u64) < MAX_LEDGER_BYTES);
        let ledger = HostAdmissionLedger::new(&path);
        let mut near_data = data.clone();
        near_data.queue.pop();
        write_ledger(&path, &near_data);
        let mut near_current = request("near-current", 10, 100);
        near_current.priority = 10;
        let (near_decision, near_lease) = ledger
            .request(
                &near_current,
                &snapshot(),
                &policy(BTreeMap::new()),
                1009,
                |_, _| OwnerState::Alive,
            )
            .unwrap();
        assert_eq!(near_decision.verdict, Verdict::Grant);
        assert!(near_lease.is_some());
        let near_final = ledger
            .load(&LockedLedgerDirectory::open(&path).unwrap())
            .unwrap();
        assert_eq!(
            near_final.leases.len() + near_final.queue.len(),
            MAX_RECORDS
        );

        fs::write(&path, &original).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let mut current = request("current", 10, 100);
        current.priority = 10;
        let (decision, lease) = ledger
            .request(
                &current,
                &snapshot(),
                &policy(BTreeMap::new()),
                1010,
                |_, _| OwnerState::Alive,
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Unknown);
        assert_eq!(decision.code, "record_capacity_exhausted");
        assert!(lease.is_none());
        assert_eq!(fs::read(&path).unwrap(), original);

        let (decision, _lease) = ledger
            .request(
                &current,
                &snapshot(),
                &policy(BTreeMap::new()),
                1011,
                |owner, _| {
                    if owner.pid == 100 {
                        OwnerState::Absent
                    } else {
                        OwnerState::Alive
                    }
                },
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Grant);
        let final_data = ledger
            .load(&LockedLedgerDirectory::open(&path).unwrap())
            .unwrap();
        assert_eq!(
            final_data.leases.len() + final_data.queue.len(),
            MAX_RECORDS
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_queue_decision_is_linear_and_bounded() {
        let mut current = request("current", 10, 100);
        current.priority = 0;
        let mut queue = Vec::with_capacity(MAX_RECORDS);
        let mut states = BTreeMap::new();
        for index in 0..MAX_RECORDS - 1 {
            let mut queued = request(
                &format!("queued-{index}"),
                100 + u32::try_from(index).unwrap(),
                1000 + u64::try_from(index).unwrap(),
            );
            queued.priority = 0;
            states.insert(queued.request_id.clone(), OwnerState::Alive);
            queue.push((u64::try_from(index).unwrap(), queued));
        }
        queue.push((u64::try_from(MAX_RECORDS - 1).unwrap(), current.clone()));
        let started = Instant::now();
        let decision = decide(
            &current,
            &snapshot(),
            &policy(BTreeMap::new()),
            &[],
            &queue,
            1010,
            &states,
        )
        .unwrap();
        assert_eq!(decision.verdict, Verdict::Queue);
        assert_eq!(decision.blockers.len(), MAX_RECORDS - 1);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn psi_decimal_parser_is_exact_and_rejects_hidden_precision() {
        assert_eq!(parse_percent_micros("0.000001"), Some(1));
        assert_eq!(parse_percent_micros("100.000000"), Some(100_000_000));
        assert_eq!(parse_percent_micros("1.0000000"), Some(1_000_000));
        assert_eq!(parse_percent_micros("1.0000001"), None);
        assert_eq!(parse_percent_micros("+1"), None);
        assert_eq!(parse_percent_micros("1."), None);
        let (_values, errors) = parse_meminfo(
            "MemTotal: abc kB\nMemAvailable: 1 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
        );
        assert!(errors.contains(&"mem_total_malformed".to_owned()));
        let (_some, _full, errors) =
            parse_psi("some avg10=0.1 avg10=0.2 total=1\nfull avg10=0 total=0\n");
        assert!(errors.contains(&"memory_psi_some_malformed".to_owned()));
    }

    #[test]
    fn impossible_snapshot_relations_fail_closed() {
        let mut impossible_memory = snapshot();
        impossible_memory.mem_total_bytes = Some(1);
        impossible_memory.mem_available_bytes = Some(2);
        assert!(decide(
            &request("current", 10, 100),
            &impossible_memory,
            &policy(BTreeMap::new()),
            &[],
            &[],
            1010,
            &BTreeMap::new(),
        )
        .is_err());
        let mut impossible_swap = snapshot();
        impossible_swap.swap_total_bytes = Some(1);
        impossible_swap.swap_free_bytes = Some(2);
        assert!(decide(
            &request("current", 10, 100),
            &impossible_swap,
            &policy(BTreeMap::new()),
            &[],
            &[],
            1010,
            &BTreeMap::new(),
        )
        .is_err());
    }

    #[test]
    fn dead_queue_owner_is_swept_unknown_is_retained_and_conflict_refuses() {
        let root = temp_path("queue-owner");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.json");
        let ledger = HostAdmissionLedger::new(&path);
        let mut tokens = BTreeMap::new();
        tokens.insert("build".to_owned(), 1);
        let policy = policy(tokens);
        let mut holder = request("holder", 10, 100);
        holder.named_tokens.insert("build".to_owned(), 1);
        let mut dead = request("dead-ticket", 11, 101);
        dead.named_tokens.insert("build".to_owned(), 1);
        let (_, mut holder_lease) = ledger
            .request(&holder, &snapshot(), &policy, 1010, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        let (queued, _) = ledger
            .request(&dead, &snapshot(), &policy, 1011, |_, _| OwnerState::Alive)
            .unwrap();
        assert_eq!(queued.verdict, Verdict::Queue);
        holder_lease.as_mut().unwrap().release().unwrap();
        let unknown_current = request("unknown-current", 12, 102);
        let before_unknown = fs::read(&path).unwrap();
        let (unknown, no_lease) = ledger
            .request(&unknown_current, &snapshot(), &policy, 1012, |owner, _| {
                if owner.pid == dead.owner.pid {
                    OwnerState::Unknown
                } else {
                    OwnerState::Alive
                }
            })
            .unwrap();
        assert_eq!(unknown.verdict, Verdict::Unknown);
        assert!(no_lease.is_none());
        assert_eq!(fs::read(&path).unwrap(), before_unknown);
        let replacement = request("replacement", 13, 103);
        let (admitted, mut replacement_lease) = ledger
            .request(&replacement, &snapshot(), &policy, 1013, |owner, _| {
                if owner.pid == dead.owner.pid {
                    OwnerState::Absent
                } else {
                    OwnerState::Alive
                }
            })
            .unwrap();
        assert_eq!(admitted.verdict, Verdict::Grant);
        let state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(!state["queue"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["request"]["request_id"].as_str() == Some("dead-ticket") }));
        replacement_lease.as_mut().unwrap().release().unwrap();

        let mut holder_two = request("holder-two", 14, 104);
        holder_two.named_tokens.insert("build".to_owned(), 1);
        let (_, mut holder_two_lease) = ledger
            .request(&holder_two, &snapshot(), &policy, 1014, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        let mut original = request("same-id", 15, 105);
        original.named_tokens.insert("build".to_owned(), 1);
        original.metadata_digest = "original".to_owned();
        assert_eq!(
            ledger
                .request(&original, &snapshot(), &policy, 1015, |_, _| {
                    OwnerState::Alive
                })
                .unwrap()
                .0
                .verdict,
            Verdict::Queue
        );
        let before_conflict = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed.memory_bytes = 2;
        changed.metadata_digest = "changed".to_owned();
        let (conflict, no_lease) = ledger
            .request(&changed, &snapshot(), &policy, 1016, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        assert_eq!(conflict.verdict, Verdict::Unknown);
        assert_eq!(conflict.code, "request_id_conflict");
        assert!(no_lease.is_none());
        assert_eq!(fs::read(&path).unwrap(), before_conflict);
        holder_two_lease.as_mut().unwrap().release().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn absent_queue_owner_is_swept_even_when_a_peer_is_unknown() {
        let root = temp_path("mixed-owner-census");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("ledger.json");
        let ledger = HostAdmissionLedger::new(&path);
        let mut tokens = BTreeMap::new();
        tokens.insert("build".to_owned(), 1);
        let policy = policy(tokens);
        let mut holder = request("holder", 10, 100);
        holder.named_tokens.insert("build".to_owned(), 1);
        let mut dead = request("dead", 11, 101);
        dead.named_tokens.insert("build".to_owned(), 1);
        let mut uncertain = request("uncertain", 12, 102);
        uncertain.named_tokens.insert("build".to_owned(), 1);
        let (_, mut holder_lease) = ledger
            .request(&holder, &snapshot(), &policy, 1010, |_, _| {
                OwnerState::Alive
            })
            .unwrap();
        for queued in [&dead, &uncertain] {
            let (decision, lease) = ledger
                .request(queued, &snapshot(), &policy, 1011, |_, _| OwnerState::Alive)
                .unwrap();
            assert_eq!(decision.verdict, Verdict::Queue);
            assert!(lease.is_none());
        }
        holder_lease.as_mut().unwrap().release().unwrap();
        let (decision, lease) = ledger
            .request(
                &request("current", 13, 103),
                &snapshot(),
                &policy,
                1012,
                |owner, _| {
                    if owner.pid == dead.owner.pid {
                        OwnerState::Absent
                    } else if owner.pid == uncertain.owner.pid {
                        OwnerState::Unknown
                    } else {
                        OwnerState::Alive
                    }
                },
            )
            .unwrap();
        assert_eq!(decision.verdict, Verdict::Unknown);
        assert!(lease.is_none());
        let state: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let queued_ids: BTreeSet<_> = state["queue"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["request"]["request_id"].as_str().unwrap())
            .collect();
        assert_eq!(queued_ids, BTreeSet::from(["uncertain"]));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn boot_change_and_pid_reuse_are_absent_while_unreadable_is_unknown() {
        let mut old_boot = owner(10, 100);
        old_boot.boot_id = "boot-old".to_owned();
        assert_eq!(
            probe_process_owner(&old_boot, &snapshot()),
            OwnerState::Absent
        );
        let current = std::process::id();
        let current_start = proc_start_ticks(current).unwrap().unwrap();
        let reused = ProcessOwner {
            pid: current,
            start_ticks: current_start + 1,
            ..owner(current, current_start)
        };
        assert_eq!(
            probe_process_owner(&reused, &snapshot()),
            OwnerState::Absent
        );
        let foreign = ProcessOwner {
            host_id: "host-b".to_owned(),
            ..owner(10, 100)
        };
        assert_eq!(
            probe_process_owner(&foreign, &snapshot()),
            OwnerState::Unknown
        );
    }

    #[test]
    fn mutations_of_fifo_owner_and_token_guards_change_the_decision() {
        let mut tokens = BTreeMap::new();
        tokens.insert("build".to_owned(), 1);
        let policy = policy(tokens);
        let mut peer = request("peer", 10, 100);
        peer.named_tokens.insert("build".to_owned(), 1);
        let mut current = request("current", 12, 102);
        current.priority = 5;
        current.named_tokens.insert("build".to_owned(), 1);
        let mut earlier = request("earlier", 11, 101);
        earlier.priority = 5;
        let leases = vec![("peer".to_owned(), peer)];
        let queue = vec![(1, earlier), (2, current.clone())];
        let unknown_states = BTreeMap::from([("peer".to_owned(), OwnerState::Unknown)]);
        assert_eq!(
            decide(
                &current,
                &snapshot(),
                &policy,
                &leases,
                &queue,
                1010,
                &unknown_states
            )
            .unwrap()
            .verdict,
            Verdict::Unknown
        );
        let absent_states = BTreeMap::from([("peer".to_owned(), OwnerState::Absent)]);
        let mut fifo_states = absent_states.clone();
        fifo_states.insert("earlier".to_owned(), OwnerState::Alive);
        assert_eq!(
            decide(
                &current,
                &snapshot(),
                &policy,
                &leases,
                &queue,
                1010,
                &fifo_states
            )
            .unwrap()
            .code,
            "queued_behind_prior_request"
        );
        assert_eq!(
            decide(
                &current,
                &snapshot(),
                &policy,
                &leases,
                &[(2, current.clone())],
                1010,
                &absent_states
            )
            .unwrap()
            .verdict,
            Verdict::Grant
        );
        let alive_states = BTreeMap::from([("peer".to_owned(), OwnerState::Alive)]);
        assert_eq!(
            decide(
                &current,
                &snapshot(),
                &policy,
                &leases,
                &[(2, current.clone())],
                1010,
                &alive_states
            )
            .unwrap()
            .code,
            "named_tokens_busy"
        );
    }
}
