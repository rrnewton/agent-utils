//! Chat relay end-to-end tests (#719 agent-teams-slack-relay): a fake chat
//! channel as files, the relay route posting into it, a simulated bridge
//! piping posts to `relay-in`, receipts back in the thread, stranger
//! quarantine and alarms. No live provider, channel, or service is touched.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
const CHAN: &str = "CHAN1";
const BOT1: &str = "UBOTSIM1";
const BOT2: &str = "UBOTSIM2";
const STRANGER: &str = "USTRANGER";

struct World {
    root: PathBuf,
    config: PathBuf,
    state: PathBuf,
    channel_file: PathBuf,
}

impl World {
    fn new() -> World {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("agent-teams-relay-e2e-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = root.join("config/agent-teams");
        let state = root.join("state");
        std::fs::create_dir_all(config.join("routes.d")).expect("config");
        std::fs::create_dir_all(&state).expect("state");
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        let channel_file = root.join("channel.txt");
        // Fake chat helpers: post appends text + separator; list cats the
        // channel; receipt posts into the same channel (its thread).
        for (name, body) in [
            (
                "post.sh",
                format!(
                    "#!/bin/sh\nwhile [ $# -gt 0 ]; do case \"$1\" in --text-file) shift; cat \"$1\" >> \"{}\";; esac; shift; done\nprintf '\\n---post---\\n' >> \"{}\"\n",
                    channel_file.display(),
                    channel_file.display()
                ),
            ),
            (
                "list.sh",
                format!("#!/bin/sh\ncat \"{}\" 2>/dev/null || true\n", channel_file.display()),
            ),
        ] {
            let path = bin.join(name);
            std::fs::write(&path, body).expect("helper");
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        let post = bin.join("post.sh");
        let list = bin.join("list.sh");
        let teams = r#"{
  "schema": "agent-teams-teams/v1",
  "hosts": {"host-a": {"routes": ["relay-coord"], "agent_teams": "agent-teams"}},
  "teams": {
    "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
"#;
        std::fs::write(config.join("teams.json"), teams).expect("teams.json");
        let routes = format!(
            r#"{{
  "schema": "agent-teams-routes/v1",
  "routes": {{"relay-coord": {{
    "driver": "command", "mode": "relay", "channel": "{CHAN}", "sender_id": "{BOT1}",
    "post_argv": ["{}", "post", "--channel", "{{channel}}", "--text-file", "{{text_file}}"],
    "list_argv": ["{}", "list", "--channel", "{{channel}}"],
    "max_envelope_bytes": 10000, "limits": {{"per_hour": 100}}
  }}}}
}}
"#,
            post.display(),
            list.display()
        );
        std::fs::write(config.join("routes.d/relay.json"), routes).expect("routes");
        let config_root = root.join("config");
        World {
            root,
            config: config_root,
            state,
            channel_file,
        }
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        self.run_stdin(args, "")
    }

    fn run_stdin(&self, args: &[&str], stdin_text: &str) -> (i32, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-teams"));
        command
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("AGENT_TEAMS_STATE_DIR", &self.state)
            .env("AGENT_TEAMS_HOST", "host-a")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().expect("run agent-teams");
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(stdin_text.as_bytes())
            .expect("write stdin");
        let output = child.wait_with_output().expect("wait");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn init_tools(&self, relay_spec: &str) {
        let pid = std::process::id().to_string();
        let (code, out, err) = self.run(&[
            "init",
            "tools",
            "--coordinator-self",
            "--coordinator-pid",
            &pid,
            "--deliver",
            "hook-inbox",
            "--accepts-from",
            "sim",
            "--relay",
            relay_spec,
        ]);
        assert_eq!(code, 0, "init tools: {out} {err}");
        let (code, out, err) = self.run(&[
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
    }

    fn channel_posts(&self) -> Vec<String> {
        let text = std::fs::read_to_string(&self.channel_file).unwrap_or_default();
        text.split("\n---post---\n")
            .map(str::trim)
            .filter(|post| !post.is_empty())
            .map(str::to_string)
            .collect()
    }

    fn hook_inbox_texts(&self, team: &str) -> Vec<String> {
        let dir: &Path = &self.state.join("teams.d").join(team).join("hook-inbox");
        let mut texts = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|x| x == "json") {
                    if let Ok(text) = std::fs::read_to_string(entry.path()) {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            texts.push(value["text"].as_str().unwrap_or_default().to_string());
                        }
                    }
                }
            }
        }
        texts
    }

    fn relay_in(&self, sender: &str, post: &str) -> serde_json::Value {
        let (code, out, err) = self.run_stdin(
            &[
                "relay-in",
                "--route",
                "relay-coord",
                "--peer-sender",
                sender,
                "--channel",
                CHAN,
                "--thread",
                "T1",
            ],
            post,
        );
        assert_eq!(code, 0, "relay-in: {out} {err}");
        serde_json::from_str(out.lines().next().unwrap_or("{}")).expect("relay-in json")
    }

    fn send_sim_to_tools(&self, text: &str, key: &str) -> (String, String) {
        let (code, out, err) = self.run(&[
            "send",
            "tools@host-a",
            "--from-team",
            "sim",
            "--text",
            text,
            "--key",
            key,
            "--json",
        ]);
        assert_eq!(code, 75, "relay send settles in-custody: {out} {err}");
        let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
        assert_eq!(value["state"], "in-custody", "{out}");
        (
            value["id"].as_str().expect("id").to_string(),
            self.channel_posts()
                .into_iter()
                .last()
                .expect("one relay post"),
        )
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn relay_round_trip_two_bots_one_team_receipt_and_exactly_once() {
    let world = World::new();
    // Two sender IDs map to the same team (sim@host-a): the Discord
    // two-vibing-bots case. One bot per team stays the normal case.
    world.init_tools(&format!(
        "relay-coord:{CHAN}:{BOT1}=sim@host-a,{BOT2}=sim@host-a"
    ));
    let (id, post) = world.send_sim_to_tools("Please hold landing.", "relay-rt-1");
    // The channel the owner reads: a human line, then the machine block.
    assert!(post.contains("Please hold landing."), "{post}");
    assert!(post.contains("```agent-teams-envelope"), "{post}");
    assert!(post.contains("[from sim@host-a via agent-teams"), "{post}");

    // The receiving bridge pipes it to relay-in with the poster's ID.
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    let receipt = outcome["receipt_text"].as_str().expect("receipt text");
    assert!(receipt.contains("receipt: accepted"), "{receipt}");
    assert!(receipt.contains("```agent-teams-receipt"), "{receipt}");
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);

    // The same post again (rescan/replay): known, never a second copy.
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "known", "{outcome}");
    assert_eq!(world.hook_inbox_texts("tools").len(), 1, "exactly once");

    // The second bot of the same team delivers the next message.
    let (_id2, post2) = world.send_sim_to_tools("Second bot, same team.", "relay-rt-2");
    let outcome = world.relay_in(BOT2, &post2);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    assert_eq!(world.hook_inbox_texts("tools").len(), 2);

    // The receipt comes back in the thread, posted by the receiving team's
    // bot. The sender confirms that bot's announcement first (the poster
    // must be mapped to the fleet the message was sent to).
    let tools_announce = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        "UBOTTOOLS",
        "https://example.invalid/org/repo.git",
    );
    let outcome = world.relay_in(
        "UBOTTOOLS",
        &agent_teams::relay::render_announce(&tools_announce),
    );
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
    let (code, out, err) = world.run(&["relay", "confirm", "UBOTTOOLS", "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    // A stranger's forged receipt for the same ID is refused, state intact.
    let outcome = world.relay_in(STRANGER, receipt);
    assert_eq!(outcome["outcome"], "receipt-refused", "{outcome}");
    let outcome = world.relay_in("UBOTTOOLS", receipt);
    assert_eq!(outcome["outcome"], "receipt-recorded", "{outcome}");
    assert!(outcome["receipt_text"].is_null(), "{outcome}");
    let (_, out, _) = world.run(&["status", &id, "--json"]);
    let value: serde_json::Value = serde_json::from_str(&out).expect("status json");
    assert_eq!(value["outbox"]["state"], "accepted", "{out}");
}

#[test]
fn other_fleet_post_is_filtered_silently() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    // An envelope for a fleet with no local team: filtered by relay-in,
    // no delivery, no receipt, no alarm, no token spent.
    let post = agent_teams::relay::render_post(&agent_teams::envelope::build_envelope(
        "tm-sim-0123456789abcdef",
        "k",
        "sim",
        "host-b",
        1,
        "harness-ancestry",
        "thirdfleet",
        "host-a",
        "info",
        None,
        &[],
        false,
        0,
        7_200_000,
        None,
        "for someone else",
    ));
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "ignored-other-team", "{outcome}");
    assert!(outcome["receipt_text"].is_null(), "silent: {outcome}");
    assert!(world.hook_inbox_texts("tools").is_empty());
    let (_code, out, _) = world.run(&["relay", "alarms"]);
    assert!(out.contains("no open stranger alarms"), "{out}");
}

#[test]
fn stranger_is_loud_quarantined_then_delivered_after_confirmation() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    // The stranger claims sim@host-a with a well-formed envelope.
    let envelope = agent_teams::envelope::build_envelope(
        "tm-sim-aaaaaaaabbbbbbbb",
        "kq",
        "sim",
        "host-a",
        7,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        agent_teams::timeutil::now_unix_ms(),
        agent_teams::timeutil::now_unix_ms() + 7_200_000,
        None,
        "quarantined hello",
    );
    // A 1-second alarm interval for this test, set before the first post.
    let declaration_path = world.state.join("teams.d/tools/declaration.json");
    let mut declaration: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&declaration_path).expect("declaration"))
            .expect("declaration json");
    declaration["relay_channels"][0]["alarm_interval_seconds"] = serde_json::json!(1);
    std::fs::write(
        &declaration_path,
        serde_json::to_string_pretty(&declaration).expect("write declaration"),
    )
    .expect("write declaration");
    let post = agent_teams::relay::render_post(&envelope);
    let outcome = world.relay_in(STRANGER, &post);
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    // Not delivered; the coordinator got a loud alarm instead.
    let inbox = world.hook_inbox_texts("tools");
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert!(inbox[0].contains("stranger alarm"), "{inbox:?}");
    assert!(inbox[0].contains("ESCALATE TO OWNER"), "{inbox:?}");
    let (_code, out, _) = world.run(&["relay", "status"]);
    assert!(
        out.contains("STRANGER ALARM"),
        "status shows alarms first: {out}"
    );
    assert_eq!(
        agent_teams::relay::quarantined_posts(&world.state).len(),
        1,
        "post kept in quarantine"
    );

    // The alarm repeats when due (1s interval, set above).
    std::thread::sleep(std::time::Duration::from_millis(1_100));
    let (_code, out, _) = world.run(&["relay", "alarms", "--json"]);
    let alarms: serde_json::Value = serde_json::from_str(&out).expect("alarms json");
    assert_eq!(
        alarms["raised_now"].as_array().map_or(0, Vec::len),
        1,
        "{out}"
    );
    let raises = alarms["alarms"][0]["raises"].as_u64().unwrap_or(0);
    assert!(raises >= 2, "alarm repeated: {out}");

    // Announce + owner confirmation delivers the quarantined post, once.
    let announce = agent_teams::relay::build_announce(
        "sim",
        "host-a",
        STRANGER,
        "https://example.invalid/org/repo.git",
    );
    let outcome = world.relay_in(STRANGER, &agent_teams::relay::render_announce(&announce));
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
    // A candidate alone never admits: `relay teams` shows it unconfirmed.
    let (_code, out, _) = world.run(&["relay", "teams"]);
    assert!(out.contains("candidate: sim@host-a"), "{out}");
    let (code, out, err) = world.run(&["relay", "confirm", STRANGER, "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("delivered 1 quarantined"), "{out}");
    let inbox = world.hook_inbox_texts("tools");
    assert_eq!(
        inbox
            .iter()
            .filter(|text| text.contains("quarantined hello"))
            .count(),
        1,
        "delivered exactly once after confirmation: {inbox:?}"
    );
    assert!(agent_teams::relay::quarantined_posts(&world.state).is_empty());
    let (_code, out, _) = world.run(&["relay", "alarms"]);
    assert!(
        out.contains("no open stranger alarms"),
        "alarm closed: {out}"
    );
}

#[test]
fn banned_stranger_stops_the_alarm() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let envelope = agent_teams::envelope::build_envelope(
        "tm-sim-ccccccccdddddddd",
        "kb",
        "sim",
        "host-a",
        3,
        "harness-ancestry",
        "tools",
        "host-a",
        "info",
        None,
        &[],
        false,
        agent_teams::timeutil::now_unix_ms(),
        agent_teams::timeutil::now_unix_ms() + 7_200_000,
        None,
        "never delivered",
    );
    let post = agent_teams::relay::render_post(&envelope);
    let outcome = world.relay_in("UBADBOT", &post);
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    let (code, out, _) = world.run(&["relay", "ban", "UBADBOT"]);
    assert_eq!(code, 0, "{out}");
    // After the ban the sender is dropped, with no new alarm and no delivery.
    let outcome = world.relay_in("UBADBOT", &post);
    assert_eq!(outcome["outcome"], "banned-sender", "{outcome}");
    assert!(
        !world
            .hook_inbox_texts("tools")
            .iter()
            .any(|text| text.contains("never delivered")),
        "banned post never delivered"
    );
    let (_code, out, _) = world.run(&["relay", "alarms"]);
    assert!(out.contains("no open stranger alarms"), "{out}");
}

#[test]
fn setup_suggests_a_per_fleet_channel_and_writes_nothing() {
    let world = World::new();
    let (code, out, _) = world.run(&[
        "relay",
        "setup",
        "--team",
        "agent-utils",
        "--provider",
        "discord",
        "--peer",
        "UBOTSIM1=sim@host-a",
    ]);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("#relay-agent-utils"), "{out}");
    assert!(out.contains("relay_channels"), "{out}");
    assert!(out.contains("Never admit an unconfirmed claim"), "{out}");
    assert!(!world.channel_file.exists(), "setup writes nothing");
}
