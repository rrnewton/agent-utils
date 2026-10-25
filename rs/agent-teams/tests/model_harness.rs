//! The model-based exactly-once harness (design section 9.7): a deterministic,
//! seeded simulation with real ledger and outbox code on a temporary
//! filesystem, fake routes with random faults, and the invariants checked
//! after every run. Any failure prints its seed for replay.
//!
//! The regular gate runs a bounded pass (`GATE_SEEDS`). The soak
//! (`make soak-agent-teams`) runs the `#[ignore]`d sweep for longer.

use agent_teams::deliver::FakeDeliverer;
use agent_teams::envelope::build_envelope;
use agent_teams::ledger::{accept, entry_path, read_entry, AcceptContext};
use agent_teams::model::{Contact, Entry, Host};
use agent_teams::route::{AttemptOutcome, Route};
use agent_teams::sender::{walk, write_outbox};
use agent_teams::serve::handle_verb;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// A tiny xorshift PRNG: deterministic from the seed, replayable.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Faults the harness injects per route attempt (design section 9.7).
#[derive(Clone, Copy, PartialEq, Debug)]
enum Fault {
    /// The route works.
    Ok,
    /// Fails before the envelope could reach the host: definite.
    BeforeSend,
    /// The answer is lost after the host accepted: ambiguous.
    AnswerLost,
    /// The route delivers the same envelope twice (a repost, a replay).
    Duplicate,
    /// The host is partitioned away for this attempt: ambiguous.
    Partitioned,
}

/// A fake host route that ends at the receiver's real `serve` dispatch.
struct FaultyRoute {
    name: String,
    state: PathBuf,
    host: String,
    seed: Mutex<u64>,
    /// Past faults, for replay notes.
    history: Mutex<Vec<Fault>>,
    /// When set, every attempt uses this fault (named regression cases).
    forced: Option<Fault>,
}

impl FaultyRoute {
    fn pick_fault(&self) -> Fault {
        if let Some(fault) = self.forced {
            return fault;
        }
        let mut seed = self.seed.lock().expect("seed lock");
        let mut rng = Rng(*seed);
        let fault = match rng.below(10) {
            0 => Fault::BeforeSend,
            1 => Fault::AnswerLost,
            2 => Fault::Duplicate,
            3 => Fault::Partitioned,
            _ => Fault::Ok,
        };
        *seed = rng.next();
        drop(seed);
        self.history.lock().expect("history lock").push(fault);
        fault
    }
}

impl Route for FaultyRoute {
    fn name(&self) -> &str {
        &self.name
    }
    fn run_verb(&self, verb: &str, payload: &Value) -> AttemptOutcome {
        // Receipts and fences are reads/short writes; faults apply to accept.
        if verb != "accept" {
            return AttemptOutcome::Answered(handle_verb(
                &self.state,
                &self.host,
                "host-a",
                verb,
                payload,
            ));
        }
        match self.pick_fault() {
            Fault::Ok => AttemptOutcome::Answered(handle_verb(
                &self.state,
                &self.host,
                "host-a",
                verb,
                payload,
            )),
            Fault::BeforeSend => {
                AttemptOutcome::NotDelivered("injected: failed before send".into())
            }
            Fault::AnswerLost => {
                let _ = handle_verb(&self.state, &self.host, "host-a", verb, payload);
                AttemptOutcome::Ambiguous("injected: answer lost".into())
            }
            Fault::Duplicate => {
                let first = handle_verb(&self.state, &self.host, "host-a", verb, payload);
                let _ = handle_verb(&self.state, &self.host, "host-a", verb, payload);
                AttemptOutcome::Answered(first)
            }
            Fault::Partitioned => AttemptOutcome::Ambiguous("injected: partition".into()),
        }
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_state(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "agent-teams-harness-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("state dir");
    dir
}

fn declaration_for(state: &Path, team: &str, sender: &str) {
    let path = state.join("teams.d").join(team).join("declaration.json");
    let decl = json!({
        "schema": "agent-teams-declaration/v1",
        "team": team,
        "deliverer": {"kind": "hook-inbox"},
        "coordinator": {"designation": "pinned", "harness_identity": {"boot_id": "x", "pid": 1, "starttime_ticks": 1}},
        "accepts_from": [{"sender": sender, "kinds": ["info", "request", "hold", "resume", "reply"]}],
        "limits": {"max_bytes": 8192, "max_ttl_seconds": 604800},
        "default_ttl_seconds": 7200,
    });
    agent_teams::jsonfile::write_json_atomic(&path, &decl).expect("declaration");
}

/// One simulated world: two hosts in one state dir, a fake deliverer wired
/// through a real declaration would need the hook inbox; instead the harness
/// counts hook-inbox files as pastes. Returns the paste count for the ID.
fn paste_count(state: &Path, team: &str, id: &str) -> usize {
    let inbox = state.join("teams.d").join(team).join("hook-inbox");
    let mut count = 0;
    for dir in [inbox.clone(), inbox.join("claimed")] {
        if dir.join(format!("{id}.json")).exists() {
            count += 1;
        }
    }
    count
}

fn run_seed(seed: u64) {
    let state = temp_state(&format!("seed{seed}"));
    declaration_for(&state, "tools", "sim");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let body = format!("hold landing (seed {seed})");
    let id = agent_teams::envelope::message_id("sim", "host-a", "tools@host-a", "k1");
    let envelope = build_envelope(
        &id,
        "k1",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "hold",
        Some("ci-hold"),
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        &body,
    );
    let mut entry = write_outbox(
        &state,
        &envelope,
        "tools@host-a",
        &json!({"proof": "harness-ancestry"}),
    )
    .expect("outbox");
    let team_entry = Entry {
        host: "host-a".into(),
        coordinator: None,
        contacts: vec![Contact::Host],
    };
    let host = Host {
        name: "host-a".into(),
        routes: vec!["faulty-1".into(), "faulty-2".into()],
        agent_teams: None,
        services: BTreeMap::new(),
    };
    let routes: Vec<Box<dyn Route>> = (1..=2)
        .map(|i| {
            Box::new(FaultyRoute {
                name: format!("faulty-{i}"),
                state: state.clone(),
                host: "host-a".into(),
                seed: Mutex::new(seed.wrapping_add(i * 7919).max(1)),
                history: Mutex::new(Vec::new()),
                forced: None,
            }) as Box<dyn Route>
        })
        .collect();
    // Pump several times, as timers would; stop at a terminal state.
    let mut final_state = String::new();
    for _ in 0..6 {
        let result = walk(&state, &mut entry, &team_entry, &host, &routes).expect("walk");
        final_state = result.state.clone();
        // Invariant: at most one paste per ID, checked after every step.
        let pastes = paste_count(&state, "tools", &id);
        assert!(
            pastes <= 1,
            "seed {seed}: ID {id} pasted {pastes} times (state {final_state})"
        );
        // Invariant: at most one ledger entry, and its state is visible.
        let ledger = read_entry(&state, "tools", &id);
        if agent_teams::sender::is_terminal(&final_state) || final_state == "accepted" {
            assert!(
                ledger.is_some(),
                "seed {seed}: settled as {final_state} without a ledger entry"
            );
            break;
        }
    }
    // Invariant: every message ends visible — settled, or honestly unsettled.
    assert!(
        [
            "accepted",
            "delivered",
            "deferred",
            "ambiguous",
            "refused",
            "expired"
        ]
        .contains(&final_state.as_str()),
        "seed {seed}: unexpected final state {final_state}"
    );
    let _ = std::fs::remove_dir_all(&state);
}

/// Fixed regression seeds (any seed that ever fails joins this list forever).
const REGRESSION_SEEDS: &[u64] = &[1, 2, 3, 7, 42, 99, 1234, 31337];
/// Bounded pass in the regular gate: regression seeds plus a sweep.
const GATE_SEEDS: u64 = 120;

#[test]
fn model_harness_gate_pass() {
    for &seed in REGRESSION_SEEDS {
        run_seed(seed);
    }
    for seed in 10_000..10_000 + GATE_SEEDS {
        run_seed(seed);
    }
}

/// The soak: `make soak-agent-teams` runs this with more seeds
/// (`AGENT_TEAMS_SOAK_SEEDS`, default 5000).
#[test]
#[ignore = "soak: run via make soak-agent-teams"]
fn model_harness_soak() {
    let seeds: u64 = std::env::var("AGENT_TEAMS_SOAK_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5000);
    for seed in 100_000..100_000 + seeds {
        run_seed(seed);
    }
}

/// Named regression (I5's shape): a slow send whose answer is lost, retried
/// by the sender, must still paste once. The ask-before-resend rule finds
/// the message already accepted.
#[test]
fn lost_answer_then_retry_pastes_once() {
    let state = temp_state("i5");
    declaration_for(&state, "tools", "sim");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = agent_teams::envelope::message_id("sim", "host-a", "tools@host-a", "retry-key");
    let envelope = build_envelope(
        &id,
        "retry-key",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "hello",
    );
    let mut entry = write_outbox(&state, &envelope, "tools@host-a", &json!({})).expect("outbox");
    let team_entry = Entry {
        host: "host-a".into(),
        coordinator: None,
        contacts: vec![Contact::Host],
    };
    let host = Host {
        name: "host-a".into(),
        routes: vec!["loses-answers".into()],
        agent_teams: None,
        services: BTreeMap::new(),
    };
    let routes: Vec<Box<dyn Route>> = vec![Box::new(FaultyRoute {
        name: "loses-answers".into(),
        state: state.clone(),
        host: "host-a".into(),
        seed: Mutex::new(1),
        history: Mutex::new(Vec::new()),
        forced: Some(Fault::AnswerLost),
    })];
    let result = walk(&state, &mut entry, &team_entry, &host, &routes).expect("walk");
    assert_eq!(
        result.state, "accepted",
        "the receipt settles a lost answer"
    );
    assert_eq!(paste_count(&state, "tools", &id), 1);
    let _ = std::fs::remove_dir_all(&state);
}

/// Crash between claim and hand-over: a `claimed` entry with nothing at the
/// deliverer is re-handed-over once by the next accept (section 9.2).
#[test]
fn claimed_entry_recovers_once() {
    let state = temp_state("claim-crash");
    declaration_for(&state, "tools", "sim");
    let declarations = agent_teams::proof::load_declarations(&state).expect("decls");
    let (_, _, declaration) = &declarations[0];
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = "tm-sim-0123456789abcdef".to_string();
    let envelope = build_envelope(
        &id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "body",
    );
    let fake = FakeDeliverer::default();
    let ctx = AcceptContext {
        state: &state,
        this_host: "host-a",
        declaration,
        via: "local",
        proof_note: "proved",
        relayed_by: None,
        peer_host: "host-a",
    };
    // Simulate the crash: write the claim but never hand over.
    let claim = json!({
        "id": id, "state": "claimed", "body_sha256": envelope["body_sha256"],
        "sender": "sim", "envelope": envelope,
    });
    let dir = state.join("teams.d/tools/ledger");
    std::fs::create_dir_all(&dir).expect("ledger dir");
    std::fs::write(
        entry_path(&state, "tools", &id),
        serde_json::to_string_pretty(&claim).unwrap(),
    )
    .expect("claim");
    let answer = accept(&ctx, &envelope, &fake).expect("accept");
    assert_eq!(answer.state.as_deref(), Some("accepted"));
    assert_eq!(fake.paste_count(&id), 1, "recovery hands over exactly once");
    let _ = std::fs::remove_dir_all(&state);
}

/// A fence before arrival bars every later copy (section 9.4).
#[test]
fn fence_bars_late_copies() {
    let state = temp_state("fence");
    declaration_for(&state, "tools", "sim");
    let id = "tm-sim-fedcba9876543210";
    let fenced = agent_teams::ledger::fence(&state, "tools", id, "2999-01-01T00:00:00Z", None)
        .expect("fence");
    assert_eq!(fenced["outcome"], "fenced");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let envelope = build_envelope(
        id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "late",
    );
    let answer = handle_verb(&state, "host-a", "host-a", "accept", &envelope);
    assert_eq!(
        answer["state"], "fenced",
        "a fenced ID is never accepted later"
    );
    assert_eq!(paste_count(&state, "tools", id), 0);
    let _ = std::fs::remove_dir_all(&state);
}

/// The same key with a different body conflicts at the receiver.
#[test]
fn same_id_different_body_conflicts() {
    let state = temp_state("conflict");
    declaration_for(&state, "tools", "sim");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = "tm-sim-aaaaaaaaaaaaaaaa";
    let first = build_envelope(
        id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "first",
    );
    let second = build_envelope(
        id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "second",
    );
    let a1 = handle_verb(&state, "host-a", "host-a", "accept", &first);
    assert_eq!(a1["outcome"], "accepted");
    let a2 = handle_verb(&state, "host-a", "host-a", "accept", &second);
    assert_eq!(a2["outcome"], "conflict");
    assert_eq!(paste_count(&state, "tools", id), 1);
    let _ = std::fs::remove_dir_all(&state);
}

/// An expired envelope is refused and never pasted (I3's shape).
#[test]
fn expired_envelope_is_refused() {
    let state = temp_state("expired");
    declaration_for(&state, "tools", "sim");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = "tm-sim-bbbbbbbbbbbbbbbb";
    let envelope = build_envelope(
        id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "hold",
        None,
        &[],
        false,
        now_ms - 10_000_000,
        now_ms - 1_000,
        None,
        "stale hold",
    );
    let answer = handle_verb(&state, "host-a", "host-a", "accept", &envelope);
    assert_eq!(answer["state"], "expired");
    assert_eq!(paste_count(&state, "tools", id), 0);
    let _ = std::fs::remove_dir_all(&state);
}

/// Serve inputs are validated before any path join (review regression).
#[test]
fn serve_rejects_path_traversal() {
    let state = temp_state("traversal");
    declaration_for(&state, "tools", "sim");
    let fenced = agent_teams::ledger::fence(
        &state,
        "tools",
        "../declaration",
        "2999-01-01T00:00:00Z",
        None,
    )
    .expect("fence");
    assert_eq!(fenced["outcome"], "error");
    let answer = handle_verb(
        &state,
        "host-a",
        "host-a",
        "receipt",
        &json!({"team": "../tools", "id": "../declaration"}),
    );
    assert_eq!(answer["known"], false);
    assert!(
        state.join("teams.d/tools/declaration.json").exists(),
        "declaration untouched"
    );
    let _ = std::fs::remove_dir_all(&state);
}

/// An envelope claiming another host than the route peer is route-refused.
#[test]
fn peer_host_mismatch_is_route_refused() {
    let state = temp_state("peer");
    declaration_for(&state, "tools", "sim");
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = "tm-sim-cccccccccccccccc";
    let envelope = build_envelope(
        id,
        "k",
        "sim",
        "host-b",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "forged origin",
    );
    let answer = handle_verb(&state, "host-a", "host-a", "accept", &envelope);
    assert_eq!(answer["outcome"], "route-refused", "{answer}");
    assert_eq!(paste_count(&state, "tools", id), 0);
    let _ = std::fs::remove_dir_all(&state);
}

/// The agentctl-queue deliverer really enqueues through `agent-queue`:
/// the item lands in the classic lane in the crate's format, a second
/// hand-over of the same envelope is idempotent, and `contains` reads the
/// crate's message state.
#[test]
fn agentctl_queue_deliverer_uses_agent_queue() {
    use agent_teams::deliver::{AgentctlQueueDeliverer, Deliverer, Handover};
    let state = temp_state("queue-deliverer");
    let queue_dir = state.join("queue");
    let deliverer = AgentctlQueueDeliverer {
        queue_dir: queue_dir.clone(),
        ledger_path: state.join("ledger.json"),
    };
    let now_ms = agent_teams::timeutil::now_unix_ms();
    let id = "tm-sim-dddddddddddddddd";
    let envelope = build_envelope(
        id,
        "k",
        "sim",
        "host-a",
        1,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        now_ms,
        now_ms + 7_200_000,
        None,
        "queued body",
    );
    assert_eq!(deliverer.contains(id), Some(false));
    assert_eq!(
        deliverer.handover(id, &envelope, "rendered text"),
        Handover::Accepted
    );
    assert_eq!(
        agent_queue::queue::message_state(&queue_dir, id).expect("state"),
        Some(agent_queue::queue::MessageState::Pending)
    );
    let doc = agent_queue::queue::load_message(&queue_dir.join("inbox").join(format!("{id}.json")))
        .expect("item");
    assert_eq!(
        doc.get("text").and_then(|t| t.as_str()),
        Some("rendered text")
    );
    assert!(doc.get("external_delivery_record").is_some());
    // Idempotent: the crate refuses the duplicate, we recognise our own item.
    assert_eq!(
        deliverer.handover(id, &envelope, "rendered text"),
        Handover::Accepted
    );
    assert_eq!(deliverer.contains(id), Some(true));
    assert_eq!(
        agent_queue::queue::pending_identifiers(&queue_dir).expect("pending"),
        vec![id.to_string()]
    );
    let _ = std::fs::remove_dir_all(&state);
}
