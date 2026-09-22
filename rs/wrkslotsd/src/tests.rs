use std::collections::BTreeSet;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::index::{
    rebuild_index_with_postpublication_hook, rebuild_index_with_prepublication_hook,
};
use crate::replay::{replay_with_post_count_hook, MAX_EVENT_BYTES, MAX_JSON_CONTAINER_DEPTH};
use crate::schema::parse_timestamp;
use crate::{canonical_sha256, read_index, rebuild_index, replay};

static SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("wrkslotsd-test-{}-{sequence}", std::process::id()));
        fs::create_dir(&path).expect("create scratch directory");
        Self(path)
    }

    fn events(&self) -> PathBuf {
        let path = self.0.join("project/worktrees/EVENTS.node-a");
        fs::create_dir_all(path.parent().expect("event parent")).expect("create control directory");
        fs::create_dir(&path).expect("create event directory");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove scratch directory");
    }
}

fn active_record(slot: &str, generation: u64) -> Value {
    active_record_for(slot, &format!("agent-{slot}"), generation, "agent")
}

fn identity(pid: u64) -> Value {
    json!({
        "pid": pid,
        "start_ticks": pid + 100,
        "boot_id": "11111111-2222-3333-4444-555555555555",
        "host_id": "neutral-host",
        "cgroup_path": "/neutral.slice/worker.scope",
    })
}

fn checkout_for(slot: &str, slot_type: &str) -> Value {
    let root = if slot_type == "validate" {
        "worktrees/validate"
    } else {
        "worktrees/slots"
    };
    json!({
        "name": "source",
        "path": format!("{root}/{slot}/source"),
        "repository": ".",
        "branch": "topic",
        "start_point": "1".repeat(40),
        "remote": "origin",
        "remote_url_sha256": "2".repeat(64),
        "landed_ref": "refs/remotes/origin/main",
        "head": "3".repeat(40),
        "containing_remote_refs": ["refs/remotes/origin/topic"],
        "vcs": "git",
    })
}

fn active_record_for(slot: &str, agent: &str, generation: u64, slot_type: &str) -> Value {
    json!({
        "slot": slot,
        "agent": agent,
        "task": "exercise observer replay",
        "purpose": "neutral fixture",
        "slot_type": slot_type,
        "machine": "node-a",
        "generation": generation,
        "created_at": "2026-09-22T09:00:00+00:00",
        "heartbeat_at": "2026-09-22T09:30:00+00:00",
        "heartbeat_ttl_seconds": 600,
        "owner": identity(1000 + generation),
        "coordinator_lease": identity(2000 + generation),
        "coordinator_recovery_note": null,
        "handoff": {
            "recorded_at": "2026-09-22T09:45:00+00:00",
            "validation": ["neutral handoff validation"],
            "limitations": [],
            "continuation": "resume the neutral fixture",
        },
        "checkouts": [checkout_for(slot, slot_type)],
    })
}

fn imported_active_record(slot: &str, agent: &str, generation: u64) -> Value {
    let mut record = active_record_for(slot, agent, generation, "agent");
    let owner = record["owner"].clone();
    let row = json!({
        "status": "active",
        "allocated": record["created_at"].clone(),
        "task": record["task"].clone(),
        "purpose": record["purpose"].clone(),
        "owner_sidecar": {
            "supervisor_pid": owner["pid"].clone(),
            "start_ticks": owner["start_ticks"].clone(),
            "boot_id": owner["boot_id"].clone(),
            "cgroup_path": owner["cgroup_path"].clone(),
            "slot": slot,
            "agent": agent,
            "task": record["task"].clone(),
        },
        "agents": [{
            "name": agent,
            "read_only": false,
            "task": record["task"].clone(),
        }],
        "source_path": record["checkouts"][0]["path"].clone(),
    });
    let row_sha256 = canonical_sha256(&row).expect("historical row digest");
    record["layout"] = Value::String("nested".to_owned());
    record["import_source"] = json!({
        "format": "worktree-state-v3",
        "path": "legacy/state.json",
        "file_sha256": "4".repeat(64),
        "row_sha256": row_sha256,
        "status": "active",
        "row": row,
    });
    record
}

fn archive_record(slot: &str, generation: u64) -> Value {
    archive_record_for(slot, generation, "agent")
}

fn archive_record_for(slot: &str, generation: u64, slot_type: &str) -> Value {
    let finished_at = "2026-09-22T10:00:00+00:00";
    json!({
        "archive_id": format!("node-a:{slot}:{generation}:{finished_at}"),
        "slot": slot,
        "agent": format!("agent-{slot}"),
        "task": "exercise observer replay",
        "purpose": "neutral fixture",
        "slot_type": slot_type,
        "machine": "node-a",
        "generation": generation,
        "created_at": "2026-09-22T09:00:00+00:00",
        "finished_at": finished_at,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["neutral validation evidence"],
        "limitations": [],
        "continuation": "continue with the next neutral fixture",
        "checkouts": [checkout_for(slot, slot_type)],
        "salvage": [{"kind": "neutral-evidence"}],
    })
}

fn imported_state(
    active: Vec<Value>,
    active_revision: u64,
    archive: Vec<Value>,
    archive_revision: u64,
) -> Value {
    json!({
        "active": {
            "schema": 2,
            "machine": "node-a",
            "revision": active_revision,
            "slots": active,
        },
        "archive": {
            "schema": 2,
            "machine": "node-a",
            "revision": archive_revision,
            "records": archive,
        },
        "holds": [],
    })
}

fn append_event(
    directory: &Path,
    sequence: u64,
    previous_sha256: &str,
    kind: &str,
    payload: Value,
) -> String {
    let core = json!({
        "schema": 1,
        "machine": "node-a",
        "sequence": sequence,
        "previous_sha256": previous_sha256,
        "recorded_at": format!("2026-09-22T10:00:{sequence:02}+00:00"),
        "kind": kind,
        "payload": payload,
    });
    let digest = canonical_sha256(&core).expect("canonical event digest");
    let mut event = core.as_object().expect("event object").clone();
    event.insert("sha256".to_owned(), Value::String(digest.clone()));
    let path = directory.join(format!("{sequence:020}.json"));
    fs::write(
        path,
        serde_json::to_vec_pretty(&Value::Object(event)).expect("encode event"),
    )
    .expect("write event");
    digest
}

fn one_event_log(directory: &Path) -> String {
    append_event(
        directory,
        1,
        &"0".repeat(64),
        "state-imported",
        imported_state(vec![active_record("slot-a", 1)], 3, vec![], 7),
    )
}

fn import_log(directory: &Path, active: Vec<Value>, archive: Vec<Value>) -> String {
    append_event(
        directory,
        1,
        &"0".repeat(64),
        "state-imported",
        imported_state(active, 0, archive, 0),
    )
}

fn python_replay(events: &Path) -> std::process::Output {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let project = events
        .parent()
        .and_then(Path::parent)
        .expect("fixture project root");
    let script = r#"
import json
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from wrkslots import cli as w
root = Path(sys.argv[2])
config = w.Config(
    root=root,
    config_path=root / '.wrkslots.yml',
    worktrees=root / 'worktrees' / 'slots',
    control=root / 'worktrees',
    machine='node-a',
    default_remote='origin',
    default_landed_ref='refs/remotes/origin/main',
    heartbeat_ttl_seconds=600,
    liveness_command=Path('/bin/true'),
    layout='nested',
)
active, archive = w._states_from_events(config, 'node-a', require_repository=False)
events = w._load_events(config, 'node-a')
print(json.dumps({
    'machine': active.machine,
    'replay_count': len(events),
    'tip_sha256': events[-1]['sha256'],
    'active_revision': active.revision,
    'active_records': w._active_to_obj(active)['slots'],
    'archive_revision': archive.revision,
    'archive_records': w._archive_to_obj(archive)['records'],
}, sort_keys=True, separators=(',', ':')))
"#;
    Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(repository.join("py"))
        .arg(project)
        .output()
        .expect("run Python differential oracle")
}

fn python_canonical(source: &str) -> (String, String) {
    let script = r#"
import hashlib
import json
import sys
canonical = json.dumps(
    json.loads(sys.argv[1]), sort_keys=True, separators=(',', ':'), ensure_ascii=True
)
print(json.dumps({
    'canonical': canonical,
    'sha256': hashlib.sha256(canonical.encode('utf-8')).hexdigest(),
}))
"#;
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(source)
        .output()
        .expect("run Python canonical JSON oracle");
    assert!(
        output.status.success(),
        "Python canonical JSON oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("parse Python oracle result");
    (
        response["canonical"]
            .as_str()
            .expect("Python canonical JSON")
            .to_owned(),
        response["sha256"]
            .as_str()
            .expect("Python canonical digest")
            .to_owned(),
    )
}

fn python_timestamp_acceptance(values: &[String]) -> Vec<bool> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repository root");
    let script = r#"
import json
import sys
sys.path.insert(0, sys.argv[1])
from wrkslots import cli as w
results = []
for value in json.load(sys.stdin):
    try:
        w._parse_timestamp(value, 'Rust differential fixture')
    except w.StateError:
        results.append(False)
    else:
        results.append(True)
print(json.dumps(results, separators=(',', ':')))
"#;
    let encoded = serde_json::to_string(values).expect("encode timestamp matrix");
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(repository.join("py"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start Python timestamp oracle");
    child
        .stdin
        .take()
        .expect("Python timestamp oracle stdin")
        .write_all(encoded.as_bytes())
        .expect("write Python timestamp matrix");
    let output = child
        .wait_with_output()
        .expect("run Python timestamp oracle");
    assert!(
        output.status.success(),
        "Python timestamp oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse Python timestamp oracle result")
}

fn rewrite_first_event_timestamp_with_python_hash(events: &Path, timestamp: &str) {
    let path = events.join("00000000000000000001.json");
    let mut event: Value =
        serde_json::from_slice(&fs::read(&path).expect("read event")).expect("parse event");
    event["recorded_at"] = Value::String(timestamp.to_owned());
    let mut core = event.as_object().expect("event object").clone();
    core.remove("sha256");
    let source = serde_json::to_string(&Value::Object(core)).expect("encode event core");
    let (_, digest) = python_canonical(&source);
    event["sha256"] = Value::String(digest);
    fs::write(
        path,
        serde_json::to_vec_pretty(&event).expect("encode timestamp event"),
    )
    .expect("write timestamp event");
}

fn append_python_hashed_payload(
    directory: &Path,
    sequence: u64,
    previous_sha256: &str,
    payload: &str,
) -> String {
    let core = raw_event_core(sequence, previous_sha256, payload);
    let (_, digest) = python_canonical(&core);
    write_raw_event(directory, sequence, &core, &digest);
    digest
}

fn raw_event_core(sequence: u64, previous_sha256: &str, payload: &str) -> String {
    format!(
        "{{\"schema\":1,\"machine\":\"node-a\",\"sequence\":{sequence},\"previous_sha256\":\"{previous_sha256}\",\"recorded_at\":\"2026-09-22T10:00:{sequence:02}+00:00\",\"kind\":\"operation-completed\",\"payload\":{payload}}}"
    )
}

fn write_raw_event(directory: &Path, sequence: u64, core: &str, digest: &str) {
    let event = format!(
        "{},\"sha256\":\"{digest}\"}}\n",
        core.strip_suffix('}').expect("event core object")
    );
    fs::write(
        directory.join(format!("{sequence:020}.json")),
        event.as_bytes(),
    )
    .expect("write Python-hashed event");
}

fn nested_payload(depth: usize) -> String {
    format!(
        "{{\"nested\":{}null{}}}",
        "[".repeat(depth),
        "]".repeat(depth)
    )
}

#[test]
fn valid_current_event_shapes_replay_to_state_summary() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let active_a = active_record("slot-a", 1);
    let first = append_event(
        &events,
        1,
        &"0".repeat(64),
        "state-imported",
        imported_state(
            vec![active_a.clone()],
            3,
            vec![archive_record("slot-old", 4)],
            7,
        ),
    );
    let second = append_event(
        &events,
        2,
        &first,
        "operation-progress-recorded",
        json!({"slot": "slot-a", "operation": "finish", "journal": {}}),
    );
    let third = append_event(
        &events,
        3,
        &second,
        "active-state-recorded",
        json!({
            "action": "finished",
            "slot": "slot-a",
            "previous_revision": 3,
            "revision": 4,
            "previous_record_sha256": canonical_sha256(&active_a).expect("record digest"),
            "record": null,
            "evidence": {},
        }),
    );
    let fourth = append_event(
        &events,
        4,
        &third,
        "active-state-recorded",
        json!({
            "action": "allocated",
            "slot": "slot-b",
            "previous_revision": 4,
            "revision": 5,
            "previous_record_sha256": null,
            "record": active_record("slot-b", 2),
            "evidence": {},
        }),
    );
    let tip = append_event(
        &events,
        5,
        &fourth,
        "archive-state-recorded",
        json!({
            "action": "finished",
            "slot": "slot-a",
            "previous_revision": 7,
            "revision": 8,
            "record": archive_record("slot-a", 1),
            "evidence": {},
        }),
    );

    let summary = replay(&events).expect("valid replay");
    assert_eq!(summary.machine, "node-a");
    assert_eq!(summary.replay_count, 5);
    assert_eq!(summary.tip_sha256, tip);
    assert_eq!(summary.active_revision, 5);
    assert_eq!(summary.active_count, 1);
    assert_eq!(summary.archive_revision, 8);
    assert_eq!(summary.archive_count, 2);
}

#[test]
fn chain_digest_sequence_and_filename_corruption_are_each_rejected() {
    let chain_scratch = Scratch::new();
    let chain_events = chain_scratch.events();
    one_event_log(&chain_events);
    append_event(
        &chain_events,
        2,
        &"f".repeat(64),
        "operation-completed",
        json!({}),
    );
    assert!(replay(&chain_events)
        .expect_err("broken chain must fail")
        .to_string()
        .contains("hash chain is broken"));

    let digest_scratch = Scratch::new();
    let digest_events = digest_scratch.events();
    one_event_log(&digest_events);
    let path = digest_events.join("00000000000000000001.json");
    let mut event: Value =
        serde_json::from_slice(&fs::read(&path).expect("read event")).expect("parse event");
    event["sha256"] = Value::String("f".repeat(64));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&event).expect("encode damaged event"),
    )
    .expect("write damaged event");
    assert!(replay(&digest_events)
        .expect_err("digest mismatch must fail")
        .to_string()
        .contains("digest does not match"));

    let sequence_scratch = Scratch::new();
    let sequence_events = sequence_scratch.events();
    let first = one_event_log(&sequence_events);
    append_event(
        &sequence_events,
        3,
        &first,
        "operation-completed",
        json!({}),
    );
    assert!(replay(&sequence_events)
        .expect_err("sequence gap must fail")
        .to_string()
        .contains("sequence gap"));

    let filename_scratch = Scratch::new();
    let filename_events = filename_scratch.events();
    let first = one_event_log(&filename_events);
    append_event(
        &filename_events,
        3,
        &first,
        "operation-completed",
        json!({}),
    );
    fs::rename(
        filename_events.join("00000000000000000003.json"),
        filename_events.join("00000000000000000002.json"),
    )
    .expect("rename event to mismatched filename");
    assert!(replay(&filename_events)
        .expect_err("sequence/filename mismatch must fail")
        .to_string()
        .contains("sequence does not match its filename"));
}

#[test]
fn canonical_hash_matches_ascii_escaped_python_unicode() {
    let value = json!({
        "emoji": "🦀",
        "latin": "café",
        "line": "a\nb",
        "cjk": "雪",
    });
    assert_eq!(
        canonical_sha256(&value).expect("canonical digest"),
        "e6a8ac7ee8665bfdb1139b074bd1defbbe94f67eec3e22a273acc5ce5ed0016e"
    );
}

#[test]
fn canonical_hash_matches_python_float_rendering() {
    for (token, expected) in [
        (
            "1.0",
            "3b6b06ecd1c968c8e738e0f11c4bb361fca80a9a694de22fe66a05286afbd081",
        ),
        (
            "1e-7",
            "ff7a1315299260617fe404199e54e6d976a0b03e47da54fccec073c2fa48ff5c",
        ),
        (
            "1e-5",
            "a4aaff4cb2776bd827d3e6a14a54ccc534ca6b62e36579fc1c53b537c89752a4",
        ),
        (
            "1e16",
            "3b2cb112e050812d03ee42b0c83897293d662709d335a4f1ce7e59d29de23ac1",
        ),
        (
            "5e-324",
            "cf4df7d2834198d284947133d3ec7a66f3d94e8d447cac6a0783324211b36d43",
        ),
    ] {
        let number: Value = serde_json::from_str(token).expect("parse float fixture");
        let value = json!({"n": number});
        assert_eq!(canonical_sha256(&value).expect("float digest"), expected);
    }
}

#[test]
fn python_hashed_supported_number_domain_replays_and_indexes_exactly() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let first = one_event_log(&events);
    let payload = r#"{"numbers":[-9223372036854775808,18446744073709551615,-0,0,1.0,-0.0,-0e0,1.2300,1.234567890123456789,1E+06,1e-6,1e-324,-1e-324,5e-324,1.7976931348623157e308]}"#;
    let (expected_payload, expected_payload_digest) = python_canonical(payload);
    assert_eq!(
        expected_payload,
        r#"{"numbers":[-9223372036854775808,18446744073709551615,0,0,1.0,-0.0,-0.0,1.23,1.2345678901234567,1000000.0,1e-06,0.0,-0.0,5e-324,1.7976931348623157e+308]}"#
    );
    assert_eq!(
        expected_payload_digest,
        "94d699409d28b6f92bb4fa8d653ecc8cb5ecefdbd095c88551e6488bdc0f8dd3"
    );
    let tip = append_python_hashed_payload(&events, 2, &first, payload);

    let summary = replay(&events).expect("supported Python numbers replay");
    assert_eq!((summary.replay_count, summary.tip_sha256), (2, tip));
    let python = python_replay(&events);
    assert!(
        python.status.success(),
        "Python rejected supported numbers: {}",
        String::from_utf8_lossy(&python.stderr)
    );

    let index = scratch.0.join("numbers.sqlite");
    rebuild_index(&events, &index).expect("index supported Python numbers");
    let connection = Connection::open(&index).expect("open numeric index");
    let indexed_payload: String = connection
        .query_row(
            "SELECT payload_json FROM event_log WHERE sequence = 2",
            [],
            |row| row.get(0),
        )
        .expect("read indexed numeric payload");
    assert_eq!(indexed_payload, expected_payload);
}

#[test]
fn python_numbers_outside_observer_domain_fail_closed() {
    for (token, expected_error) in [
        ("18446744073709551616", "lossless observer domain"),
        ("-9223372036854775809", "lossless observer domain"),
        ("1e400", "non-finite number"),
        ("-1e400", "non-finite number"),
        ("NaN", "cannot parse event"),
        ("Infinity", "cannot parse event"),
        ("-Infinity", "cannot parse event"),
    ] {
        let scratch = Scratch::new();
        let events = scratch.events();
        let first = one_event_log(&events);
        append_python_hashed_payload(&events, 2, &first, &format!("{{\"number\":{token}}}"));
        let python = python_replay(&events);
        assert!(
            python.status.success(),
            "Python rejected its supported number {token}: {}",
            String::from_utf8_lossy(&python.stderr)
        );
        let error = replay(&events).expect_err("unsupported observer number must fail");
        assert!(
            error.to_string().contains(expected_error),
            "unexpected error for {token}: {error}"
        );
    }
}

#[test]
fn json_depth_boundary_matches_the_declared_observer_limit() {
    let accepted = Scratch::new();
    let accepted_events = accepted.events();
    let first = one_event_log(&accepted_events);
    let payload = nested_payload(MAX_JSON_CONTAINER_DEPTH - 2);
    append_python_hashed_payload(&accepted_events, 2, &first, &payload);
    replay(&accepted_events).expect("declared JSON nesting boundary must replay");
    assert!(python_replay(&accepted_events).status.success());

    let rejected = Scratch::new();
    let rejected_events = rejected.events();
    let first = one_event_log(&rejected_events);
    let payload = nested_payload(MAX_JSON_CONTAINER_DEPTH - 1);
    append_python_hashed_payload(&rejected_events, 2, &first, &payload);
    assert!(python_replay(&rejected_events).status.success());
    assert!(replay(&rejected_events)
        .expect_err("over-depth Python event must fail closed")
        .to_string()
        .contains("127-container JSON nesting limit"));
}

#[test]
fn unicode_scalar_pair_matches_python_but_lone_surrogate_fails_closed() {
    let raw = Scratch::new();
    let raw_events = raw.events();
    let first = one_event_log(&raw_events);
    let raw_tip = append_python_hashed_payload(&raw_events, 2, &first, r#"{"text":"🦀"}"#);
    replay(&raw_events).expect("raw Unicode scalar event");

    let escaped = Scratch::new();
    let escaped_events = escaped.events();
    let first = one_event_log(&escaped_events);
    let escaped_tip =
        append_python_hashed_payload(&escaped_events, 2, &first, r#"{"text":"\ud83e\udd80"}"#);
    replay(&escaped_events).expect("paired-surrogate Unicode scalar event");
    assert_eq!(raw_tip, escaped_tip);

    for payload in [
        r#"{"text":"bad\udc80byte"}"#,
        r#"{"text":"bad\ud800byte"}"#,
        r#"{"text":"bad\ud800\u0041byte"}"#,
    ] {
        let rejected = Scratch::new();
        let rejected_events = rejected.events();
        let first = one_event_log(&rejected_events);
        append_python_hashed_payload(&rejected_events, 2, &first, payload);
        assert!(python_replay(&rejected_events).status.success());
        assert!(replay(&rejected_events)
            .expect_err("lone surrogate must fail closed")
            .to_string()
            .contains("lone Unicode surrogate escape"));
    }

    let escaped_literal = Scratch::new();
    let escaped_literal_events = escaped_literal.events();
    let first = one_event_log(&escaped_literal_events);
    append_python_hashed_payload(&escaped_literal_events, 2, &first, r#"{"text":"\\ud800"}"#);
    replay(&escaped_literal_events).expect("escaped surrogate spelling is scalar-only text");
}

#[test]
fn malformed_numbers_are_reported_as_json_parse_errors() {
    for token in [
        "1e",
        "1e+",
        "1-2",
        "1e400x",
        "18446744073709551616x",
        "1e400 x",
        "18446744073709551616 x",
    ] {
        let scratch = Scratch::new();
        let events = scratch.events();
        let first = one_event_log(&events);
        let core = raw_event_core(2, &first, &format!("{{\"number\":{token}}}"));
        write_raw_event(&events, 2, &core, &"0".repeat(64));
        let error = replay(&events).expect_err("malformed JSON number must fail");
        assert!(
            error.to_string().contains("cannot parse event"),
            "malformed number {token} was mislabeled: {error}"
        );
    }
}

#[test]
fn event_timestamps_require_an_iso_timezone() {
    let valid = Scratch::new();
    let events = valid.events();
    one_event_log(&events);
    let path = events.join("00000000000000000001.json");
    let mut event: Value =
        serde_json::from_slice(&fs::read(&path).expect("read event")).expect("parse event");
    event["recorded_at"] = Value::String("2026-09-22 15:30:00+05:30".to_owned());
    let mut core = event.as_object().expect("event object").clone();
    core.remove("sha256");
    event["sha256"] = Value::String(
        canonical_sha256(&Value::Object(core)).expect("recompute timestamp event digest"),
    );
    fs::write(
        &path,
        serde_json::to_vec_pretty(&event).expect("encode event"),
    )
    .expect("write event");
    replay(&events).expect("timezone-aware ISO timestamp");

    event["recorded_at"] = Value::String("2026-09-22T10:00:00".to_owned());
    let mut core = event.as_object().expect("event object").clone();
    core.remove("sha256");
    event["sha256"] = Value::String(
        canonical_sha256(&Value::Object(core)).expect("recompute naive event digest"),
    );
    fs::write(
        &path,
        serde_json::to_vec_pretty(&event).expect("encode event"),
    )
    .expect("write event");
    assert!(replay(&events)
        .expect_err("timezone-naive event timestamp must fail")
        .to_string()
        .contains("timezone-naive"));
}

#[test]
fn timestamp_parser_matches_python_generated_matrix() {
    let dates = [
        "2026-09-22",
        "20260922",
        "2026-W39-2",
        "2026W392",
        "2026-W39",
        "2026W39",
    ];
    let separators = ["T", " ", "x", "+", "🐍", "\u{301}"];
    let times = [
        "00",
        "23",
        "0000",
        "2359",
        "00:00",
        "23:59",
        "000000",
        "235959",
        "00:00:00",
        "23:59:59",
        "10.1",
        "10,1234567",
        "1000.5",
        "10:00,5",
        "100000.1234567",
        "10:00:00,123",
        "10.",
        "10,",
        "123",
        "12345",
        "1234567",
        "12345678",
        "12:",
        "12x",
        "12:34:",
        "12:34x",
        "12:34:56:",
        "12:34:56:7",
        "10.123456x",
        "10.123456🐍",
        "12:34:56.123456:garbage",
        "123456123456x",
        "12:34:56:123456x",
    ];
    let zones = [
        "Z",
        "+00",
        "-00",
        "+0000",
        "-00:00",
        "+000000",
        "+00:00:00",
        "+23:59:59.9999999",
        "-235959,9",
        "+00:99",
        "+0099",
        "+23:00:99",
        "+12345678",
        "-23275192",
        "+12:34:56:7",
    ];
    let mut accepted = BTreeSet::new();
    for date in dates {
        for separator in separators {
            for time in times {
                for zone in zones {
                    accepted.insert(format!("{date}{separator}{time}{zone}"));
                }
            }
        }
    }
    accepted.insert("2026-09-22010:00:00+00".to_owned());
    accepted.insert("2026W392010:00:00+00".to_owned());
    accepted.insert("9999-W52-5T00Z".to_owned());

    let mut rejected = BTreeSet::new();
    for date in [
        "0000-01-01",
        "2026-02-29",
        "2026-09-31",
        "2026-13-01",
        "2026-W00-1",
        "2026-W54-1",
        "2026-W39-0",
        "2026-W39-8",
        "2026-266",
        "2026266",
        "2026-09",
        "9999-W52-6",
        "9999-W52-7",
        "9999W526",
        "9999W527",
    ] {
        rejected.insert(format!("{date}T10:00:00+00:00"));
    }
    rejected.insert("2026-W39-2010:00:00+00".to_owned());
    for time in [
        "0",
        "24",
        "2400",
        "24:00",
        "2360",
        "23:60",
        "235960",
        "23:59:60",
        "10:0000",
        "1000:00",
        "123456x5",
        "10.12345x",
        "12345612345x",
        "12:34:56:12345x",
    ] {
        rejected.insert(format!("2026-09-22T{time}+00:00"));
    }
    for zone in [
        "",
        "z",
        "+0",
        "+0:1",
        "+000",
        "+00:",
        "+00::00",
        "+0000:00",
        "+24",
        "+2400",
        "+23:60",
        "+235960",
        "+010203x5",
        "+01:02:03x5",
        "Z+00",
    ] {
        rejected.insert(format!("2026-09-22T10:00:00{zone}"));
    }
    assert!(accepted.is_disjoint(&rejected));

    let accepted_count = accepted.len();
    let values = accepted.into_iter().chain(rejected).collect::<Vec<_>>();
    assert_eq!(accepted_count, 17_823);
    assert_eq!(values.len(), 17_868);
    let python = python_timestamp_acceptance(&values);
    assert_eq!(python.len(), values.len());
    for (index, (value, python_accepts)) in values.iter().zip(python).enumerate() {
        let expected = index < accepted_count;
        assert_eq!(
            python_accepts, expected,
            "matrix classification disagrees with Python for {value:?}"
        );
        assert_eq!(
            parse_timestamp(value, "fixture timestamp").is_ok(),
            python_accepts,
            "Rust/Python timestamp mismatch for {value:?}"
        );
    }
}

#[test]
fn fully_hashed_event_timestamps_match_python_at_regression_boundaries() {
    for timestamp in [
        "2026-09-22T100000+00:00",
        "2026-09-22T1000+00:00",
        "2026-09-22T1000Z",
        "2026-09-22T123Z",
        "2026-09-22T12345+00:00",
        "20260922T10:00:00+00:00",
        "2026-W39-2🐍1000+00:00",
    ] {
        let scratch = Scratch::new();
        let events = scratch.events();
        one_event_log(&events);
        rewrite_first_event_timestamp_with_python_hash(&events, timestamp);
        let python = python_replay(&events);
        assert!(
            python.status.success(),
            "Python rejected valid fully-hashed event timestamp {timestamp:?}: {}",
            String::from_utf8_lossy(&python.stderr)
        );
        replay(&events)
            .unwrap_or_else(|error| panic!("Rust rejected event timestamp {timestamp:?}: {error}"));
    }

    for timestamp in [
        "2026-09-22T10:00:00z",
        "2016-12-31T23:59:60Z",
        "2026-09-22T10:00:00+0:1",
        "2026-09-22X10:00+0:1",
        "2026-09-22T10:00:00+00::00",
    ] {
        let scratch = Scratch::new();
        let events = scratch.events();
        one_event_log(&events);
        rewrite_first_event_timestamp_with_python_hash(&events, timestamp);
        assert!(
            !python_replay(&events).status.success(),
            "Python accepted invalid fully-hashed event timestamp {timestamp:?}"
        );
        assert!(
            replay(&events).is_err(),
            "Rust accepted invalid fully-hashed event timestamp {timestamp:?}"
        );
    }
}

#[test]
fn unknown_event_kind_fails_closed_after_valid_envelope() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let first = one_event_log(&events);
    append_event(&events, 2, &first, "future-state-transition", json!({}));
    assert!(replay(&events)
        .expect_err("unknown event kind must fail")
        .to_string()
        .contains("unknown event kind"));
}

#[test]
fn known_non_state_event_kinds_are_tolerated_after_envelope_validation() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let mut previous = one_event_log(&events);
    let kinds = [
        "handoff-read",
        "handoff-removed",
        "handoff-write-completed",
        "handoff-write-intended",
        "handoff-written",
        "legacy-validate-checkout-removed",
        "operation-completed",
        "operation-progress-recorded",
        "ownerless-agent-cache-relocated",
        "ownerless-agent-worktree-removed",
        "ownerless-validate-path-removed",
        "partial-updates-recovered",
        "reclaim-started",
        "recovery-started",
        "retirement-attempted",
        "slot-held",
        "slot-hold-released",
    ];
    for (index, kind) in kinds.iter().enumerate() {
        previous = append_event(
            &events,
            u64::try_from(index).expect("test index") + 2,
            &previous,
            kind,
            json!({}),
        );
    }
    let summary = replay(&events).expect("known non-state kinds replay");
    assert_eq!(summary.replay_count, 18);
    assert_eq!(summary.tip_sha256, previous);
    assert_eq!(summary.active_revision, 3);
    assert_eq!(summary.archive_revision, 7);
}

#[test]
fn index_rebuild_materializes_and_reads_summary() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let tip = one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");

    let rebuilt = rebuild_index(&events, &index).expect("rebuild index");
    assert_eq!(rebuilt.replay_count, 1);
    assert_eq!(rebuilt.tip_sha256, tip);
    assert_eq!(rebuilt.active_count, 1);
    assert_eq!(read_index(&index).expect("read index"), rebuilt);
}

#[test]
fn event_replaced_by_fifo_after_enumeration_fails_without_blocking() {
    let scratch = Scratch::new();
    let events = scratch.events();
    one_event_log(&events);
    let event = events.join("00000000000000000001.json");

    let error = replay_with_post_count_hook(&events, || {
        fs::remove_file(&event).expect("remove enumerated event");
        let status = Command::new("mkfifo")
            .arg("--")
            .arg(&event)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo failed with {status}");
    })
    .expect_err("post-enumeration FIFO substitution must fail closed");
    assert!(
        error.to_string().contains("not a regular file"),
        "unexpected FIFO error: {error}"
    );
}

#[test]
fn failed_first_rebuild_leaves_no_terminal_index() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let index = scratch.0.join("observer.sqlite");

    let error = rebuild_index(&events, &index).expect_err("empty replay must fail");
    assert!(error.to_string().contains("event log") && error.to_string().contains("empty"));
    assert!(!index.exists(), "failed first rebuild published an index");
    let leftovers = fs::read_dir(&scratch.0)
        .expect("list scratch directory")
        .map(|entry| entry.expect("read scratch entry").file_name())
        .filter(|name| name.to_string_lossy().contains("wrkslotsd-private"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "private index leftovers: {leftovers:?}"
    );
}

#[test]
fn failed_late_first_rebuild_leaves_no_partial_terminal_index() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let first_tip = one_event_log(&events);
    append_event(&events, 2, &first_tip, "operation-completed", json!({}));
    let second = events.join("00000000000000000002.json");
    let mut damaged: Value =
        serde_json::from_slice(&fs::read(&second).expect("read event")).expect("parse event");
    damaged["sha256"] = Value::String("f".repeat(64));
    fs::write(
        &second,
        serde_json::to_vec_pretty(&damaged).expect("encode damaged event"),
    )
    .expect("write damaged event");
    let index = scratch.0.join("observer.sqlite");

    let error = rebuild_index(&events, &index).expect_err("late first replay failure must fail");
    assert!(error.to_string().contains("digest does not match"));
    assert!(
        !index.exists(),
        "failed late first rebuild published a partial index"
    );
    let leftovers = fs::read_dir(&scratch.0)
        .expect("list scratch directory")
        .map(|entry| entry.expect("read scratch entry").file_name())
        .filter(|name| name.to_string_lossy().contains("wrkslotsd-private"))
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "private index leftovers: {leftovers:?}"
    );
}

#[test]
fn initial_index_is_absent_until_complete_database_is_publishable() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let tip = one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    let ready = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker_events = events.clone();
    let worker_index = index.clone();
    let worker_ready = Arc::clone(&ready);
    let worker_release = Arc::clone(&release);
    let worker = thread::spawn(move || {
        rebuild_index_with_prepublication_hook(&worker_events, &worker_index, || {
            worker_ready.wait();
            worker_release.wait();
        })
    });

    ready.wait();
    assert!(
        !index.exists(),
        "terminal path became visible before publication"
    );
    assert!(read_index(&index)
        .expect_err("unpublished index must be absent")
        .to_string()
        .contains("does not exist"));
    release.wait();
    let summary = worker
        .join()
        .expect("initial rebuild worker did not panic")
        .expect("publish completed index");
    assert_eq!(summary.tip_sha256, tip);
    assert_eq!(read_index(&index).expect("read published index"), summary);
}

#[test]
fn initial_index_is_complete_at_its_first_visible_instant() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let tip = one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    let ready = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let worker_events = events.clone();
    let worker_index = index.clone();
    let worker_ready = Arc::clone(&ready);
    let worker_release = Arc::clone(&release);
    let worker = thread::spawn(move || {
        rebuild_index_with_postpublication_hook(&worker_events, &worker_index, |private_path| {
            assert!(
                !private_path.exists(),
                "atomic rename retained its private source name"
            );
            worker_ready.wait();
            worker_release.wait();
        })
    });

    ready.wait();
    let visible = read_index(&index).expect("newly visible index must already be complete");
    assert_eq!(visible.tip_sha256, tip);
    let private_siblings = fs::read_dir(&scratch.0)
        .expect("list scratch directory")
        .map(|entry| entry.expect("read scratch entry").file_name())
        .filter(|name| name.to_string_lossy().contains("wrkslotsd-private"))
        .collect::<Vec<_>>();
    assert!(
        private_siblings.is_empty(),
        "published index retained private siblings: {private_siblings:?}"
    );
    release.wait();
    let summary = worker
        .join()
        .expect("initial rebuild worker did not panic")
        .expect("finish initial publication");
    assert_eq!(summary, visible);
}

#[test]
fn index_preserves_python_accepted_u64_revisions_losslessly() {
    let scratch = Scratch::new();
    let events = scratch.events();
    append_event(
        &events,
        1,
        &"0".repeat(64),
        "state-imported",
        imported_state(vec![active_record("slot-a", 1)], u64::MAX, vec![], u64::MAX),
    );
    let python = python_replay(&events);
    assert!(
        python.status.success(),
        "Python rejected maximum u64 revisions: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let replayed = replay(&events).expect("Rust replays maximum u64 revisions");
    assert_eq!(replayed.active_revision, u64::MAX);
    assert_eq!(replayed.archive_revision, u64::MAX);

    let index = scratch.0.join("observer.sqlite");
    let rebuilt = rebuild_index(&events, &index).expect("index maximum u64 revisions");
    assert_eq!(rebuilt, replayed);
    assert_eq!(
        read_index(&index).expect("read maximum revisions"),
        replayed
    );

    let connection = Connection::open(&index).expect("open maximum-revision index");
    let (active, active_type, archive, archive_type): (String, String, String, String) = connection
        .query_row(
            "SELECT active_revision, typeof(active_revision), archive_revision, typeof(archive_revision) FROM observer_metadata",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read lossless revision cells");
    assert_eq!(active, u64::MAX.to_string());
    assert_eq!(archive, u64::MAX.to_string());
    assert_eq!(
        (active_type.as_str(), archive_type.as_str()),
        ("text", "text")
    );
}

#[test]
fn failed_transaction_does_not_partially_replace_previous_index() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let first_tip = one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    let original = rebuild_index(&events, &index).expect("initial rebuild");

    append_event(&events, 2, &first_tip, "operation-completed", json!({}));
    let second = events.join("00000000000000000002.json");
    let mut damaged: Value =
        serde_json::from_slice(&fs::read(&second).expect("read event")).expect("parse event");
    damaged["sha256"] = Value::String("f".repeat(64));
    fs::write(
        &second,
        serde_json::to_vec_pretty(&damaged).expect("encode damaged event"),
    )
    .expect("write damaged event");

    let error = rebuild_index(&events, &index).expect_err("late replay failure must roll back");
    assert!(error.to_string().contains("digest does not match"));
    assert_eq!(
        read_index(&index).expect("old index remains readable"),
        original
    );

    let connection = Connection::open(&index).expect("reopen derived index");
    let event_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM event_log", [], |row| row.get(0))
        .expect("count retained event rows");
    assert_eq!(event_count, 1);
    let active_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM active_records", [], |row| row.get(0))
        .expect("count retained active rows");
    assert_eq!(active_count, 1);
}

#[test]
fn complete_nested_schema_rejects_malformed_records_and_naive_time() {
    let missing_field = Scratch::new();
    let events = missing_field.events();
    let mut record = active_record("slot-a", 1);
    record["checkouts"][0]
        .as_object_mut()
        .expect("checkout object")
        .remove("head");
    import_log(&events, vec![record], vec![]);
    assert!(replay(&events)
        .expect_err("missing nested field must fail")
        .to_string()
        .contains("invalid fields"));

    let naive_time = Scratch::new();
    let events = naive_time.events();
    let mut record = active_record("slot-a", 1);
    record["heartbeat_at"] = Value::String("2026-09-22T09:30:00".to_owned());
    import_log(&events, vec![record], vec![]);
    assert!(replay(&events)
        .expect_err("timezone-naive record must fail")
        .to_string()
        .contains("timezone-naive"));

    let bad_archive = Scratch::new();
    let events = bad_archive.events();
    let mut record = archive_record("slot-old", 1);
    record["physical_storage"] = Value::String("present".to_owned());
    import_log(&events, vec![], vec![record]);
    assert!(replay(&events)
        .expect_err("invalid archive storage must fail")
        .to_string()
        .contains("removed physical storage"));
}

#[test]
fn active_agent_uniqueness_honors_import_exception_and_removal() {
    let duplicate = Scratch::new();
    let events = duplicate.events();
    import_log(
        &events,
        vec![
            active_record_for("slot-a", "worker", 1, "agent"),
            active_record_for("slot-b", "worker", 1, "agent"),
        ],
        vec![],
    );
    assert!(replay(&events)
        .expect_err("ordinary duplicate agent must fail")
        .to_string()
        .contains("owns both active slots"));
    assert!(!python_replay(&events).status.success());

    let imported = Scratch::new();
    let events = imported.events();
    import_log(
        &events,
        vec![
            imported_active_record("slot-a", "worker", 1),
            imported_active_record("slot-b", "worker", 1),
        ],
        vec![],
    );
    assert_eq!(
        replay(&events)
            .expect("historical imports do not claim live agent uniqueness")
            .active_count,
        2
    );
    let python = python_replay(&events);
    assert!(
        python.status.success(),
        "Python rejected duplicate historical agents: {}",
        String::from_utf8_lossy(&python.stderr)
    );

    let removal = Scratch::new();
    let events = removal.events();
    let record = active_record_for("slot-a", "worker", 1, "agent");
    let first = import_log(&events, vec![record.clone()], vec![]);
    let second = append_event(
        &events,
        2,
        &first,
        "active-state-recorded",
        json!({
            "action": "released",
            "slot": "slot-a",
            "previous_revision": 0,
            "revision": 1,
            "previous_record_sha256": canonical_sha256(&record).expect("record digest"),
            "record": null,
            "evidence": {},
        }),
    );
    append_event(
        &events,
        3,
        &second,
        "active-state-recorded",
        json!({
            "action": "allocated",
            "slot": "slot-b",
            "previous_revision": 1,
            "revision": 2,
            "previous_record_sha256": null,
            "record": active_record_for("slot-b", "worker", 1, "agent"),
            "evidence": {},
        }),
    );
    let summary = replay(&events).expect("removal releases live agent identity");
    assert_eq!((summary.active_revision, summary.active_count), (2, 1));

    let conflicting_update = Scratch::new();
    let events = conflicting_update.events();
    let slot_a = active_record_for("slot-a", "worker", 1, "agent");
    let slot_b = active_record_for("slot-b", "other", 1, "agent");
    let first = import_log(&events, vec![slot_a, slot_b.clone()], vec![]);
    append_event(
        &events,
        2,
        &first,
        "active-state-recorded",
        json!({
            "action": "heartbeat",
            "slot": "slot-b",
            "previous_revision": 0,
            "revision": 1,
            "previous_record_sha256": canonical_sha256(&slot_b).expect("record digest"),
            "record": active_record_for("slot-b", "worker", 1, "agent"),
            "evidence": {},
        }),
    );
    assert!(replay(&events)
        .expect_err("agent-conflicting update must fail")
        .to_string()
        .contains("already owns another active slot"));
}

fn append_recovery_pair(events: &Path, active: &Value, slot_type: &str, action: &str) -> String {
    let first = import_log(events, vec![active.clone()], vec![]);
    let archived = archive_record_for("slot-a", 1, slot_type);
    let archive_id = archived["archive_id"].as_str().expect("archive id");
    let source_record_sha256 = canonical_sha256(active).expect("source record digest");
    let evidence = if action == "absent-validation-row-recovered" {
        json!({
            "archive_id": archive_id,
            "source_record_sha256": source_record_sha256,
            "validation_outcome": "unknown",
        })
    } else {
        json!({
            "archive_id": archive_id,
            "source_record_sha256": source_record_sha256,
            "physical_storage": "externally-absent",
        })
    };
    let second = append_event(
        events,
        2,
        &first,
        "archive-state-recorded",
        json!({
            "action": action,
            "slot": "slot-a",
            "previous_revision": 0,
            "revision": 1,
            "record": archived,
            "evidence": evidence.clone(),
        }),
    );
    append_event(
        events,
        3,
        &second,
        "active-state-recorded",
        json!({
            "action": action,
            "slot": "slot-a",
            "previous_revision": 0,
            "revision": 1,
            "previous_record_sha256": source_record_sha256,
            "record": null,
            "evidence": evidence,
        }),
    )
}

#[test]
fn absent_recovery_evidence_is_exact_for_both_slot_types() {
    for (slot_type, action) in [
        ("validate", "absent-validation-row-recovered"),
        ("agent", "absent-agent-row-recovered"),
    ] {
        let scratch = Scratch::new();
        let events = scratch.events();
        let active = active_record_for("slot-a", "worker", 1, slot_type);
        append_recovery_pair(&events, &active, slot_type, action);
        let summary = replay(&events).expect("typed absent-row recovery");
        assert_eq!((summary.active_count, summary.archive_count), (0, 1));
        let python = python_replay(&events);
        assert!(
            python.status.success(),
            "Python rejected typed recovery: {}",
            String::from_utf8_lossy(&python.stderr)
        );
    }

    let wrong_source = Scratch::new();
    let events = wrong_source.events();
    let active = active_record_for("slot-a", "worker", 1, "validate");
    append_recovery_pair(
        &events,
        &active,
        "validate",
        "absent-validation-row-recovered",
    );
    let path = events.join("00000000000000000003.json");
    let mut event: Value = serde_json::from_slice(&fs::read(&path).expect("read recovery event"))
        .expect("parse recovery event");
    event["payload"]["evidence"]["source_record_sha256"] = Value::String("f".repeat(64));
    let mut core = event.as_object().expect("event object").clone();
    core.remove("sha256");
    event["sha256"] =
        Value::String(canonical_sha256(&Value::Object(core)).expect("recompute envelope digest"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&event).expect("encode event"),
    )
    .expect("write event");
    assert!(replay(&events)
        .expect_err("wrong recovery source digest must fail")
        .to_string()
        .contains("source record"));
    assert!(!python_replay(&events).status.success());

    let wrong_type = Scratch::new();
    let events = wrong_type.events();
    let active = active_record_for("slot-a", "worker", 1, "validate");
    append_recovery_pair(&events, &active, "validate", "absent-agent-row-recovered");
    assert!(replay(&events)
        .expect_err("agent recovery for validation row must fail")
        .to_string()
        .contains("archive agent recovery evidence does not match its record"));
    assert!(!python_replay(&events).status.success());
}

#[test]
fn older_and_forked_replays_cannot_replace_a_newer_index() {
    let source = Scratch::new();
    let events = source.events();
    let first = one_event_log(&events);
    append_event(&events, 2, &first, "operation-completed", json!({}));
    let index = source.0.join("observer.sqlite");
    let indexed = rebuild_index(&events, &index).expect("index two events");

    let older = Scratch::new();
    let older_events = older.events();
    one_event_log(&older_events);
    assert!(rebuild_index(&older_events, &index)
        .expect_err("older replay must fail")
        .to_string()
        .contains("older"));
    assert_eq!(read_index(&index).expect("newer index retained"), indexed);

    let forked = Scratch::new();
    let forked_events = forked.events();
    let fork_first = one_event_log(&forked_events);
    append_event(
        &forked_events,
        2,
        &fork_first,
        "operation-completed",
        json!({"different": true}),
    );
    assert!(rebuild_index(&forked_events, &index)
        .expect_err("forked replay must fail")
        .to_string()
        .contains("does not extend"));
    assert_eq!(
        read_index(&index).expect("unforked index retained"),
        indexed
    );

    let second_tip = indexed.tip_sha256.clone();
    append_event(&events, 3, &second_tip, "operation-completed", json!({}));
    let extended = rebuild_index(&events, &index).expect("strict extension replaces index");
    assert_eq!(extended.replay_count, 3);
}

#[test]
fn incompatible_index_schema_is_not_overwritten() {
    let scratch = Scratch::new();
    let events = scratch.events();
    one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    rebuild_index(&events, &index).expect("build index");
    let connection = Connection::open(&index).expect("open index");
    connection
        .execute("UPDATE observer_metadata SET schema_version = 99", [])
        .expect("install incompatible schema marker");
    drop(connection);
    assert!(rebuild_index(&events, &index)
        .expect_err("incompatible index must not be overwritten")
        .to_string()
        .contains("unsupported derived index schema 99"));
    let connection = Connection::open(&index).expect("reopen incompatible index");
    let schema: u64 = connection
        .query_row(
            "SELECT schema_version FROM observer_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("read retained schema marker");
    assert_eq!(schema, 99);
}

#[test]
fn concurrent_rebuilders_cannot_publish_a_partial_index() {
    let scratch = Scratch::new();
    let events = scratch.events();
    one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let barrier = Arc::clone(&barrier);
        let events = events.clone();
        let index = index.clone();
        workers.push(thread::spawn(move || {
            rebuild_index_with_prepublication_hook(&events, &index, || {
                barrier.wait();
            })
        }));
    }
    let results = workers
        .into_iter()
        .map(|worker| worker.join().expect("rebuild worker did not panic"))
        .collect::<Vec<_>>();
    assert!(
        results.iter().all(Result::is_ok),
        "concurrent rebuild failures: {results:?}"
    );
    let summary = read_index(&index).expect("concurrent rebuild left one complete index");
    assert_eq!((summary.replay_count, summary.active_count), (1, 1));
}

#[test]
fn index_symlinks_and_hardlinks_fail_closed() {
    use std::os::unix::fs::symlink;

    let scratch = Scratch::new();
    let events = scratch.events();
    one_event_log(&events);
    let index = scratch.0.join("observer.sqlite");
    rebuild_index(&events, &index).expect("build source index");

    let symbolic = scratch.0.join("symbolic.sqlite");
    symlink(&index, &symbolic).expect("create index symlink");
    assert!(read_index(&symbolic)
        .expect_err("index symlink must fail")
        .to_string()
        .contains("not a real regular file"));

    let hard = scratch.0.join("hard.sqlite");
    fs::hard_link(&index, &hard).expect("create index hard link");
    assert!(read_index(&hard)
        .expect_err("index hard link must fail")
        .to_string()
        .contains("multiple hard links"));
    assert!(rebuild_index(&events, &index)
        .expect_err("linked index rebuild must fail")
        .to_string()
        .contains("multiple hard links"));
}

#[test]
fn index_inside_event_directory_is_rejected_without_touching_events() {
    let scratch = Scratch::new();
    let events = scratch.events();
    one_event_log(&events);
    let index = events.join("observer.sqlite");
    assert!(rebuild_index(&events, &index)
        .expect_err("index inside event authority must fail")
        .to_string()
        .contains("must not be placed"));
    assert!(!index.exists());
}

#[test]
fn event_size_limit_accepts_the_exact_boundary_and_rejects_one_byte_more() {
    let scratch = Scratch::new();
    let events = scratch.events();
    let path = events.join("00000000000000000001.json");
    one_event_log(&events);
    let mut event: Value =
        serde_json::from_slice(&fs::read(&path).expect("read event")).expect("parse event");
    event["payload"]["active"]["slots"][0]["purpose"] = Value::String(String::new());
    let empty = serde_json::to_vec_pretty(&event).expect("encode empty-padding event");
    let limit = usize::try_from(MAX_EVENT_BYTES).expect("event limit fits usize");
    let padding = limit
        .checked_sub(empty.len())
        .expect("fixture envelope fits under event limit");
    event["payload"]["active"]["slots"][0]["purpose"] = Value::String("x".repeat(padding));
    let mut core = event.as_object().expect("event object").clone();
    core.remove("sha256");
    event["sha256"] =
        Value::String(canonical_sha256(&Value::Object(core)).expect("hash exact-limit event"));
    let encoded = serde_json::to_vec_pretty(&event).expect("encode exact-limit event");
    assert_eq!(encoded.len(), limit);
    fs::write(&path, encoded).expect("write exact-limit event");
    replay(&events).expect("exact event-size limit must be accepted");

    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("open exact-limit event")
        .write_all(b" ")
        .expect("extend event beyond limit");
    assert!(replay(&events)
        .expect_err("oversized event must fail")
        .to_string()
        .contains("replay limit"));
}

#[test]
fn python_and_rust_agree_on_valid_records_and_adversarial_schema() {
    let valid = Scratch::new();
    let events = valid.events();
    import_log(
        &events,
        vec![active_record("slot-a", 1)],
        vec![archive_record("slot-old", 2)],
    );
    let python = python_replay(&events);
    assert!(
        python.status.success(),
        "Python rejected valid fixture: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let python: Value = serde_json::from_slice(&python.stdout).expect("parse Python result");
    let index = valid.0.join("differential.sqlite");
    let rust = rebuild_index(&events, &index).expect("Rust accepts valid fixture");
    assert_eq!(python["machine"], rust.machine);
    assert_eq!(python["replay_count"], rust.replay_count);
    assert_eq!(python["tip_sha256"], rust.tip_sha256);
    assert_eq!(python["active_revision"], rust.active_revision);
    assert_eq!(python["archive_revision"], rust.archive_revision);

    let connection = Connection::open(&index).expect("open differential index");
    let mut active_statement = connection
        .prepare("SELECT record_json FROM active_records ORDER BY slot")
        .expect("prepare active query");
    let active = active_statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query active records")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("read active record"))
                .expect("parse active record")
        })
        .collect::<Vec<_>>();
    assert_eq!(python["active_records"], Value::Array(active));
    let mut archive_statement = connection
        .prepare("SELECT record_json FROM archive_records ORDER BY rowid")
        .expect("prepare archive query");
    let archive = archive_statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query archive records")
        .map(|row| {
            serde_json::from_str::<Value>(&row.expect("read archive record"))
                .expect("parse archive record")
        })
        .collect::<Vec<_>>();
    assert_eq!(python["archive_records"], Value::Array(archive));

    for damage in ["nested-checkout", "duplicate-agent", "naive-timestamp"] {
        let rejected = Scratch::new();
        let events = rejected.events();
        let mut first = active_record_for("slot-a", "worker", 1, "agent");
        let mut records = vec![first.clone()];
        match damage {
            "nested-checkout" => {
                first["checkouts"][0]
                    .as_object_mut()
                    .expect("checkout object")
                    .remove("head");
                records = vec![first];
            }
            "duplicate-agent" => {
                records.push(active_record_for("slot-b", "worker", 1, "agent"));
            }
            "naive-timestamp" => {
                first["created_at"] = Value::String("2026-09-22T09:00:00".to_owned());
                records = vec![first];
            }
            _ => unreachable!(),
        }
        import_log(&events, records, vec![]);
        assert!(replay(&events).is_err(), "Rust accepted {damage}");
        let python = python_replay(&events);
        assert!(!python.status.success(), "Python accepted {damage}");
    }
}

#[test]
fn python_normalized_active_records_drive_storage_and_following_hashes() {
    let scratch = Scratch::new();
    let events = scratch.events();

    let mut omitted_slot_type = active_record_for("slot-a", "worker-a", 1, "agent");
    omitted_slot_type
        .as_object_mut()
        .expect("active record object")
        .remove("slot_type");
    let mut normalized_slot_type = omitted_slot_type.clone();
    normalized_slot_type["slot_type"] = Value::String("agent".to_owned());

    let mut null_import = active_record_for("slot-b", "worker-b", 1, "agent");
    null_import["import_source"] = Value::Null;
    let mut normalized_import = null_import.clone();
    normalized_import
        .as_object_mut()
        .expect("active record object")
        .remove("import_source");

    let first = import_log(
        &events,
        vec![omitted_slot_type.clone(), null_import.clone()],
        vec![],
    );
    let mut next_slot_type = omitted_slot_type;
    next_slot_type["heartbeat_at"] = Value::String("2026-09-22T09:31:00+00:00".to_owned());
    let second = append_event(
        &events,
        2,
        &first,
        "active-state-recorded",
        json!({
            "action": "heartbeat",
            "slot": "slot-a",
            "previous_revision": 0,
            "revision": 1,
            "previous_record_sha256": canonical_sha256(&normalized_slot_type)
                .expect("normalized slot-type digest"),
            "record": next_slot_type,
            "evidence": {},
        }),
    );
    let mut next_import = null_import;
    next_import["heartbeat_at"] = Value::String("2026-09-22T09:31:00+00:00".to_owned());
    append_event(
        &events,
        3,
        &second,
        "active-state-recorded",
        json!({
            "action": "heartbeat",
            "slot": "slot-b",
            "previous_revision": 1,
            "revision": 2,
            "previous_record_sha256": canonical_sha256(&normalized_import)
                .expect("normalized import-source digest"),
            "record": next_import,
            "evidence": {},
        }),
    );

    let python = python_replay(&events);
    assert!(
        python.status.success(),
        "Python rejected normalization fixture: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let python: Value = serde_json::from_slice(&python.stdout).expect("parse Python result");
    let summary = replay(&events).expect("Rust accepts Python-normalized transitions");
    assert_eq!((summary.active_revision, summary.active_count), (2, 2));
    let index = scratch.0.join("normalized.sqlite");
    rebuild_index(&events, &index).expect("index normalized active state");
    let connection = Connection::open(&index).expect("open normalized index");
    let stored = connection
        .prepare("SELECT record_json FROM active_records ORDER BY slot")
        .expect("prepare normalized record query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query normalized records")
        .map(|row| serde_json::from_str::<Value>(&row.expect("read record")).expect("parse record"))
        .collect::<Vec<_>>();
    assert_eq!(python["active_records"], Value::Array(stored.clone()));
    assert!(stored.iter().all(|record| record["slot_type"] == "agent"));
    assert!(stored
        .iter()
        .all(|record| record.get("import_source").is_none()));

    let rejected = Scratch::new();
    let rejected_events = rejected.events();
    let mut raw = active_record_for("slot-a", "worker-a", 1, "agent");
    raw.as_object_mut()
        .expect("active record object")
        .remove("slot_type");
    let first = import_log(&rejected_events, vec![raw.clone()], vec![]);
    append_event(
        &rejected_events,
        2,
        &first,
        "active-state-recorded",
        json!({
            "action": "heartbeat",
            "slot": "slot-a",
            "previous_revision": 0,
            "revision": 1,
            "previous_record_sha256": canonical_sha256(&raw).expect("raw record digest"),
            "record": raw,
            "evidence": {},
        }),
    );
    assert!(replay(&rejected_events).is_err());
    assert!(!python_replay(&rejected_events).status.success());
}
