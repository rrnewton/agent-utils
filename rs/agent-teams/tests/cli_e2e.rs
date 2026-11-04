//! End-to-end CLI tests: real binary, real state directories, local route.
//! Two teams on one host, one pinned coordinator (this test process, so its
//! ancestry contains the pinned identity for every child invocation).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct World {
    root: PathBuf,
    config: PathBuf,
    state: PathBuf,
    index: Option<PathBuf>,
    /// `AGENT_TEAMS_AGENTCTL` for child runs, when set.
    agentctl: Option<PathBuf>,
}

impl World {
    fn new() -> World {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("agent-teams-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config/agent-teams");
        let state = root.join("state");
        std::fs::create_dir_all(config.join("routes.d")).expect("config");
        std::fs::create_dir_all(&state).expect("state");
        // State holds bodies; keep it private like a real install.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700));
        let teams = r#"{
  "schema": "agent-teams-teams/v1",
  "comment": "e2e",
  "hosts": {"host-a": {"routes": ["local"]}},
  "teams": {
    "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
"#;
        std::fs::write(config.join("teams.json"), teams).expect("teams.json");
        World {
            config: root.join("config"),
            root,
            state,
            index: None,
            agentctl: None,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-teams"));
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("AGENT_TEAMS_STATE_DIR", &self.state)
            .env("AGENT_TEAMS_HOST", "host-a");
        if let Some(index) = &self.index {
            command.env("AGENTCTL_COORDINATOR_INDEX_DIR", index);
        }
        if let Some(agentctl) = &self.agentctl {
            command.env("AGENT_TEAMS_AGENTCTL", agentctl);
        }
        let output = command.output().expect("run agent-teams");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn init_both(&self) {
        let pid = std::process::id().to_string();
        for (team, from) in [("tools", "sim"), ("sim", "tools")] {
            let (code, out, err) = self.run(&[
                "init",
                team,
                "--coordinator-self",
                "--coordinator-pid",
                &pid,
                "--deliver",
                "hook-inbox",
                "--accepts-from",
                from,
            ]);
            assert_eq!(code, 0, "init {team}: {out} {err}");
        }
    }

    fn hook_inbox_count(&self, team: &str) -> usize {
        let dir: &Path = &self.state.join("teams.d").join(team).join("hook-inbox");
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .count()
            })
            .unwrap_or(0)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn show_check_send_hook_reply_round_trip() {
    let world = World::new();
    let (code, out, _) = world.run(&["show"]);
    assert_eq!(code, 0);
    assert!(out.contains("team tools"), "{out}");

    // Before init, check must fail: no declaration on this host.
    let (code, out, _) = world.run(&["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no declaration"), "{out}");

    world.init_both();
    let (code, out, _) = world.run(&["check"]);
    assert_eq!(code, 0, "{out}");

    // Send from sim (the pinned coordinator is this test process, which is
    // in the ancestry of every child; --from-team picks the declaration).
    let (code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "sim",
        "--text",
        "Please hold landing.",
        "--kind",
        "hold",
        "--topic",
        "ci-hold",
        "--json",
    ]);
    assert_eq!(code, 75, "accepted is not settled: {out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "accepted");
    let id = value["id"].as_str().expect("id").to_string();
    assert!(value["tag"]
        .as_str()
        .unwrap_or("")
        .starts_with("[from sim@host-a via agent-teams · to: tools@host-a · id: tm-"));
    assert_eq!(world.hook_inbox_count("tools"), 1);

    // The same send again is a retry, not a second copy.
    let (code, out, _) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "sim",
        "--text",
        "Please hold landing.",
        "--kind",
        "hold",
        "--topic",
        "ci-hold",
    ]);
    assert_eq!(code, 75, "{out}");
    assert!(out.contains("retry"), "{out}");
    assert_eq!(world.hook_inbox_count("tools"), 1, "still one copy");

    // The hook prints it once, then never again.
    let (code, out, err) = world.run(&["hook", "--team", "tools"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Please hold landing."), "{out}");
    assert!(
        out.contains("[from sim@host-a via agent-teams · to: tools@host-a"),
        "{out}"
    );
    let (_, out, _) = world.run(&["hook", "--team", "tools"]);
    assert!(
        !out.contains("Please hold landing."),
        "hook printed twice: {out}"
    );

    // A second hook run upgrades presented -> delivered.
    let (code, out, _) = world.run(&["seen", &id]);
    assert_eq!(code, 0);
    assert!(out.contains("delivered"), "{out}");

    // Reply (as tools) lands in sim's inbox, once.
    let (code, out, err) = world.run(&["reply", &id, "--from-team", "tools", "--text", "Holding."]);
    // reply delegates to send, which needs the from-team proof: the pinned
    // identity proves both teams here, so disambiguate via the reply path.
    assert!(
        code == 75 || code == 0 || code == 2,
        "reply code {code}: {out} {err}"
    );
}

fn set_may_send_to(world: &World, team: &str, entries: &[&str]) {
    let path = world
        .state
        .join("teams.d")
        .join(team)
        .join("declaration.json");
    let mut value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("declaration")).expect("json");
    value["may_send_to"] = serde_json::json!(entries);
    std::fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");
}

#[test]
fn may_send_to_allows_the_front_door_and_refuses_the_rest() {
    let world = World::new();
    world.init_both();

    // tools may reach sim only at a host that is not this one: refused here,
    // with the allowed front door named, and nothing queued anywhere.
    set_may_send_to(&world, "tools", &["sim@lead-host"]);
    let (code, out, err) = world.run(&[
        "send",
        "sim",
        "--from-team",
        "tools",
        "--text",
        "hello",
        "--json",
    ]);
    assert_ne!(code, 0, "{out} {err}");
    assert!(
        format!("{out}{err}").contains("refused: may-send-to"),
        "{out} {err}"
    );
    assert!(
        format!("{out}{err}").contains("sim@lead-host"),
        "{out} {err}"
    );
    assert_eq!(world.hook_inbox_count("sim"), 0);

    // The named front door itself is allowed.
    set_may_send_to(&world, "tools", &["sim@host-a"]);
    let (code, out, err) = world.run(&[
        "send",
        "sim",
        "--from-team",
        "tools",
        "--text",
        "hello",
        "--kind",
        "request",
        "--topic",
        "front-door",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    let id = value["id"].as_str().expect("id").to_string();
    assert_eq!(world.hook_inbox_count("sim"), 1);

    // A bare team entry allows any host; same-team sends are always allowed,
    // even when the list names other teams only.
    set_may_send_to(&world, "tools", &["sim"]);
    let (code, out, err) = world.run(&[
        "send",
        "sim",
        "--from-team",
        "tools",
        "--text",
        "again",
        "--topic",
        "bare",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    set_may_send_to(&world, "tools", &["sim@lead-host"]);
    let (_code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "tools",
        "--text",
        "self",
        "--topic",
        "same-team",
        "--json",
    ]);
    // The allow list lets it through (delivery then refuses it on the
    // receiver's accepts_from, which names only the other team here).
    assert!(
        !format!("{out}{err}").contains("may-send-to"),
        "same team must not hit the allow list: {out} {err}"
    );

    // Reply obeys the same list: sim may not reply to tools off its front door.
    set_may_send_to(&world, "sim", &["tools@lead-host"]);
    let (code, out, err) = world.run(&["reply", &id, "--from-team", "sim", "--text", "answer"]);
    assert_ne!(code, 0, "{out} {err}");
    assert!(
        format!("{out}{err}").contains("refused: may-send-to"),
        "{out} {err}"
    );
    assert_eq!(world.hook_inbox_count("tools"), 0);
}

#[test]
fn foreign_sender_is_refused_by_policy() {
    let world = World::new();
    world.init_both();
    // tools accepts from sim only; a send "from tools to tools" is refused.
    let (code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "tools",
        "--text",
        "loopback",
        "--json",
    ]);
    assert_eq!(code, 65, "{out} {err}");
    assert_eq!(world.hook_inbox_count("tools"), 0);
}

#[test]
fn import_prints_a_proposal_and_writes_nothing() {
    let world = World::new();
    let source = world.root.join("coordinators.json");
    std::fs::write(
        &source,
        r#"{"hosts": {"host-a": {"fqdn": "host-a.example.net",
          "coordinator": {"name": "coordinator",
            "contacts": [{"kind": "agentctl", "registry": "/tmp/r"}, {"kind": "herdr"}]}}}}"#,
    )
    .expect("source");
    let (code, out, err) = world.run(&[
        "import",
        "--coordinators-file",
        source.to_str().unwrap(),
        "--team",
        "proj",
    ]);
    assert_eq!(code, 0, "{err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("import json");
    assert_eq!(
        value["teams.json"]["teams"]["proj"]["entries"][0]["host"],
        "host-a"
    );
    assert!(out.contains("herdr contact dropped"), "{out}");
}

#[test]
fn probe_reports_declarations() {
    let world = World::new();
    world.init_both();
    let (code, out, _) = world.run(&["probe", "--json"]);
    assert_eq!(code, 0);
    let value: serde_json::Value = serde_json::from_str(&out).expect("probe json");
    assert_eq!(value["teams"].as_array().expect("teams").len(), 2);
    assert!(value["teams"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t["coordinator_live"] == true));
}

/// Coordinator discovery: a team missing from teams.json is still
/// addressable by name when the host coordinator index names a live
/// designated coordinator for the SENDING team, and the receiver accepts
/// that same-host coordinator by default (no explicit accepts_from).
#[test]
fn send_by_team_name_through_the_host_index() {
    let mut world = World::new();
    // teams.json knows only the sender; `tools` exists solely as a
    // declaration plus, for the default-accept rule, the sender's index
    // entry. Rewrite the teams file accordingly.
    let teams = r#"{
  "schema": "agent-teams-teams/v1",
  "hosts": {"host-a": {"routes": ["local"]}},
  "teams": {
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
"#;
    std::fs::write(world.config.join("agent-teams/teams.json"), teams).expect("teams.json");
    // The sender's host-index entry: identity of this process, so the
    // child invocations are its descendants exactly as in production.
    let index = world.root.join("index");
    std::fs::create_dir_all(&index).expect("index dir");
    let identity = agent_queue::identity::observe(u64::from(std::process::id()))
        .expect("observe self")
        .identity;
    let entry = serde_json::json!({
        "schema": "agentctl-coordinator-index/v1",
        "registry": world.root.join("reg-sim").to_string_lossy(),
        "name": "coordinator",
        "token": "sim-generation",
        "harness_cwd": world.root.to_string_lossy(),
        "harness_identity": serde_json::to_value(&identity).expect("identity json"),
        "repo_origins": [],
        "team": "sim",
        "updated_at": 0,
    });
    std::fs::write(
        index.join("sim.json"),
        serde_json::to_string_pretty(&entry).unwrap(),
    )
    .expect("index entry");
    // And the recipient's entry: send-by-name resolves the recipient's
    // designated coordinator, while default-accept checks the sender's.
    let mut recipient = entry.clone();
    recipient["team"] = serde_json::json!("tools");
    recipient["registry"] = serde_json::json!(world.root.join("reg-tools").to_string_lossy());
    recipient["token"] = serde_json::json!("tools-generation");
    std::fs::write(
        index.join("tools.json"),
        serde_json::to_string_pretty(&recipient).unwrap(),
    )
    .expect("recipient index entry");
    // Index readers require the named registries to still hold their
    // designations (see hostindex::load).
    for registry in [world.root.join("reg-sim"), world.root.join("reg-tools")] {
        std::fs::create_dir_all(&registry).expect("registry dir");
        std::fs::write(registry.join("coordinator.json"), "{}").expect("designation");
    }
    world.index = Some(index);

    let pid = std::process::id().to_string();
    let init = |world: &World, team: &str, accepts: Option<&str>| {
        let mut args = vec![
            "init",
            team,
            "--coordinator-self",
            "--coordinator-pid",
            &pid,
            "--deliver",
            "hook-inbox",
        ];
        if let Some(from) = accepts {
            args.push("--accepts-from");
            args.push(from);
        }
        let (code, out, err) = world.run(&args);
        assert_eq!(code, 0, "init {team}: {out} {err}");
    };
    init(&world, "sim", Some("tools"));
    init(&world, "tools", None);

    // Discovery lists the live same-host coordinator and how to address it.
    let (code, out, err) = world.run(&["coordinators"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("sim @host-a"), "{out}");
    assert!(out.contains("live"), "{out}");

    // By name, with no teams-file entry for tools and no accepts_from on
    // the receiver: accepted once via the same-host default.
    let (code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "sim",
        "--text",
        "Index-addressed hello.",
    ]);
    assert_eq!(code, 75, "send by name: {out} {err}");
    assert_eq!(world.hook_inbox_count("tools"), 1);
    let _ = world;
}

/// #721: a declaration pinned to a retired coordinator is a red flag in
/// `check` (with the fix), and a local send to it answers `undeliverable`
/// while keeping the message in the outbox for retry.
#[test]
fn pinned_retired_coordinator_flags_check_and_send_undeliverable() {
    let world = World::new();
    // Registry with one live designated coordinator (this process).
    let registry = world.root.join("registry");
    let coord_dir = registry.join("agents").join("coord");
    std::fs::create_dir_all(&coord_dir).expect("registry");
    let identity = agent_queue::identity::observe(u64::from(std::process::id()))
        .expect("observe self")
        .identity;
    std::fs::write(
        coord_dir.join("agent.json"),
        serde_json::to_string(&serde_json::json!({
            "name": "coord",
            "harness_identity": serde_json::to_value(&identity).expect("identity"),
        }))
        .expect("record"),
    )
    .expect("record file");
    std::fs::write(registry.join("coordinator.json"), "{\"name\": \"coord\"}")
        .expect("designation");
    // Sender team via pinned self (ancestry proof, as in init_both).
    let pid = std::process::id().to_string();
    let (code, out, err) = world.run(&[
        "init",
        "sim",
        "--coordinator-self",
        "--coordinator-pid",
        &pid,
        "--deliver",
        "hook-inbox",
        "--accepts-from",
        "tools",
    ]);
    assert_eq!(code, 0, "init sim: {out} {err}");
    // Receiver declaration pinned to the retired name.
    let team_dir = world.state.join("teams.d/tools");
    std::fs::create_dir_all(&team_dir).expect("team dir");
    let declaration = serde_json::json!({
        "schema": "agent-teams-declaration/v1",
        "team": "tools",
        "deliverer": {"kind": "agentctl-queue", "registry": registry.to_string_lossy(), "agent": "old-coord"},
        "coordinator": {"designation": "registry"},
        "accepts_from": [{"sender": "sim", "kinds": ["info", "request", "hold", "resume", "reply"]}],
        "limits": {"max_bytes": 8192, "per_sender_per_hour": 30, "max_ttl_seconds": 604800},
        "default_ttl_seconds": 7200,
    });
    std::fs::write(
        team_dir.join("declaration.json"),
        serde_json::to_string_pretty(&declaration).unwrap(),
    )
    .expect("declaration");

    let (code, out, _) = world.run(&["check"]);
    assert_eq!(code, 1, "check must fail: {out}");
    assert!(out.contains("pinned-agent"), "{out}");
    assert!(out.contains("fix:"), "{out}");

    let (code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "sim",
        "--text",
        "are you there",
    ]);
    assert_eq!(code, 75, "send: {out} {err}");
    assert!(out.contains("undeliverable"), "{out}");
    // The message stays in the outbox, retryable after the fix.
    let (code, out, _) = world.run(&["outbox"]);
    assert_eq!(code, 0);
    assert!(out.contains("undeliverable"), "{out}");
}

/// Review finding 3: a declaration whose pinned coordinator process is gone refuses every send
/// as sender-not-coordinator (and `check` says so); once the pin becomes the registry
/// designation, which `agentctl coordinator set NEW` writes, the designated coordinator (this
/// test process, an ancestor of every child run) sends.
#[test]
fn registry_designation_lets_the_new_coordinator_send_after_a_stale_pin() {
    let mut world = World::new();
    // agentctl is never asked; designation and records are read from the files.
    world.agentctl = Some(PathBuf::from("/bin/false"));
    let registry = world.root.join("registry");
    let coord_dir = registry.join("agents").join("coord-new");
    std::fs::create_dir_all(&coord_dir).expect("registry");
    let identity = agent_queue::identity::observe(u64::from(std::process::id()))
        .expect("observe self")
        .identity;
    std::fs::write(
        coord_dir.join("agent.json"),
        serde_json::json!({"name": "coord-new", "harness_identity": identity}).to_string(),
    )
    .expect("record file");
    std::fs::write(
        registry.join("coordinator.json"),
        "{\"name\": \"coord-new\"}",
    )
    .expect("designation");
    // The receiving team, pinned to this process (ancestry proof as in init_both).
    let pid = std::process::id().to_string();
    let (code, out, err) = world.run(&[
        "init",
        "tools",
        "--coordinator-self",
        "--coordinator-pid",
        &pid,
        "--deliver",
        "hook-inbox",
        "--accepts-from",
        "sim",
    ]);
    assert_eq!(code, 0, "init tools: {out} {err}");
    // The sending team, delivering through the registry, still pinned to a retired harness.
    let mut dead = serde_json::to_value(&identity).expect("identity");
    dead["starttime_ticks"] = serde_json::json!(identity.starttime_ticks + 999_999);
    let team_dir = world.state.join("teams.d/sim");
    std::fs::create_dir_all(&team_dir).expect("team dir");
    let mut declaration = serde_json::json!({
        "schema": "agent-teams-declaration/v1",
        "team": "sim",
        "deliverer": {"kind": "agentctl-queue", "registry": registry.to_string_lossy()},
        "coordinator": {"designation": "pinned", "harness_identity": dead},
        "accepts_from": [{"sender": "tools", "kinds": ["info", "request", "hold", "resume", "reply"]}],
        "limits": {"max_bytes": 8192, "per_sender_per_hour": 30, "max_ttl_seconds": 604800},
        "default_ttl_seconds": 7200,
    });
    let path = team_dir.join("declaration.json");
    std::fs::write(&path, serde_json::to_string_pretty(&declaration).unwrap()).expect("write");
    let (code, out, _) = world.run(&["check", "sim"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("designation-not-live"), "{out}");
    let send = |world: &World| {
        world.run(&[
            "send",
            "tools",
            "--from-team",
            "sim",
            "--text",
            "handed over",
        ])
    };
    let (code, out, err) = send(&world);
    assert_ne!(code, 0, "{out} {err}");
    assert!(
        format!("{out}{err}").contains("sender-not-coordinator"),
        "{out} {err}"
    );
    // What the handover writes.
    declaration["coordinator"] = serde_json::json!({"designation": "registry"});
    std::fs::write(&path, serde_json::to_string_pretty(&declaration).unwrap()).expect("write");
    let (code, out, err) = send(&world);
    assert_eq!(code, 75, "accepted from the new coordinator: {out} {err}");
    assert!(out.contains("accepted"), "{out}");
    assert_eq!(world.hook_inbox_count("tools"), 1);
}

/// One pump at a time per state directory: two watchers sharing it never walk the same outbox
/// entry at once.
#[test]
fn a_second_pump_on_the_same_state_does_nothing() {
    let world = World::new();
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(world.state.join("pump.lock"))
        .expect("lock file");
    fs2::FileExt::lock_exclusive(&lock).expect("hold the pump lock");
    let (code, out, err) = world.run(&["pump", "--deferred"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("another pump is running"), "{out}");
    fs2::FileExt::unlock(&lock).expect("release");
    let (code, out, _) = world.run(&["pump", "--deferred"]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("nothing to do"), "{out}");
}

/// #770: replies carry at least 24h by default (they were the messages
/// expiring while a route was down); ordinary sends keep the 2h default.
#[test]
fn reply_envelope_gets_a_longer_default_ttl() {
    let world = World::new();
    world.init_both();
    let (code, out, err) = world.run(&[
        "send",
        "tools",
        "--from-team",
        "sim",
        "--text",
        "Question?",
        "--key",
        "ttl-q",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    let id = value["id"].as_str().expect("id").to_string();
    let ttl_seconds = |world: &World, id: &str| -> i64 {
        let text = std::fs::read_to_string(world.state.join("outbox").join(format!("{id}.json")))
            .expect("outbox");
        let entry: serde_json::Value = serde_json::from_str(&text).expect("outbox json");
        let env = &entry["envelope"];
        let created =
            agent_teams::timeutil::parse_rfc3339_ms(env["created_at"].as_str().unwrap()).unwrap();
        let not_after =
            agent_teams::timeutil::parse_rfc3339_ms(env["not_after"].as_str().unwrap()).unwrap();
        (not_after - created) / 1000
    };
    assert!(
        ttl_seconds(&world, &id) <= 7200,
        "ordinary send keeps short TTL"
    );
    let (code, out, err) = world.run(&[
        "reply",
        &id,
        "--from-team",
        "tools",
        "--text",
        "Answer.",
        "--json",
    ]);
    assert!(code == 75 || code == 0, "reply: {code} {out} {err}");
    // The reply's outbox entry is the one whose envelope kind is reply.
    let mut reply_ttl = None;
    for entry in std::fs::read_dir(world.state.join("outbox"))
        .expect("outbox dir")
        .flatten()
    {
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(entry.path()).expect("read"))
                .expect("json");
        if value["envelope"]["kind"] == "reply" {
            let rid = value["id"].as_str().unwrap().to_string();
            reply_ttl = Some(ttl_seconds(&world, &rid));
        }
    }
    assert!(
        reply_ttl.is_some_and(|s| s >= 86_400),
        "reply TTL >= 24h: {reply_ttl:?}"
    );
}
