//! `agentctl restore` and `agentctl watch` against the fake Herdr and a scripted host.

use super::*;
use crate::restore::{
    restore, watch_once, CommandOutcome, Host, RestoreConfig, RestoreOptions, TimerWatch,
    WatchOptions,
};
use std::cell::{Cell, RefCell};

const BOOT: &str = "11111111-2222-3333-4444-555555555555";

/// A host whose commands answer from a script and whose clock only moves when it sleeps.
struct ScriptedHost {
    boot: String,
    now: Cell<f64>,
    boot_time: u64,
    calls: RefCell<Vec<Vec<String>>>,
    answers: RefCell<Vec<(String, CommandOutcome)>>,
}

impl ScriptedHost {
    fn new() -> Self {
        Self {
            boot: BOOT.to_owned(),
            now: Cell::new(2_000_000_000.0),
            boot_time: 2_000_000_000 - 3600,
            calls: RefCell::new(Vec::new()),
            answers: RefCell::new(Vec::new()),
        }
    }

    /// The next command whose joined argv contains `needle` gets `outcome` (first match wins,
    /// once); anything unscripted succeeds silently.
    fn answer(&self, needle: &str, status: i32, stdout: &str, stderr: &str) {
        self.answers.borrow_mut().push((
            needle.to_owned(),
            CommandOutcome {
                status: Some(status),
                stdout: stdout.to_owned(),
                stderr: stderr.to_owned(),
            },
        ));
    }

    fn commands(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .map(|argv| argv.join(" "))
            .collect()
    }
}

impl Host for ScriptedHost {
    fn boot_id(&self) -> crate::subagents::Result<String> {
        Ok(self.boot.clone())
    }
    fn boot_time(&self) -> Option<u64> {
        Some(self.boot_time)
    }
    fn now(&self) -> f64 {
        self.now.get()
    }
    fn sleep(&self, duration: Duration) {
        self.now.set(self.now.get() + duration.as_secs_f64());
    }
    fn run(&self, argv: &[String], _cwd: Option<&Path>, _timeout: Duration) -> CommandOutcome {
        self.calls.borrow_mut().push(argv.to_vec());
        let joined = argv.join(" ");
        let mut answers = self.answers.borrow_mut();
        match answers
            .iter()
            .position(|(needle, _)| joined.contains(needle.as_str()))
        {
            Some(index) => answers.remove(index).1,
            None => CommandOutcome {
                status: Some(0),
                ..CommandOutcome::default()
            },
        }
    }
}

fn config(fixture: &Fixture, coordinator: Option<&str>) -> RestoreConfig {
    let mut document = json!({
        "schema": "agentctl-restore/v1",
        "units": ["bridge.service"],
        "startup_delay_seconds": 7,
        "report_dir": fixture.root.join("reports"),
    });
    if let Some(name) = coordinator {
        document["coordinator"] = json!(name);
    }
    let registry = fixture.root.join("registry");
    fs::create_dir_all(&registry).unwrap();
    fs::write(registry.join("restore.json"), document.to_string()).unwrap();
    RestoreConfig::load(&registry).unwrap()
}

fn options(wrkslots: Option<&str>) -> RestoreOptions {
    RestoreOptions {
        startup_timeout: Duration::from_secs(30),
        wrkslots: wrkslots.map(PathBuf::from),
        ..RestoreOptions::default()
    }
}

/// One agent the reboot killed: a dead harness whose pane is an idle shell.
fn dead_agent(fixture: &Fixture) -> AgentRecord {
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            "lane",
            &fixture.root,
            StartOptions {
                harness: "codex".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load("lane").unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(record.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    record
}

#[test]
fn restore_remounts_first_revives_then_starts_units_and_is_idempotent() {
    let fixture = Fixture::new();
    let old = dead_agent(&fixture);
    let host = ScriptedHost::new();
    host.answer(
        "image status",
        0,
        "",
        "wrkslots: mounted lane.img at /slots/lane\n",
    );
    let config = config(&fixture, None);
    let registry = fixture.root.join("registry");
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config,
        &options(Some("/opt/wrkslots")),
    )
    .unwrap();
    assert_eq!(report["boot_id"], BOOT);
    assert_eq!(report["herdr"]["ready"], true);
    assert_eq!(report["slot_images"]["action"], "ran");
    assert_eq!(
        report["slot_images"]["mounted"],
        json!(["lane.img at /slots/lane"])
    );
    assert_eq!(report["agents"][0]["name"], "lane");
    assert_eq!(report["agents"][0]["action"], "revived", "{report}");
    assert_ne!(report["agents"][0]["pane_id"], json!(old.pane_id));
    assert_eq!(report["units"][0]["action"], "started");
    assert_eq!(report["complete"], true);
    // The slot manager runs before any agent, the units after every agent.
    assert_eq!(
        host.commands(),
        [
            "/opt/wrkslots image status",
            "systemctl --user reset-failed bridge.service",
            "systemctl --user start bridge.service",
        ]
    );
    // The configured delay follows each launch.
    assert_eq!(host.now(), 2_000_000_007.0);
    let path = fixture.root.join("reports").join(format!("{BOOT}.json"));
    assert_eq!(
        crate::restore::read_report(&path).unwrap()["complete"],
        true
    );

    let again = restore(&fixture.client, &registry, &host, &config, &options(None)).unwrap();
    assert_eq!(again["action"], "already-restored");
    assert_eq!(
        host.commands().len(),
        3,
        "a completed boot runs nothing again"
    );

    let forced = restore(
        &fixture.client,
        &registry,
        &host,
        &config,
        &RestoreOptions {
            force: true,
            ..options(None)
        },
    )
    .unwrap();
    assert_eq!(forced["agents"][0]["action"], "skipped", "{forced}");
}

#[test]
fn restore_dry_run_changes_nothing_and_reads_slot_warnings() {
    let fixture = Fixture::new();
    dead_agent(&fixture);
    let host = ScriptedHost::new();
    let config = config(&fixture, None);
    let registry = fixture.root.join("registry");
    let before = fs::read(registry.join("lane/agent.json")).unwrap();
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config,
        &RestoreOptions {
            dry_run: true,
            ..options(Some("/opt/wrkslots"))
        },
    )
    .unwrap();
    assert_eq!(report["agents"][0]["action"], "revive");
    assert_eq!(report["complete"], false);
    assert_eq!(report["would_complete"], true);
    assert!(host.commands().is_empty());
    assert_eq!(fs::read(registry.join("lane/agent.json")).unwrap(), before);
    assert!(!fixture.root.join("reports").exists());

    host.answer(
        "image status",
        0,
        "",
        "wrkslots: WARNING: slot image not mounted: lane.img: device busy\n",
    );
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config,
        &options(Some("/opt/wrkslots")),
    )
    .unwrap();
    assert_eq!(report["slot_images"]["action"], "failed");
    assert_eq!(
        report["slot_images"]["warnings"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn restore_refuses_an_unregistered_coordinator_and_stays_incomplete() {
    let fixture = Fixture::new();
    dead_agent(&fixture);
    let host = ScriptedHost::new();
    let config = config(&fixture, Some("coordinator"));
    let registry = fixture.root.join("registry");
    let report = restore(&fixture.client, &registry, &host, &config, &options(None)).unwrap();
    assert_eq!(report["coordinator"]["action"], "refused");
    assert_eq!(report["complete"], false);
    assert_eq!(report["notice"], Value::Null);
    assert_eq!(report["agents"][0]["action"], "revived");
}

#[test]
fn restore_configuration_is_strict() {
    let fixture = Fixture::new();
    let registry = fixture.root.join("registry");
    fs::create_dir_all(&registry).unwrap();
    for document in [
        json!({"schema": "agentctl-restore/v2"}),
        json!({"schema": "agentctl-restore/v1", "units": ["--now"]}),
        json!({"schema": "agentctl-restore/v1", "units": ["a b.service"]}),
        json!({"schema": "agentctl-restore/v1", "also_registered": [{"registry": "relative", "name": "x"}]}),
        json!({"schema": "agentctl-restore/v1", "timers": [{"unit": "tick.service", "max_age_seconds": 60}]}),
        json!({"schema": "agentctl-restore/v1", "unknown": true}),
    ] {
        fs::write(registry.join("restore.json"), document.to_string()).unwrap();
        assert!(RestoreConfig::load(&registry).is_err(), "{document}");
    }
    fs::remove_file(registry.join("restore.json")).unwrap();
    assert_eq!(
        RestoreConfig::load(&registry)
            .unwrap()
            .startup_delay_seconds,
        20
    );
}

#[test]
fn watch_restarts_a_timer_that_never_fires_and_starts_its_service_when_needed() {
    let fixture = Fixture::new();
    let registry = fixture.root.join("registry");
    let host = ScriptedHost::new();
    let never = "ActiveState=active\nNextElapseUSecRealtime=\nNextElapseUSecMonotonic=infinity\nLastTriggerUSec=@1990000000\n";
    let fresh = "ActiveState=active\nNextElapseUSecRealtime=\nNextElapseUSecMonotonic=1d 2h\nLastTriggerUSec=@1999999990\n";
    host.answer(
        "show observer.timer --timestamp=unix -p ActiveState",
        0,
        never,
        "",
    );
    host.answer(
        "show observer.timer --timestamp=unix -p ActiveState",
        0,
        never,
        "",
    );
    host.answer(
        "show observer.timer --timestamp=unix -p Triggers",
        0,
        "Triggers=observer.service\n",
        "",
    );
    host.answer(
        "show observer.timer --timestamp=unix -p ActiveState",
        0,
        fresh,
        "",
    );
    host.answer(
        "show tick.timer --timestamp=unix -p ActiveState",
        0,
        fresh,
        "",
    );
    let config = RestoreConfig {
        timers: vec![
            TimerWatch {
                unit: "observer.timer".to_owned(),
                max_age_seconds: 1800,
            },
            TimerWatch {
                unit: "tick.timer".to_owned(),
                max_age_seconds: 1800,
            },
        ],
        ..config(&fixture, None)
    };
    let findings = watch_once(
        &fixture.client,
        &registry,
        &host,
        &RestoreConfig {
            units: Vec::new(),
            ..config
        },
        &WatchOptions {
            interval: Duration::from_secs(60),
            once: true,
            check_only: false,
            wrkslots: None,
            state_dir: Some(fixture.root.join("watch")),
        },
    );
    let timers = findings
        .iter()
        .filter(|row| row["check"] == "timer")
        .collect::<Vec<_>>();
    assert_eq!(timers.len(), 2);
    assert_eq!(timers[0]["ok"], true, "{findings:?}");
    assert_eq!(
        timers[0]["repair"]["actions"],
        json!([
            "systemctl --user restart observer.timer",
            "systemctl --user start observer.service"
        ])
    );
    assert_eq!(timers[1]["ok"], true);
    assert_eq!(timers[1]["repair"], Value::Null);
    assert!(findings
        .iter()
        .any(|row| row["check"] == "herdr" && row["ok"] == true));
}

#[test]
fn watch_reports_a_failed_unit_it_cannot_start() {
    let fixture = Fixture::new();
    let registry = fixture.root.join("registry");
    let host = ScriptedHost::new();
    host.answer("show bridge.service", 0, "ActiveState=failed\n", "");
    host.answer(
        "start bridge.service",
        1,
        "",
        "Job failed: start-limit-hit\n",
    );
    host.answer("show bridge.service", 0, "ActiveState=failed\n", "");
    let findings = watch_once(
        &fixture.client,
        &registry,
        &host,
        &config(&fixture, None),
        &WatchOptions {
            interval: Duration::from_secs(60),
            once: true,
            check_only: false,
            wrkslots: None,
            state_dir: None,
        },
    );
    let unit = findings.iter().find(|row| row["check"] == "unit").unwrap();
    assert_eq!(unit["ok"], false);
    assert!(unit["detail"].as_str().unwrap().contains("start-limit-hit"));
    assert!(host
        .commands()
        .contains(&"systemctl --user reset-failed bridge.service".to_owned()));
}

#[test]
fn restore_revives_the_coordinator_first_re_registers_it_and_queues_one_notice() {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            "coordinator",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let coordinator = fixture.manager().load("coordinator").unwrap();
    // A chat bridge keeps its own registry, where the coordinator is an adopted record.
    let bridge = fixture.root.join("bridge-registry");
    let label = fixture
        .client
        .workspace_label(coordinator.workspace_id.as_deref().unwrap())
        .unwrap();
    ManagedAgents::new(&fixture.client, &bridge)
        .unwrap()
        .adopt(
            "bridge-coordinator",
            AdoptOptions {
                pane_id: coordinator.pane_id.clone().unwrap(),
                expected_workspace: label,
                cwd: fixture.root.clone(),
                harness: "claude".to_owned(),
                session: None,
            },
        )
        .unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(coordinator.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    let host = ScriptedHost::new();
    let registry = fixture.root.join("registry");
    let config = RestoreConfig {
        also_registered: vec![crate::restore::AlsoRegistered {
            registry: bridge.clone(),
            name: "bridge-coordinator".to_owned(),
        }],
        ..config(&fixture, Some("coordinator"))
    };
    let report = restore(&fixture.client, &registry, &host, &config, &options(None)).unwrap();
    assert_eq!(report["coordinator"]["action"], "revived", "{report}");
    let pane = report["coordinator"]["pane_id"].clone();
    assert_ne!(pane, json!(coordinator.pane_id));
    assert_eq!(
        report["also_registered"][0]["action"], "adopted",
        "{report}"
    );
    let bridged = ManagedAgents::new(&fixture.client, &bridge)
        .unwrap()
        .load("bridge-coordinator")
        .unwrap();
    assert_eq!(json!(bridged.pane_id), pane);
    assert_eq!(report["complete"], true);
    assert_eq!(report["notice"]["message_id"], "restore-11111111");
    let text = fixture.client.runs.lock().unwrap().join("\n");
    assert!(
        text.contains(
            "[agentctl restore] The host restarted: boot 11111111-2222-3333-4444-555555555555"
        ),
        "{text}"
    );
    assert!(text.contains("Revived: coordinator ->"), "{text}");
    assert!(
        text.contains("Session crons and background shells did not survive"),
        "{text}"
    );
    assert_eq!(report["restart_detected"], true);
}

#[test]
fn watch_tells_a_live_coordinator_once_an_hour_about_what_it_could_not_repair() {
    let fixture = Fixture::new();
    fixture
        .manager()
        .start(
            "coordinator",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let registry = fixture.root.join("registry");
    let host = ScriptedHost::new();
    for _ in 0..2 {
        host.answer("show bridge.service", 0, "ActiveState=failed\n", "");
        host.answer("start bridge.service", 1, "", "Job failed\n");
        host.answer("show bridge.service", 0, "ActiveState=failed\n", "");
    }
    let config = config(&fixture, Some("coordinator"));
    let options = WatchOptions {
        interval: Duration::from_secs(60),
        once: true,
        check_only: false,
        wrkslots: None,
        state_dir: Some(fixture.root.join("watch")),
    };
    let first =
        crate::restore::watch(&fixture.client, &registry, &host, &config, &options).unwrap();
    assert_eq!(first["ok"], false);
    assert_eq!(first["notice"]["sent"], true, "{first}");
    let sent = fixture.client.runs.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(
        sent[0].contains("[agentctl watch]") && sent[0].contains("unit bridge.service"),
        "{sent:?}"
    );
    assert!(fixture.root.join("watch/health.json").is_file());
    host.sleep(Duration::from_secs(600));
    let second =
        crate::restore::watch(&fixture.client, &registry, &host, &config, &options).unwrap();
    assert_eq!(second["notice"]["sent"], false);
    assert_eq!(
        fixture.client.runs.lock().unwrap().len(),
        1,
        "no repeat within the hour"
    );
}

#[test]
fn watch_reports_a_timer_whose_service_fails_on_every_run_without_restarting_it() {
    let fixture = Fixture::new();
    let registry = fixture.root.join("registry");
    let host = ScriptedHost::new();
    let fresh = "ActiveState=active\nNextElapseUSecRealtime=\nNextElapseUSecMonotonic=1d 2h\nLastTriggerUSec=@1999999990\n";
    host.answer(
        "show observer.timer --timestamp=unix -p ActiveState",
        0,
        fresh,
        "",
    );
    host.answer(
        "show observer.timer --timestamp=unix -p Triggers",
        0,
        "Triggers=observer.service\n",
        "",
    );
    host.answer(
        "show observer.service",
        0,
        "ActiveState=failed\nResult=exit-code\nExecMainStatus=1\n",
        "",
    );
    let config = RestoreConfig {
        units: Vec::new(),
        timers: vec![TimerWatch {
            unit: "observer.timer".to_owned(),
            max_age_seconds: 1800,
        }],
        ..config(&fixture, None)
    };
    let findings = watch_once(
        &fixture.client,
        &registry,
        &host,
        &config,
        &WatchOptions {
            interval: Duration::from_secs(60),
            once: true,
            check_only: false,
            wrkslots: None,
            state_dir: None,
        },
    );
    let timer = findings.iter().find(|row| row["check"] == "timer").unwrap();
    assert_eq!(timer["ok"], false);
    assert_eq!(
        timer["detail"],
        "observer.service failed on its last run (Result=exit-code, exit status 1)"
    );
    assert!(
        !host
            .commands()
            .iter()
            .any(|command| command.contains(" restart ") || command.contains(" start ")),
        "{:?}",
        host.commands()
    );
}

#[test]
fn restore_leaves_excluded_records_alone_and_never_excludes_the_coordinator() {
    let fixture = Fixture::new();
    let old = dead_agent(&fixture);
    let host = ScriptedHost::new();
    let registry = fixture.root.join("registry");
    let config = RestoreConfig {
        exclude: vec!["lane".to_owned()],
        ..config(&fixture, None)
    };
    let report = restore(&fixture.client, &registry, &host, &config, &options(None)).unwrap();
    assert_eq!(report["agents"][0]["action"], "excluded");
    assert_eq!(report["refused"], 0);
    assert_eq!(report["complete"], true);
    assert_eq!(fixture.manager().load("lane").unwrap().token, old.token);
    fs::write(
        registry.join("restore.json"),
        json!({"schema": "agentctl-restore/v1", "coordinator": "lead", "exclude": ["lead"]})
            .to_string(),
    )
    .unwrap();
    assert!(RestoreConfig::load(&registry).is_err());
}

#[test]
fn a_restore_on_the_boot_its_agents_were_pinned_on_says_no_restart_and_keeps_crons() {
    let fixture = Fixture::new();
    fixture
        .manager()
        .start(
            "coordinator",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    // The fake pins every harness under this boot id.
    let host = ScriptedHost {
        boot: "22222222-3333-4444-5555-666666666666".to_owned(),
        ..ScriptedHost::new()
    };
    let registry = fixture.root.join("registry");
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config(&fixture, Some("coordinator")),
        &options(None),
    )
    .unwrap();
    assert_eq!(report["restart_detected"], false);
    assert_eq!(report["coordinator"]["action"], "skipped");
    let text = fixture.client.runs.lock().unwrap().join("\n");
    assert!(
        text.contains(
            "Restore on the current boot 22222222-3333-4444-5555-666666666666; no restart detected"
        ),
        "{text}"
    );
    assert!(!text.contains("did not survive"), "{text}");
    assert!(!text.contains("The host restarted"), "{text}");
}

#[test]
fn after_a_restart_restore_holds_boxed_slots_then_hands_each_to_its_back_agent() {
    let fixture = Fixture::new();
    // A dead lane without a slot makes the anchors show an earlier boot.
    dead_agent(&fixture);
    // A boxed lane whose launch timed out while its harness came up, at pane "owned-2".
    fixture
        .manager()
        .start(
            "boxed",
            &fixture.root,
            StartOptions {
                harness: "codex".to_owned(),
                resume: Some("goal-conversation".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let mut boxed = fixture.manager().load("boxed").unwrap();
    boxed.lifecycle = "launch_failed".to_owned();
    boxed.harness_identity = None;
    boxed.anchor_rule = None;
    boxed.terminal_id = None;
    boxed.session_agent = None;
    boxed.session_value = None;
    boxed.slot = Some("boxed-slot".to_owned());
    boxed.slot_project = Some(fixture.root.display().to_string());
    boxed.slot_isolation = Some("userns".to_owned());
    boxed.launch_spec = None;
    fixture.manager().save(&boxed).unwrap();
    let host = ScriptedHost::new();
    let status = json!({"active": [{
        "slot": "boxed-slot", "agent": "boxed", "generation": 3,
        "hold_reason": "reboot: agent boxed waits for agentctl restore",
    }]})
    .to_string();
    host.answer("status --slot boxed-slot --format json", 0, &status, "");
    let registry = fixture.root.join("registry");
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config(&fixture, None),
        &options(Some("/opt/wrkslots")),
    )
    .unwrap();
    assert_eq!(report["restart_detected"], true);
    // A launch_failed record is not running, so only running boxed agents are held.
    assert_eq!(report["slot_holds"], json!([]), "{report}");
    let boxed_row = report["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "boxed")
        .unwrap()
        .clone();
    assert_eq!(boxed_row["action"], "promoted", "{report}");
    assert_eq!(boxed_row["slot_owner"]["action"], "transferred", "{report}");
    assert_eq!(boxed_row["slot_owner"]["unheld"], true);
    let root = fixture.root.display().to_string();
    let pid = boxed_row["slot_owner"]["owner_pid"].as_u64().unwrap();
    let commands = host.commands();
    assert!(commands.contains(&format!(
        "/opt/wrkslots --project-root {root} transfer-owner boxed-slot --agent boxed --owner-pid {pid} --expected-generation 3"
    )), "{commands:?}");
    assert!(commands.contains(&format!(
        "/opt/wrkslots --project-root {root} unhold boxed-slot"
    )));
}

#[test]
fn after_a_restart_restore_holds_the_slot_of_each_running_boxed_agent() {
    let fixture = Fixture::new();
    let mut lane = dead_agent(&fixture);
    lane.slot = Some("lane-slot".to_owned());
    lane.slot_project = Some(fixture.root.display().to_string());
    lane.slot_isolation = Some("userns".to_owned());
    lane.launch_spec = None;
    fixture.manager().save(&lane).unwrap();
    let host = ScriptedHost::new();
    host.answer(
        "hold lane-slot",
        1,
        "",
        "slot lane-slot is already held for a different reason: keep",
    );
    let registry = fixture.root.join("registry");
    let report = restore(
        &fixture.client,
        &registry,
        &host,
        &config(&fixture, None),
        &RestoreOptions {
            dry_run: false,
            ..options(Some("/opt/wrkslots"))
        },
    )
    .unwrap();
    assert_eq!(report["slot_holds"][0]["slot"], "lane-slot", "{report}");
    assert_eq!(report["slot_holds"][0]["action"], "failed");
    let root = fixture.root.display().to_string();
    assert_eq!(
        host.commands()[1],
        format!("/opt/wrkslots --project-root {root} hold lane-slot --reason reboot: agent lane waits for agentctl restore")
    );
}
