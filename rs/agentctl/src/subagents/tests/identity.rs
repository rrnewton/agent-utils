use super::*;

fn registry(fixture: &Fixture) -> PathBuf {
    fixture.root.join("registry")
}

fn record(fixture: &Fixture, agent_name: &str) -> Value {
    agent::read_private_json(&registry(fixture).join(agent_name).join("agent.json")).unwrap()
}

fn write_record(fixture: &Fixture, agent_name: &str, document: &Value) {
    let directory = registry(fixture).join(agent_name);
    if !directory.exists() {
        DirBuilder::new().mode(0o700).create(&directory).unwrap();
    }
    agent::atomic_json(&directory.join("agent.json"), document).unwrap();
}

/// A second record that claims the same pane as `worker`, as a corrupted registry would.
fn write_claiming_record(fixture: &Fixture, agent_name: &str) {
    let mut other = record(fixture, "worker");
    other["name"] = json!(agent_name);
    other["token"] = json!(format!("{agent_name}-generation"));
    other["terminal_id"] = Value::Null;
    write_record(fixture, agent_name, &other);
}

fn failed_documents(fixture: &Fixture, agent_name: &str) -> Vec<Value> {
    let failed = registry(fixture).join(agent_name).join("queue/failed");
    let Ok(entries) = fs::read_dir(&failed) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap())
        .collect()
}

fn journal_count(fixture: &Fixture) -> usize {
    fs::read_dir(registry(fixture).join(".renames")).map_or(0, |entries| {
        entries
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .count()
    })
}

/// Make `worker` look like a record written before anchors existed.
fn strip_anchors(fixture: &Fixture, agent_name: &str) {
    let mut document = record(fixture, agent_name);
    for key in [
        "terminal_id",
        "harness_identity",
        "session_agent",
        "session_value",
    ] {
        document[key] = Value::Null;
    }
    write_record(fixture, agent_name, &document);
    fixture
        .client
        .report_session
        .store(false, Ordering::Relaxed);
}

fn send(fixture: &Fixture, agent_name: &str, text: &str) -> Result<QueueResult> {
    fixture
        .manager()
        .send(agent_name, text, DrainOptions::default())
}

fn runs(fixture: &Fixture) -> Vec<String> {
    fixture.client.runs.lock().unwrap().clone()
}

fn label(fixture: &Fixture, tab: &str) -> String {
    fixture.client.labels.lock().unwrap()[tab].clone()
}

fn history_names(document: &Value) -> Vec<String> {
    document["name_history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect()
}

/// Rename `worker` to `reviewer` and fail inside it after the journal and the Herdr name.
fn crash_rename(fixture: &Fixture) {
    fixture.client.fail_rename_tab.store(true, Ordering::SeqCst);
    let error = fixture
        .manager()
        .rename("worker", "reviewer")
        .expect_err("injected crash");
    assert!(error.to_string().contains("simulated crash"), "{error}");
    fixture
        .client
        .fail_rename_tab
        .store(false, Ordering::SeqCst);
    assert_eq!(journal_count(fixture), 1);
}

fn findings(report: &Value, agent_name: &str) -> Vec<String> {
    report["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == agent_name)
        .unwrap_or_else(|| panic!("no doctor row for {agent_name}: {report}"))["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding.as_str().unwrap().to_owned())
        .collect()
}

// Anchors recorded by start.

#[test]
fn start_pins_terminal_and_harness_process_and_writes_the_shared_fields() {
    let fixture = Fixture::new();
    fixture.start(None);
    let document = record(&fixture, "worker");
    assert_eq!(document["terminal_id"], "term-owned");
    assert_eq!(document["harness_identity"]["pid"], 300);
    // Like the Python edition's dataclass serialization, the list is always present.
    assert_eq!(document["name_history"], json!([]));
}

#[test]
fn start_refuses_a_pane_another_record_already_claims() {
    let fixture = Fixture::new();
    fixture.start(None);
    let mut other = record(&fixture, "worker");
    other["name"] = json!("other");
    other["token"] = json!("other-generation");
    // Only the pane and terminal claims remain; the native-session check is separate.
    other["session_agent"] = Value::Null;
    other["session_value"] = Value::Null;
    // The live record moves away, so only the claim on pane `owned` remains.
    fs::rename(
        registry(&fixture).join("worker"),
        registry(&fixture).join("other"),
    )
    .unwrap();
    agent::atomic_json(&registry(&fixture).join("other/agent.json"), &other).unwrap();
    // The fake reallocates pane `owned` for the next start.
    fixture.client.panes.lock().unwrap().clear();
    let error = fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                ..StartOptions::default()
            },
        )
        .expect_err("duplicate pane claim");
    assert!(
        error.to_string().contains("already registered as 'other'"),
        "{error}"
    );
}

// The guard before and after each effect.

#[test]
fn a_harness_replaced_before_send_types_nothing_and_stays_pending() {
    let fixture = Fixture::new();
    fixture.start(None);
    // Another program now runs in the same terminal.
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert("owned".to_owned(), 999);
    let error = send(&fixture, "worker", "for the original agent").expect_err("refused");
    assert_eq!(
        error.outcome(),
        Some(agent::QueueOutcome::Pending),
        "{error}"
    );
    assert!(
        error.to_string().contains("no longer a foreground process"),
        "{error}"
    );
    assert!(!runs(&fixture).contains(&"for the original agent".to_owned()));
    assert!(fixture.client.expected_terminals.lock().unwrap().is_empty());
    assert!(failed_documents(&fixture, "worker").is_empty());
}

fn misroutes(fixture: &Fixture, agent_name: &str) -> Vec<Value> {
    let path = registry(fixture).join(agent_name).join("misroutes.jsonl");
    fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_pane_swap_between_check_and_send_interrupts_the_wrong_agent_and_retries() {
    let fixture = Fixture::new();
    fixture.start(None);
    let pane = record(&fixture, "worker")["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let original = fixture.client.harness_pids.lock().unwrap()[&pane];
    fixture
        .client
        .replace_harness_before_effect
        .store(true, Ordering::SeqCst);
    let error = send(&fixture, "worker", "work meant for the worker agent").expect_err("pending");
    assert_eq!(
        error.outcome(),
        Some(agent::QueueOutcome::Pending),
        "{error}"
    );
    assert_eq!(
        *fixture.client.keys_sent.lock().unwrap(),
        [(pane.clone(), "esc".to_owned())]
    );
    let all = runs(&fixture);
    assert!(all.contains(&"work meant for the worker agent".to_owned()));
    assert_eq!(all.last().map(String::as_str), Some(MISROUTE_NOTE));
    let entries = misroutes(&fixture, "worker");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["detection"], "identity-changed-after-write");
    assert_eq!(entries[0]["interrupted"], true);
    assert_eq!(entries[0]["note_sent"], true);
    assert!(entries[0]["message_id"].is_string(), "{}", entries[0]);
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert(pane, original);
    fixture
        .manager()
        .drain("worker", DrainOptions::default())
        .unwrap();
    assert_eq!(
        runs(&fixture).last().map(String::as_str),
        Some("work meant for the worker agent")
    );
}

#[test]
fn a_second_misroute_of_one_message_is_quarantined() {
    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .replace_harness_before_effect
        .store(true, Ordering::SeqCst);
    send(&fixture, "worker", "a message that keeps going astray").expect_err("pending");
    let pane = record(&fixture, "worker")["pane_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let recorded = record(&fixture, "worker")["harness_identity"]["pid"]
        .as_u64()
        .unwrap();
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert(pane, recorded);
    fixture
        .client
        .replace_harness_before_effect
        .store(true, Ordering::SeqCst);
    let error = fixture
        .manager()
        .drain("worker", DrainOptions::default())
        .map(|result| result.quarantined.len())
        .unwrap();
    assert_eq!(error, 1);
    let documents = failed_documents(&fixture, "worker");
    assert_eq!(documents[0]["probable_misroute"], true);
    assert_eq!(documents[0]["misroutes"].as_array().unwrap().len(), 2);
    assert_eq!(misroutes(&fixture, "worker").len(), 2);
}

#[test]
fn label_and_terminal_drift_refuse_input() {
    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert("tab".to_owned(), "someone-else".to_owned());
    let error = send(&fixture, "worker", "hello").expect_err("label drift");
    assert_eq!(error.outcome(), Some(agent::QueueOutcome::Pending));
    assert!(
        error.to_string().contains("tab label is 'someone-else'"),
        "{error}"
    );

    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .terminals
        .lock()
        .unwrap()
        .insert("owned".to_owned(), "term-restarted".to_owned());
    let error = send(&fixture, "worker", "hello").expect_err("terminal drift");
    assert_eq!(error.outcome(), Some(agent::QueueOutcome::Pending));
    assert!(
        error.to_string().contains("terminal is 'term-restarted'"),
        "{error}"
    );
    assert!(!runs(&fixture).contains(&"hello".to_owned()));
}

#[test]
fn the_expect_capability_is_used_and_a_herdr_refusal_types_nothing() {
    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .expect_supported
        .store(true, Ordering::SeqCst);
    send(&fixture, "worker", "first").unwrap();
    assert_eq!(
        fixture.client.expected_terminals.lock().unwrap().last(),
        Some(&Some("term-owned".to_owned()))
    );
    fixture
        .client
        .swap_terminal_before_effect
        .store(true, Ordering::SeqCst);
    let error = send(&fixture, "worker", "second").expect_err("refused by Herdr");
    assert_eq!(
        error.outcome(),
        Some(agent::QueueOutcome::Pending),
        "{error}"
    );
    assert!(error.to_string().contains("expectation_failed"), "{error}");
    assert!(!runs(&fixture).contains(&"second".to_owned()));
    assert!(failed_documents(&fixture, "worker").is_empty());
}

#[test]
fn without_the_capability_no_expectation_is_sent() {
    let fixture = Fixture::new();
    fixture.start(None);
    send(&fixture, "worker", "first").unwrap();
    assert_eq!(*fixture.client.expected_terminals.lock().unwrap(), [None]);
    assert_eq!(runs(&fixture).last().map(String::as_str), Some("first"));
}

// Legacy records and explicit anchoring.

#[test]
fn an_unanchored_record_refuses_input_until_anchored() {
    let fixture = Fixture::new();
    fixture.start(None);
    strip_anchors(&fixture, "worker");
    let error = send(&fixture, "worker", "queued until anchored").expect_err("unanchored");
    assert_eq!(
        error.outcome(),
        Some(agent::QueueOutcome::Pending),
        "{error}"
    );
    assert!(
        error.to_string().contains("agentctl anchor worker"),
        "{error}"
    );
    let result = fixture.manager().anchor("worker", false).unwrap();
    assert_eq!(result["harness_pid"], 300);
    assert_eq!(result["replaced"], false);
    assert_eq!(result["terminal_id"], "term-owned");
    assert_eq!(
        result["previous"],
        json!({"terminal_id": null, "harness_pid": null})
    );
    fixture
        .manager()
        .drain("worker", DrainOptions::default())
        .unwrap();
    assert_eq!(
        runs(&fixture).last().map(String::as_str),
        Some("queued until anchored")
    );
}

#[test]
fn anchor_refuses_a_changed_harness_without_replace() {
    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert("owned".to_owned(), 999);
    let error = fixture
        .manager()
        .anchor("worker", false)
        .expect_err("changed harness");
    assert!(
        error.to_string().contains("rerun with --replace"),
        "{error}"
    );
    let result = fixture.manager().anchor("worker", true).unwrap();
    assert_eq!(result["replaced"], true);
    assert_eq!(result["harness_pid"], 999);
    send(&fixture, "worker", "after explicit re-anchor").unwrap();
}

#[test]
fn a_duplicate_claim_is_refused_at_anchor() {
    let fixture = Fixture::new();
    fixture.start(None);
    write_claiming_record(&fixture, "other");
    let error = fixture
        .manager()
        .anchor("worker", true)
        .expect_err("duplicate claim");
    assert!(
        error.to_string().contains("already registered as 'other'"),
        "{error}"
    );
}

#[test]
fn anchor_refuses_custom_pane_adapters() {
    let fixture = Fixture::new();
    fixture.start(None);
    let mut document = record(&fixture, "worker");
    document["adapter"] = json!("herdr-relay");
    document["harness_identity"] = Value::Null;
    write_record(&fixture, "worker", &document);
    let error = fixture
        .manager()
        .anchor("worker", false)
        .expect_err("relay");
    assert!(
        error.to_string().contains("custom panes pin their process"),
        "{error}"
    );
}

// Rename.

#[test]
fn rename_moves_registry_name_herdr_name_and_label_together() {
    let fixture = Fixture::new();
    fixture.start(None);
    let result = fixture.manager().rename("worker", "reviewer").unwrap();
    assert_eq!(result["herdr_steps"], "done");
    assert_eq!(result["recovered"], false);
    assert_eq!(result["name"], "reviewer");
    assert_eq!(result["previous_name"], "worker");
    assert_eq!(result["pane_id"], "owned");
    assert!(result["external_references"]
        .as_str()
        .unwrap()
        .contains("'worker'"));
    assert!(!registry(&fixture).join("worker").exists());
    let renamed = record(&fixture, "reviewer");
    assert_eq!(renamed["name"], "reviewer");
    assert_eq!(renamed["pane_id"], "owned");
    assert_eq!(history_names(&renamed), ["worker"]);
    assert!(journal_id_pattern(
        renamed["name_history"][0]["journal_id"].as_str().unwrap()
    ));
    let names = fixture.client.agent_names().unwrap();
    assert_eq!(names.get("reviewer").map(String::as_str), Some("owned"));
    assert!(!names.contains_key("worker"));
    assert_eq!(label(&fixture, "tab"), "reviewer");
    assert_eq!(journal_count(&fixture), 0);
    send(&fixture, "reviewer", "work under the new name").unwrap();
    assert_eq!(
        runs(&fixture).last().map(String::as_str),
        Some("work under the new name")
    );
    let error = send(&fixture, "worker", "nobody").expect_err("old name is gone");
    assert!(error.to_string().contains("unknown agent"), "{error}");
}

#[test]
fn rename_preconditions_refuse_before_any_change() {
    for (prepare, message) in [
        ("registered", "already registered"),
        ("herdr-name", "already named"),
        ("label", "already labelled"),
        ("unanchored", "agentctl anchor worker"),
    ] {
        let fixture = Fixture::new();
        fixture.start(None);
        match prepare {
            "registered" => DirBuilder::new()
                .mode(0o700)
                .create(registry(&fixture).join("reviewer"))
                .unwrap(),
            "herdr-name" => {
                fixture
                    .client
                    .herdr_names
                    .lock()
                    .unwrap()
                    .insert("reviewer".to_owned(), "owned".to_owned());
            }
            "label" => {
                fixture.client.panes.lock().unwrap().push(Pane {
                    pane_id: "spare".to_owned(),
                    tab_id: "spare-tab".to_owned(),
                    workspace_id: "workspace".to_owned(),
                });
                fixture
                    .client
                    .labels
                    .lock()
                    .unwrap()
                    .insert("spare-tab".to_owned(), "reviewer".to_owned());
            }
            _ => strip_anchors(&fixture, "worker"),
        }
        let error = fixture
            .manager()
            .rename("worker", "reviewer")
            .expect_err(prepare);
        assert!(error.to_string().contains(message), "{prepare}: {error}");
        assert!(registry(&fixture).join("worker/agent.json").exists());
        assert_eq!(label(&fixture, "tab"), "worker", "{prepare}");
        assert_eq!(journal_count(&fixture), 0, "{prepare}");
    }
}

#[test]
fn a_crash_after_the_journal_blocks_both_names_and_a_rerun_completes() {
    let fixture = Fixture::new();
    fixture.start(None);
    crash_rename(&fixture);
    for agent_name in ["worker", "reviewer"] {
        let error = send(&fixture, agent_name, "blocked").expect_err(agent_name);
        assert!(
            error
                .to_string()
                .contains("rerun `agentctl rename worker reviewer`"),
            "{agent_name}: {error}"
        );
    }
    let error = fixture
        .manager()
        .start(
            "reviewer",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                ..StartOptions::default()
            },
        )
        .expect_err("start of the reserved name");
    assert!(error.to_string().contains("incomplete"), "{error}");
    assert!(!registry(&fixture).join("reviewer").exists());
    let error = fixture
        .manager()
        .rename("worker", "other")
        .expect_err("a different rename");
    assert!(error.to_string().contains("rerun exactly"), "{error}");
    let result = fixture.manager().rename("worker", "reviewer").unwrap();
    assert_eq!(result["recovered"], true);
    assert_eq!(result["herdr_steps"], "done");
    let renamed = record(&fixture, "reviewer");
    assert_eq!(history_names(&renamed), ["worker"]);
    assert_eq!(renamed["pane_id"], "owned");
    assert_eq!(label(&fixture, "tab"), "reviewer");
    assert_eq!(journal_count(&fixture), 0);
}

#[test]
fn a_crash_between_publication_and_the_directory_move_recovers_with_one_history_entry() {
    let fixture = Fixture::new();
    fixture.start(None);
    FAIL_RENAME_DIRECTORY_MOVE.with(|fail_once| fail_once.set(true));
    let error = fixture
        .manager()
        .rename("worker", "reviewer")
        .expect_err("injected crash");
    assert!(error.to_string().contains("simulated crash"), "{error}");
    // The renamed content was published inside the old directory, which did not move.
    assert_eq!(record(&fixture, "worker")["name"], "reviewer");
    assert_eq!(journal_count(&fixture), 1);
    let result = fixture.manager().rename("worker", "reviewer").unwrap();
    assert_eq!(result["recovered"], true);
    assert_eq!(history_names(&record(&fixture, "reviewer")), ["worker"]);
    assert!(!registry(&fixture).join("worker").exists());
}

#[test]
fn rename_recovery_after_the_harness_exits_finishes_the_registry_only() {
    let fixture = Fixture::new();
    fixture.start(None);
    crash_rename(&fixture);
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert("owned".to_owned(), 999);
    let result = fixture.manager().rename("worker", "reviewer").unwrap();
    assert!(
        result["herdr_steps"]
            .as_str()
            .unwrap()
            .starts_with("skipped-recipient-changed"),
        "{result}"
    );
    assert_eq!(journal_count(&fixture), 0);
    assert!(registry(&fixture).join("reviewer").exists());
    assert!(!registry(&fixture).join("worker").exists());
}

#[test]
fn an_existing_duplicate_claim_refuses_input() {
    let fixture = Fixture::new();
    fixture.start(None);
    write_claiming_record(&fixture, "other");
    let error = send(&fixture, "worker", "must not reach a shared pane").expect_err("claimed");
    assert_eq!(error.outcome(), Some(agent::QueueOutcome::Pending));
    assert!(error.to_string().contains("also claims pane"), "{error}");
}

#[test]
fn an_unreadable_record_refuses_input_conservatively() {
    let fixture = Fixture::new();
    fixture.start(None);
    let broken = registry(&fixture).join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join("agent.json"), "{").unwrap();
    let error = send(&fixture, "worker", "held until readable").expect_err("unreadable");
    assert_eq!(error.outcome(), Some(agent::QueueOutcome::Pending));
    assert!(
        error
            .to_string()
            .contains("cannot prove that pane ownership is unique"),
        "{error}"
    );
}

#[test]
fn rename_refuses_at_the_history_limit() {
    let fixture = Fixture::new();
    fixture.start(None);
    let mut document = record(&fixture, "worker");
    document["name_history"] = Value::Array(
        (0..256)
            .map(|index| json!({"name": "earlier", "renamed_at": 1.0, "journal_id": format!("{index:032x}")}))
            .collect(),
    );
    write_record(&fixture, "worker", &document);
    let error = fixture
        .manager()
        .rename("worker", "reviewer")
        .expect_err("history full");
    assert!(
        error.to_string().contains("the most a record keeps"),
        "{error}"
    );
    assert!(registry(&fixture).join("worker").exists());
}

#[test]
fn rename_recovery_with_the_pane_gone_finishes_the_registry_only() {
    let fixture = Fixture::new();
    fixture.start(None);
    crash_rename(&fixture);
    fixture
        .client
        .panes
        .lock()
        .unwrap()
        .retain(|pane| pane.pane_id != "owned");
    let result = fixture.manager().rename("worker", "reviewer").unwrap();
    assert_eq!(result["herdr_steps"], "skipped-pane-missing");
    assert!(registry(&fixture).join("reviewer").exists());
    assert!(!registry(&fixture).join("worker").exists());
    assert_eq!(journal_count(&fixture), 0);
}

#[test]
fn rename_recovery_refuses_when_both_directories_exist() {
    let fixture = Fixture::new();
    fixture.start(None);
    crash_rename(&fixture);
    DirBuilder::new()
        .mode(0o700)
        .create(registry(&fixture).join("reviewer"))
        .unwrap();
    let error = fixture
        .manager()
        .rename("worker", "reviewer")
        .expect_err("both directories");
    assert!(
        error.to_string().contains("both agent directories"),
        "{error}"
    );
}

#[test]
fn an_adopted_rename_changes_only_the_registry_alias() {
    let fixture = Fixture::new();
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert("tab".to_owned(), "human-chosen".to_owned());
    fixture.adopt();
    let result = fixture.manager().rename("foreign", "renamed").unwrap();
    assert_eq!(result["herdr_steps"], "done");
    assert_eq!(label(&fixture, "tab"), "human-chosen");
    assert_eq!(record(&fixture, "renamed")["adapter"], "herdr-foreign");
    assert!(fixture.client.agent_names().unwrap().is_empty());
}

// Doctor.

#[test]
fn doctor_reports_drift_read_only_and_repairs_only_presentation() {
    let fixture = Fixture::new();
    fixture.start(None);
    let report = fixture.manager().doctor(false).unwrap();
    assert_eq!(report["clean"], true, "{report}");
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert("tab".to_owned(), "retitled-by-hand".to_owned());
    let report = fixture.manager().doctor(false).unwrap();
    assert_eq!(report["clean"], false);
    assert_eq!(findings(&report, "worker"), ["label-mismatch"]);
    assert_eq!(
        label(&fixture, "tab"),
        "retitled-by-hand",
        "read-only by default"
    );
    let repaired = fixture.manager().doctor(true).unwrap();
    assert_eq!(repaired["records"][0]["repaired"], true, "{repaired}");
    assert_eq!(repaired["clean"], true);
    assert_eq!(label(&fixture, "tab"), "worker");

    // Repair is refused while another anchor fails.
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert("tab".to_owned(), "retitled-by-hand".to_owned());
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert("owned".to_owned(), 999);
    let report = fixture.manager().doctor(true).unwrap();
    let mut found = findings(&report, "worker");
    found.sort();
    assert_eq!(found, ["harness-replaced", "label-mismatch"]);
    assert!(report["records"][0].get("repaired").is_none(), "{report}");
    assert_eq!(label(&fixture, "tab"), "retitled-by-hand");
}

#[test]
fn doctor_finds_missing_panes_legacy_records_and_unmanaged_tabs() {
    let fixture = Fixture::new();
    fixture.start(None);
    let mut legacy = record(&fixture, "worker");
    let object = legacy.as_object_mut().unwrap();
    for key in ["terminal_id", "harness_identity", "name_history"] {
        object.remove(key);
    }
    legacy["name"] = json!("legacy");
    legacy["token"] = json!("legacy-generation");
    legacy["pane_id"] = json!("legacy-pane");
    legacy["tab_id"] = json!("legacy-tab");
    legacy["session_agent"] = Value::Null;
    legacy["session_value"] = Value::Null;
    write_record(&fixture, "legacy", &legacy);
    {
        let mut panes = fixture.client.panes.lock().unwrap();
        panes.retain(|pane| pane.pane_id != "owned");
        for (pane, tab) in [("legacy-pane", "legacy-tab"), ("hand-pane", "hand-tab")] {
            panes.push(Pane {
                pane_id: pane.to_owned(),
                tab_id: tab.to_owned(),
                workspace_id: "workspace".to_owned(),
            });
        }
    }
    {
        let mut labels = fixture.client.labels.lock().unwrap();
        labels.insert("legacy-tab".to_owned(), "legacy".to_owned());
        labels.insert("hand-tab".to_owned(), "hand-made".to_owned());
    }
    fixture
        .client
        .herdr_names
        .lock()
        .unwrap()
        .insert("legacy".to_owned(), "legacy-pane".to_owned());
    let report = fixture.manager().doctor(false).unwrap();
    assert_eq!(findings(&report, "worker"), ["pane-missing"], "{report}");
    assert_eq!(findings(&report, "legacy"), ["unanchored"], "{report}");
    assert!(report["workspace"].as_array().unwrap().contains(
        &json!({"finding": "unmanaged-tab", "tab_id": "hand-tab", "label": "hand-made"})
    ));
    assert_eq!(report["clean"], false);
}

#[test]
fn doctor_reports_duplicate_claims() {
    let fixture = Fixture::new();
    fixture.start(None);
    write_claiming_record(&fixture, "other");
    let report = fixture.manager().doctor(false).unwrap();
    assert!(findings(&report, "worker").contains(&"duplicate-claim".to_owned()));
    assert!(findings(&report, "other").contains(&"duplicate-claim".to_owned()));
}

#[test]
fn doctor_reports_an_incomplete_rename() {
    let fixture = Fixture::new();
    fixture.start(None);
    crash_rename(&fixture);
    let report = fixture.manager().doctor(false).unwrap();
    assert_eq!(
        report["journals"],
        json!([{"old": "worker", "new": "reviewer"}])
    );
    assert!(findings(&report, "worker").contains(&"rename-incomplete".to_owned()));
    assert_eq!(report["clean"], false);
}

// Shared on-disk formats.

#[test]
fn record_identity_fields_validate_like_the_python_edition() {
    let fixture = Fixture::new();
    fixture.start(None);
    let base = record(&fixture, "worker");
    let mut python = base.clone();
    python["name_history"] = json!([{
        "name": "older",
        "renamed_at": 1_760_000_000.5,
        "journal_id": "0123456789abcdef0123456789abcdef",
    }]);
    write_record(&fixture, "worker", &python);
    let loaded = fixture.manager().get("worker").unwrap();
    assert_eq!(loaded["name_history"], python["name_history"]);
    let entry = |name: Value, renamed_at: Value, journal_id: Value| json!({"name": name, "renamed_at": renamed_at, "journal_id": journal_id});
    let good_id = json!("0123456789abcdef0123456789abcdef");
    let invalid: Vec<(&str, Value)> = vec![
        ("name_history", Value::Null),
        ("name_history", json!({})),
        (
            "name_history",
            json!([entry(
                json!("older"),
                json!(1.0),
                json!("0123456789ABCDEF0123456789ABCDEF")
            )]),
        ),
        (
            "name_history",
            json!([
                entry(json!("older"), json!(1.0), good_id.clone()),
                entry(json!("oldest"), json!(2.0), good_id.clone())
            ]),
        ),
        (
            "name_history",
            json!([entry(json!("Older"), json!(1.0), good_id.clone())]),
        ),
        (
            "name_history",
            json!([entry(json!("older"), json!("soon"), good_id.clone())]),
        ),
        (
            "name_history",
            json!([{"name": "older", "renamed_at": 1.0, "journal_id": good_id, "extra": 1}]),
        ),
        ("terminal_id", json!("")),
        ("terminal_id", json!("t".repeat(129))),
        ("terminal_id", json!(7)),
        ("harness_identity", json!({"pid": 1})),
    ];
    for (key, value) in invalid {
        let mut document = base.clone();
        document[key] = value.clone();
        write_record(&fixture, "worker", &document);
        let error = fixture
            .manager()
            .get("worker")
            .expect_err(&format!("{key} = {value}"));
        assert!(error.to_string().contains("invalid"), "{key}: {error}");
    }
}

#[test]
fn rename_journals_validate_like_the_python_edition() {
    let path = Path::new("/registry/.renames/123-4.json");
    let valid = json!({
        "schema": "agentctl-rename/v1",
        "token": "123-4",
        "old": "worker",
        "new": "reviewer",
        "adapter": "herdr",
        "pane_id": "w1:p1",
        "tab_id": "w1:t1",
        "terminal_id": null,
        "workspace_id": "w1",
        "journal_id": "0123456789abcdef0123456789abcdef",
        "started_at": 1_760_000_000.25,
    });
    let journal = RenameJournal::parse(&valid, path).unwrap();
    assert_eq!(journal.document(), valid);
    let mutations: Vec<(&str, Option<Value>)> = vec![
        ("extra", Some(json!(1))),
        ("terminal_id", None),
        ("schema", Some(json!("agentctl-rename/v2"))),
        ("token", Some(json!("other-token"))),
        ("new", Some(json!("worker"))),
        ("old", Some(json!("Worker"))),
        ("adapter", Some(json!("herdr-pane"))),
        ("pane_id", Some(json!(""))),
        ("tab_id", Some(Value::Null)),
        ("terminal_id", Some(json!(5))),
        ("workspace_id", Some(json!(false))),
        ("journal_id", Some(json!("0123"))),
        ("started_at", Some(json!(true))),
        ("started_at", Some(json!("now"))),
    ];
    for (key, value) in mutations {
        let mut document = valid.clone();
        match value {
            Some(value) => document[key] = value,
            None => {
                document.as_object_mut().unwrap().remove(key);
            }
        }
        assert!(
            RenameJournal::parse(&document, path).is_err(),
            "{key} mutation accepted"
        );
    }
}

#[test]
fn the_countermand_stops_when_the_occupant_changes_after_the_interrupt() {
    let fixture = Fixture::new();
    fixture.start(None);
    fixture
        .client
        .replace_harness_before_effect
        .store(true, Ordering::SeqCst);
    fixture
        .client
        .replace_harness_after_esc
        .store(true, Ordering::SeqCst);
    let error =
        send(&fixture, "worker", "work meant for the worker agent").expect_err("quarantined");
    assert_eq!(
        error.outcome(),
        Some(agent::QueueOutcome::PossiblySubmitted),
        "{error}"
    );
    assert!(error.to_string().contains("note not sent"), "{error}");
    assert!(!runs(&fixture).contains(&MISROUTE_NOTE.to_owned()));
    let entries = misroutes(&fixture, "worker");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["interrupted"], true);
    assert_eq!(entries[0]["note_sent"], false);
    assert_eq!(
        failed_documents(&fixture, "worker")[0]["probable_misroute"],
        true
    );
}
