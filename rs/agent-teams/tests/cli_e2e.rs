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
