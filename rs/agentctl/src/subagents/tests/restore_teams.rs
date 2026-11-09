//! `agentctl restore-teams` against two fake-Herdr registries and a scripted host.

use super::*;
use crate::restore::{CommandOutcome, Host};
use crate::restore_teams::{
    resolve_entries, restore_one_with_client, restore_teams, RegistryEntry, RestoreTeamsOptions,
    TeamsConfig,
};
use std::cell::{Cell, RefCell};

const BOOT: &str = "11111111-2222-3333-4444-555555555555";

/// A host whose clock only moves when it sleeps; every command succeeds.
struct ScriptedHost {
    now: Cell<f64>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl ScriptedHost {
    fn new() -> Self {
        Self {
            now: Cell::new(2_000_000_000.0),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Host for ScriptedHost {
    fn boot_id(&self) -> crate::subagents::Result<String> {
        Ok(BOOT.to_owned())
    }
    fn boot_time(&self) -> Option<u64> {
        Some(2_000_000_000 - 3600)
    }
    fn now(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, duration: Duration) {
        self.now.set(self.now.get() + duration.as_secs_f64());
    }
    fn run(&self, argv: &[String], _cwd: Option<&Path>, _timeout: Duration) -> CommandOutcome {
        self.calls.borrow_mut().push(argv.to_vec());
        CommandOutcome {
            status: Some(0),
            ..CommandOutcome::default()
        }
    }
}

fn registry_of(fixture: &Fixture) -> PathBuf {
    fixture.root.join("registry")
}

fn write_config(fixture: &Fixture, extra: Value) {
    let registry = registry_of(fixture);
    fs::create_dir_all(&registry).unwrap();
    let mut document = json!({
        "schema": "agentctl-restore/v1",
        "startup_delay_seconds": 0,
        "report_dir": fixture.root.join("reports"),
    });
    for (key, value) in extra.as_object().unwrap() {
        document[key] = value.clone();
    }
    fs::write(registry.join("restore.json"), document.to_string()).unwrap();
}

/// One agent the reboot killed: a dead harness whose pane is an idle shell.
fn dead_agent(fixture: &Fixture, name: &str, harness: &str) -> AgentRecord {
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            name,
            &fixture.root,
            StartOptions {
                harness: harness.to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load(name).unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(record.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    record
}

fn entry(registry: PathBuf) -> RegistryEntry {
    RegistryEntry {
        registry,
        herdr_bin: None,
    }
}

fn teams_options() -> RestoreTeamsOptions {
    RestoreTeamsOptions {
        boot: false,
        force: false,
        dry_run: false,
        startup_timeout: Duration::from_secs(30),
        cert_wait: Duration::ZERO,
        herdr_bin: PathBuf::from("herdr"),
    }
}

/// The runner the CLI builds, over the two fixtures' fake clients.
macro_rules! runner_for {
    ($host:expr, $a:expr, $b:expr) => {
        |entry: &RegistryEntry,
         config: &mut crate::restore::RestoreConfig,
         options: &crate::restore::RestoreOptions| {
            let fixture = if entry.registry == registry_of($a) {
                $a
            } else {
                $b
            };
            restore_one_with_client(
                &fixture.client,
                $host,
                &entry.registry,
                config,
                options,
                false,
            )
        }
    };
}

#[test]
fn two_registries_restore_in_one_run_and_a_rerun_replays_them() {
    let first = Fixture::new();
    let second = Fixture::new();
    dead_agent(&first, "lane", "codex");
    dead_agent(&second, "other", "codex");
    write_config(&first, json!({}));
    write_config(&second, json!({}));
    let host = ScriptedHost::new();
    let entries = [entry(registry_of(&first)), entry(registry_of(&second))];

    let outcome = restore_teams(
        &host,
        &entries,
        &teams_options(),
        runner_for!(&host, &first, &second),
    )
    .unwrap();
    assert_eq!(outcome.exit_code, 0, "{}", outcome.report);
    assert_eq!(outcome.report["schema"], "agentctl-restore-teams-report/v1");
    assert_eq!(outcome.report["complete"], true);
    assert_eq!(outcome.report["refused"], 0);
    let rows = outcome.report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{}", outcome.report);
    assert_eq!(rows[0]["name"], "lane");
    assert_eq!(rows[0]["action"], "revived");
    assert_eq!(rows[0]["registry"], json!(registry_of(&first)));
    assert_eq!(rows[1]["name"], "other");
    assert_eq!(rows[1]["action"], "revived");
    assert_eq!(rows[1]["registry"], json!(registry_of(&second)));
    assert_eq!(outcome.table.len(), 2);
    assert!(
        outcome.table[0].contains("lane revived"),
        "{}",
        outcome.table[0]
    );
    let first_pane = first.manager().status("lane").unwrap()["pane_id"].clone();
    let second_pane = second.manager().status("other").unwrap()["pane_id"].clone();

    // An immediate rerun in the same boot replays the recorded
    // per-agent outcomes, labelled, at the same exit class, and
    // launches nothing again.
    let replay = restore_teams(
        &host,
        &entries,
        &teams_options(),
        runner_for!(&host, &first, &second),
    )
    .unwrap();
    assert_eq!(replay.exit_code, 0, "{}", replay.report);
    let rows = replay.report["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{}", replay.report);
    assert!(
        rows.iter().all(|row| row["replayed"] == true),
        "{}",
        replay.report
    );
    assert!(rows.iter().all(|row| row["action"] == "revived"));
    assert!(
        replay.table.iter().all(|line| line.ends_with("(replayed)")),
        "{:?}",
        replay.table
    );
    assert_eq!(
        first.manager().status("lane").unwrap()["pane_id"],
        first_pane
    );
    assert_eq!(
        second.manager().status("other").unwrap()["pane_id"],
        second_pane
    );
}

#[test]
fn a_missing_certificate_gates_only_its_registry_at_75() {
    let gated = Fixture::new();
    let open = Fixture::new();
    let dead = dead_agent(&gated, "lane", "codex");
    dead_agent(&open, "other", "codex");
    write_config(
        &gated,
        json!({
            "certificate_file": gated.root.join("missing.pem"),
            "certificate_fetch": "example-fetch-command",
        }),
    );
    write_config(&open, json!({}));
    let host = ScriptedHost::new();
    let entries = [entry(registry_of(&gated)), entry(registry_of(&open))];
    let calls: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
    let outcome = restore_teams(
        &host,
        &entries,
        &teams_options(),
        |entry, config, options| {
            calls.borrow_mut().push(entry.registry.clone());
            let fixture = if entry.registry == registry_of(&gated) {
                &gated
            } else {
                &open
            };
            restore_one_with_client(
                &fixture.client,
                &host,
                &entry.registry,
                config,
                options,
                false,
            )
        },
    )
    .unwrap();
    assert_eq!(outcome.exit_code, 75, "{}", outcome.report);
    // Nothing was launched for the gated registry: its restore never ran.
    assert_eq!(*calls.borrow(), [registry_of(&open)]);
    assert_eq!(
        gated.manager().load("lane").unwrap().pane_id,
        dead.pane_id,
        "the gated lane's record must be untouched"
    );
    assert!(!gated.root.join("reports").exists());
    let first = &outcome.report["registries"][0];
    assert_eq!(first["status"], "certificate-timeout");
    assert_eq!(first["exit_code"], 75);
    assert_eq!(first["certificate"]["remedy"], "example-fetch-command");
    assert_eq!(outcome.report["registries"][1]["status"], "complete");
    assert!(
        outcome
            .table
            .iter()
            .any(|line| line.contains("certificate-timeout")
                && line.contains("example-fetch-command"))
    );
}

#[test]
fn one_refused_lane_finishes_the_rest_at_74_and_replays_at_74() {
    let first = Fixture::new();
    let second = Fixture::new();
    dead_agent(&first, "lane", "codex");
    // A dead Muse lane with no recorded session and two candidates in
    // the store for its cwd refuses per-lane (the restore.rs fixture).
    second.client.report_session.store(false, Ordering::Relaxed);
    let store = second.root.join("data/muse");
    crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = Some(store.clone()));
    for id in [
        "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
        "cccccccc-cccc-cccc-cccc-cccccccccccc",
    ] {
        let dir = store.join("sessions/2026/11/06").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.jsonl"),
            format!(
                "{{\"cwd\":\"{}\",\"text\":\"fix the flaky shard\"}}\n",
                second.root.display()
            ),
        )
        .unwrap();
    }
    dead_agent(&second, "stuck", "muse");
    write_config(&first, json!({}));
    write_config(&second, json!({}));
    let host = ScriptedHost::new();
    let entries = [entry(registry_of(&first)), entry(registry_of(&second))];

    let outcome = restore_teams(
        &host,
        &entries,
        &teams_options(),
        runner_for!(&host, &first, &second),
    )
    .unwrap();
    crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = None);
    assert_eq!(outcome.exit_code, 74, "{}", outcome.report);
    assert_eq!(outcome.report["complete"], true);
    assert_eq!(outcome.report["refused"], 1);
    assert_eq!(
        outcome.report["refused_lanes"],
        json!([{"registry": registry_of(&second), "name": "stuck"}])
    );
    let rows = outcome.report["rows"].as_array().unwrap();
    assert_eq!(rows[0]["action"], "revived", "{}", outcome.report);
    let refused = rows.iter().find(|row| row["name"] == "stuck").unwrap();
    assert_eq!(refused["action"], "refused");
    assert!(
        outcome
            .table
            .iter()
            .any(|line| line.contains("stuck refused")),
        "{:?}",
        outcome.table
    );

    // The rerun keeps the recorded 74 class with replayed rows.
    let replay = restore_teams(
        &host,
        &entries,
        &teams_options(),
        runner_for!(&host, &first, &second),
    )
    .unwrap();
    assert_eq!(replay.exit_code, 74, "{}", replay.report);
    let rows = replay.report["rows"].as_array().unwrap();
    let refused = rows.iter().find(|row| row["name"] == "stuck").unwrap();
    assert_eq!(refused["action"], "refused");
    assert_eq!(refused["replayed"], true);
}

#[test]
fn a_single_registry_filter_touches_only_that_registry() {
    let first = Fixture::new();
    let second = Fixture::new();
    dead_agent(&first, "lane", "codex");
    let untouched = dead_agent(&second, "other", "codex");
    write_config(&first, json!({}));
    write_config(&second, json!({}));
    // Both registries are configured; the filter limits the run to one.
    let config = TeamsConfig {
        entries: vec![entry(registry_of(&first)), entry(registry_of(&second))],
    };
    let entries = resolve_entries(Some(&config), &[registry_of(&first)]).unwrap();
    assert_eq!(entries.len(), 1);
    let record_path = registry_of(&second).join("agents/other/agent.json");
    let record_before = fs::read(&record_path).unwrap();
    let runs_before = second.client.runs.lock().unwrap().len();

    let host = ScriptedHost::new();
    let outcome = restore_teams(
        &host,
        &entries,
        &teams_options(),
        runner_for!(&host, &first, &second),
    )
    .unwrap();
    assert_eq!(outcome.exit_code, 0, "{}", outcome.report);
    assert_eq!(outcome.report["registries"].as_array().unwrap().len(), 1);
    assert_eq!(
        outcome.report["rows"][0]["registry"],
        json!(registry_of(&first))
    );
    // The other registry is untouched: same record bytes, no pane
    // traffic, no report written, the dead lane still on its old pane.
    assert_eq!(fs::read(&record_path).unwrap(), record_before);
    assert_eq!(second.client.runs.lock().unwrap().len(), runs_before);
    assert!(!second.root.join("reports").exists());
    assert_eq!(
        second.manager().load("other").unwrap().pane_id,
        untouched.pane_id
    );
    assert!(!registry_of(&second).join(".restore.lock").exists());
}
