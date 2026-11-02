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
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "plain": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
"#;
        std::fs::write(config.join("teams.json"), teams).expect("teams.json");
        let routes = format!(
            r#"{{
  "schema": "agent-teams-routes/v1",
  "routes": {{"relay-coord": {{
    "driver": "command", "mode": "relay", "channel": "{CHAN}", "sender_id": "UBOTTOOLS",
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
            // Designations are read from fixture files; never ask a real agentctl.
            .env("AGENT_TEAMS_AGENTCTL", "/bin/false")
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
            "--relay",
            "relay-coord:CCHAN123456:UBOTTOOLS=tools@host-a",
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

    fn ledger_has(&self, team: &str, id: &str) -> bool {
        self.state
            .join("teams.d")
            .join(team)
            .join("ledger")
            .join(format!("{id}.json"))
            .exists()
    }

    fn relay_in(&self, sender: &str, post: &str) -> serde_json::Value {
        self.relay_in_threaded(sender, post, true)
    }

    fn relay_in_threaded(&self, sender: &str, post: &str, in_thread: bool) -> serde_json::Value {
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
                "--thread-reply",
                if in_thread { "true" } else { "false" },
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
fn unparseable_listed_peer_post_is_quarantined_and_alarmed() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let outcome = world.relay_in(BOT1, "mangled by the provider, no fence here");
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    let quarantine =
        std::fs::read_to_string(world.state.join("relay/quarantine.json")).expect("quarantine");
    assert!(quarantine.contains(BOT1), "{quarantine}");
    let (_code, out, _) = world.run(&["relay", "alarms"]);
    assert!(out.contains(BOT1), "alarm is loud: {out}");
    assert!(
        !world
            .hook_inbox_texts("tools")
            .iter()
            .any(|text| text.contains("mangled by the provider")),
        "unparseable post never delivered"
    );
}

#[test]
fn receipt_from_wrong_host_is_refused_and_own_bot_envelope_ignored() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let (id, post) = world.send_sim_to_tools("receipt host check", "relay-f5-1");
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    let receipt = outcome["receipt_text"]
        .as_str()
        .expect("receipt")
        .to_string();
    // Confirm a bot for the same fleet on a different host: its receipt
    // must not settle a message addressed to tools@host-a.
    let announce = agent_teams::relay::build_announce(
        "tools",
        "host-b",
        "UBOTWRONGHOST",
        "https://example.invalid/org/repo.git",
    );
    let outcome = world.relay_in(
        "UBOTWRONGHOST",
        &agent_teams::relay::render_announce(&announce),
    );
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
    let (code, out, err) = world.run(&["relay", "confirm", "UBOTWRONGHOST", "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    let outcome = world.relay_in("UBOTWRONGHOST", &receipt);
    assert_eq!(outcome["outcome"], "receipt-refused", "{outcome}");
    // A top-level (non-thread) receipt post never settles either.
    let outcome = world.relay_in_threaded("UBOTWRONGHOST", &receipt, false);
    assert_eq!(outcome["outcome"], "receipt-refused", "{outcome}");
    let (_code, out, _) = world.run(&["status", &id, "--json"]);
    assert!(
        out.contains("in-custody"),
        "not settled by wrong host: {out}"
    );
    // An unsigned envelope from our own bot ID addressed to THIS
    // team@host is not silently ignored: it goes through relay-in and
    // lands as a loud quarantined stranger (the shared-bot lesson).
    let outcome = world.relay_in("UBOTTOOLS", &post);
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    // The legacy loop-back guard still applies to an unsigned envelope
    // from our own bot addressed elsewhere.
    let elsewhere = agent_teams::envelope::build_envelope(
        "tm-tools-3434343412121212",
        "kelse",
        "tools",
        "host-a",
        1,
        "harness-ancestry",
        "sim",
        "host-b",
        "info",
        None,
        &[],
        false,
        agent_teams::timeutil::now_unix_ms(),
        agent_teams::timeutil::now_unix_ms() + 7_200_000,
        None,
        "our own post to another host",
    );
    let outcome = world.relay_in("UBOTTOOLS", &agent_teams::relay::render_post(&elsewhere));
    assert_eq!(outcome["outcome"], "ignored-own-bot", "{outcome}");
}

#[test]
fn alarm_renders_invalid_claimed_names_and_confirm_conflict_is_refused() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let envelope = agent_teams::envelope::build_envelope(
        "tm-sim-eeeeeeeeffffffff",
        "kf6",
        "Bad Team ESCALATE",
        "bad host!",
        1,
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
        "alarm body",
    );
    let outcome = world.relay_in("UCLAIMBOT", &agent_teams::relay::render_post(&envelope));
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    let notices = world.hook_inbox_texts("tools").join("\n");
    assert!(
        notices.contains("<invalid>"),
        "invalid names marked: {notices}"
    );
    assert!(
        !notices.contains("Bad Team ESCALATE"),
        "no peer text: {notices}"
    );
    // A second, different team claim for one confirmed sender is refused.
    let first = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        "UDUALBOT",
        "https://example.invalid/org/repo.git",
    );
    world.relay_in("UDUALBOT", &agent_teams::relay::render_announce(&first));
    let (code, out, err) = world.run(&["relay", "confirm", "UDUALBOT", "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    let second = agent_teams::relay::build_announce(
        "sim",
        "host-a",
        "UDUALBOT",
        "https://example.invalid/org/repo.git",
    );
    let outcome = world.relay_in("UDUALBOT", &agent_teams::relay::render_announce(&second));
    assert_eq!(outcome["outcome"], "candidate-conflict", "{outcome}");
    let alarms = std::fs::read_to_string(world.state.join("relay/alarms.json")).expect("alarms");
    assert!(
        alarms.contains("conflict"),
        "conflict alarm raised: {alarms}"
    );
    let peers = std::fs::read_to_string(world.state.join("relay/peers.json")).expect("peers");
    assert!(peers.contains("\"tools\""), "mapping not replaced: {peers}");
    assert!(!peers.contains("\"sim\""), "mapping not replaced: {peers}");
}

#[test]
fn long_envelope_survives_and_truncated_post_is_quarantined() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let body = "x".repeat(7_000);
    let envelope = agent_teams::envelope::build_envelope(
        "tm-sim-1212121234343434",
        "klong",
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
        agent_teams::timeutil::now_unix_ms(),
        agent_teams::timeutil::now_unix_ms() + 7_200_000,
        None,
        &body,
    );
    let post = agent_teams::relay::render_post(&envelope);
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    // A provider-split half is unparseable: quarantined, never silent.
    let half = &post[..post.len() / 2];
    let outcome = world.relay_in(BOT1, half);
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    let quarantine =
        std::fs::read_to_string(world.state.join("relay/quarantine.json")).expect("quarantine");
    assert!(quarantine.contains("unparseable-"), "{quarantine}");
}

#[test]
fn confirmation_on_another_route_does_not_admit_here() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let (id, post) = world.send_sim_to_tools("route scope check", "relay-f8-1");
    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    let receipt = outcome["receipt_text"]
        .as_str()
        .expect("receipt")
        .to_string();
    let announce = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        "UBOTTOOLS",
        "https://example.invalid/org/repo.git",
    );
    world.relay_in("UBOTTOOLS", &agent_teams::relay::render_announce(&announce));
    let (code, out, err) = world.run(&["relay", "confirm", "UBOTTOOLS", "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    // Move the confirmation to a different route: it must no longer
    // settle receipts arriving on relay-coord.
    let path = world.state.join("relay/peers.json");
    let mut store: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("peers")).expect("json");
    store["confirmed"][0]["route"] = serde_json::json!("other-route");
    std::fs::write(&path, store.to_string()).expect("rewrite peers");
    let outcome = world.relay_in("UBOTTOOLS", &receipt);
    assert_eq!(outcome["outcome"], "receipt-refused", "{outcome}");
    let (_code, out, _) = world.run(&["status", &id, "--json"]);
    assert!(out.contains("in-custody"), "not settled cross-route: {out}");
}

#[test]
fn shared_bot_signed_identity_admits_forgery_is_stranger() {
    // One shared bot posts for every team: the sender ID cannot identify
    // the team, so authority is the envelope's signature.
    const SHARED: &str = "USHAREDBOT";
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{SHARED}=sim@host-a"));
    // The sender team has a key (keygen/announce creates it), so sends sign.
    let _ = agent_teams::relay::ensure_signing_key(&world.state, "sim", "host-a").expect("key");
    let (id, post) = world.send_sim_to_tools("signed hello", "relay-signed-1");
    assert!(post.contains("\"relay\""), "envelope is signed: {post}");
    // Unknown key yet: a stranger, quarantined and loud.
    let outcome = world.relay_in(SHARED, &post);
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    assert!(
        !world
            .hook_inbox_texts("tools")
            .iter()
            .any(|text| text.contains("signed hello")),
        "envelope not delivered (the inbox alarm is expected)"
    );
    // Announce carries the public key; owner confirmation trusts it and
    // the quarantined post drains exactly once.
    let (_, public) =
        agent_teams::relay::ensure_signing_key(&world.state, "sim", "host-a").expect("key");
    let mut announce = agent_teams::relay::build_announce(
        "sim",
        "host-a",
        SHARED,
        "https://example.invalid/org/repo.git",
    );
    announce["pubkey"] = serde_json::json!(public);
    announce["integrity"] = serde_json::json!(agent_teams::relay::announce_integrity(&announce));
    let outcome = world.relay_in(SHARED, &agent_teams::relay::render_announce(&announce));
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
    let (code, out, err) = world.run(&["relay", "confirm", SHARED, "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(world.ledger_has("tools", &id), "drained once");
    let outcome = world.relay_in(SHARED, &post);
    assert_eq!(outcome["outcome"], "known", "{outcome}");
    assert!(world.ledger_has("tools", &id), "exactly once");
    // A tampered body fails verification: stranger, never delivered.
    let mut forged: serde_json::Value = serde_json::from_str(
        post.split("```agent-teams-envelope\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap(),
    )
    .expect("envelope json");
    forged["body"] = serde_json::json!("forged body");
    let outcome = world.relay_in(SHARED, &agent_teams::relay::render_post(&forged));
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    // An unsigned post while a key is trusted is a stranger too.
    let mut unsigned = forged.clone();
    unsigned.as_object_mut().unwrap().remove("relay");
    unsigned["body"] = serde_json::json!("signed hello");
    // Rebuild a valid unsigned envelope through the post renderer is not
    // possible for this ID; assert quarantine grew instead via alarms file.
    let outcome = world.relay_in(SHARED, &agent_teams::relay::render_post(&unsigned));
    assert_eq!(outcome["outcome"], "stranger-quarantined", "{outcome}");
    assert!(world.ledger_has("tools", &id), "still exactly once");
    // Our own signed post loops back and is ignored, never an alarm.
    let mut own = agent_teams::envelope::build_envelope(
        "tm-tools-1212121234343434",
        "kown",
        "tools",
        "host-a",
        1,
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
        "our own post",
    );
    agent_teams::relay::sign_envelope(&world.state, &mut own).expect("sign own");
    let outcome = world.relay_in(SHARED, &agent_teams::relay::render_post(&own));
    assert_eq!(outcome["outcome"], "ignored-own-post", "{outcome}");
}

#[test]
fn slack_mangled_own_announce_is_ignored_not_stranger() {
    // Exact live shape: Slack strips the fence info string, leaving a bare
    // ``` fence around the announce JSON. Our own announce must loop back
    // as ignored, never as a stranger alarm.
    const SHARED: &str = "USHAREDBOT";
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{SHARED}=sim@host-a"));
    let (_, public) =
        agent_teams::relay::ensure_signing_key(&world.state, "tools", "host-a").expect("key");
    let mut announce = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        SHARED,
        "https://example.invalid/org/repo.git",
    );
    announce["pubkey"] = serde_json::json!(public);
    announce["integrity"] = serde_json::json!(agent_teams::relay::announce_integrity(&announce));
    let mangled =
        agent_teams::relay::render_announce(&announce).replace("```agent-teams-announce", "```");
    assert!(mangled.contains("\n```\n{"), "{mangled}");
    let outcome = world.relay_in(SHARED, &mangled);
    assert_eq!(outcome["outcome"], "ignored-own-announce", "{outcome}");
    assert!(
        !world
            .hook_inbox_texts("tools")
            .iter()
            .any(|text| text.contains("ESCALATE") || text.contains("stranger")),
        "own announce raised no stranger alarm: {:?}",
        world.hook_inbox_texts("tools")
    );
    // The same bare-fence mangling on a peer announce still records a
    // candidate (content classification, not the fence tag).
    let mut peer = agent_teams::relay::build_announce(
        "sim",
        "host-a",
        SHARED,
        "https://example.invalid/org/repo.git",
    );
    peer["pubkey"] = serde_json::json!("00".repeat(32));
    peer["integrity"] = serde_json::json!(agent_teams::relay::announce_integrity(&peer));
    let mangled_peer =
        agent_teams::relay::render_announce(&peer).replace("```agent-teams-announce", "```");
    let outcome = world.relay_in(SHARED, &mangled_peer);
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
}

#[test]
fn shared_bot_signed_both_ways_with_receipts() {
    // THE live configuration: two teams post through ONE bot ID. The
    // bridge's own-bot ID therefore proves nothing; relay-in must decide
    // from the signature. Signed peer posts sharing our bot ID are
    // delivered both ways, with receipts, and never dropped as
    // "ignored-own-bot" (the silent-loss bug this regresses).
    const SHARED: &str = "USHAREDBOT";
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{SHARED}=sim@host-a"));
    // Give sim a relay channel too (each host declares the shared bot
    // as the other team), and make the route's own sender the shared
    // bot, exactly as on the live bridges.
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
        "--relay",
        &format!("relay-coord:{CHAN}:{SHARED}=tools@host-a"),
    ]);
    assert_eq!(code, 0, "init sim relay: {out} {err}");
    let routes_path = world.config.join("agent-teams/routes.d/relay.json");
    let routes = std::fs::read_to_string(&routes_path)
        .expect("routes")
        .replace("UBOTTOOLS", SHARED);
    std::fs::write(&routes_path, routes).expect("routes rewrite");
    // Each team@host has its own key; each side trusts the other's
    // (what announce + confirm records on a real host).
    let (_, sim_public) =
        agent_teams::relay::ensure_signing_key(&world.state, "sim", "host-a").expect("sim key");
    let (_, tools_public) =
        agent_teams::relay::ensure_signing_key(&world.state, "tools", "host-a").expect("tools key");
    agent_teams::relay::trust_key(&world.state, "sim", "host-a", &sim_public, "test")
        .expect("trust sim");
    agent_teams::relay::trust_key(&world.state, "tools", "host-a", &tools_public, "test")
        .expect("trust tools");
    // sim -> tools through the shared bot: delivered, with a receipt.
    let (id, post) = world.send_sim_to_tools("signed hello both ways", "relay-bothways-1");
    let outcome = world.relay_in(SHARED, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    let receipt = outcome["receipt_text"]
        .as_str()
        .expect("receipt")
        .to_string();
    let outcome = world.relay_in(SHARED, &receipt);
    assert_eq!(outcome["outcome"], "receipt-recorded", "{outcome}");
    let (_code, out, _) = world.run(&["status", &id, "--json"]);
    let status: serde_json::Value = serde_json::from_str(&out).expect("status json");
    assert_ne!(
        status["outbox"]["state"], "in-custody",
        "settled by receipt: {out}"
    );
    // tools -> sim through the same bot: also delivered, with a receipt.
    let (code, out, err) = world.run(&[
        "send",
        "sim@host-a",
        "--from-team",
        "tools",
        "--text",
        "signed reply both ways",
        "--key",
        "relay-bothways-2",
        "--json",
    ]);
    assert_eq!(code, 75, "tools send settles in-custody: {out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    let id2 = value["id"].as_str().expect("id").to_string();
    let post = world
        .channel_posts()
        .into_iter()
        .last()
        .expect("tools relay post");
    let outcome = world.relay_in(SHARED, &post);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    assert!(world.ledger_has("sim", &id2), "delivered to sim ledger");
    let receipt = outcome["receipt_text"]
        .as_str()
        .expect("receipt")
        .to_string();
    let outcome = world.relay_in(SHARED, &receipt);
    assert_eq!(outcome["outcome"], "receipt-recorded", "{outcome}");
    // Our own signed post (from this host's receiving team, our key) is
    // still ignored, and a tampered signed post is a loud stranger.
    let mut own = agent_teams::envelope::build_envelope(
        "tm-tools-5656565634343434",
        "kown2",
        "tools",
        "host-a",
        1,
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
        "our own post",
    );
    agent_teams::relay::sign_envelope(&world.state, &mut own).expect("sign own");
    let outcome = world.relay_in(SHARED, &agent_teams::relay::render_post(&own));
    assert_eq!(outcome["outcome"], "ignored-own-post", "{outcome}");
}

#[test]
fn slack_one_line_tag_and_body_delivers() {
    // Exact live shape: Slack stores the header tag and the body on ONE
    // line (the newline after the bracketed tag became a space, middle
    // dots intact). The tag cross-check compares only the bracketed
    // prefix, so this delivers; a bracket that really disagrees is
    // filed as unparseable, not delivered.
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let (id, post) = world.send_sim_to_tools("relay live test one-line", "relay-oneline-1");
    let one_line = post.replacen("]\n", "] ", 1);
    assert!(
        !one_line.lines().next().unwrap().ends_with(']'),
        "{one_line}"
    );
    let outcome = world.relay_in(BOT1, &one_line);
    assert_eq!(outcome["outcome"], "accepted", "{outcome}");
    assert!(world.ledger_has("tools", &id), "delivered");
    // Bracket really disagrees (wrong id in the tag only; the envelope
    // JSON is untouched): not delivered. Legacy admission has no
    // signature to trust, so it is filed under the unparseable kind,
    // never as a stranger (#763 review fix 2).
    let (id2, post2) = world.send_sim_to_tools("relay live test tampered", "relay-oneline-2");
    let one_line2 = post2.replacen("]\n", "] ", 1);
    let bracket_end = one_line2.find(']').unwrap();
    let tampered = format!(
        "{}{}",
        one_line2[..bracket_end].replace(&id2, "tm-sim-0000000000000000"),
        &one_line2[bracket_end..]
    );
    let outcome = world.relay_in(BOT1, &tampered);
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    assert!(
        !world.ledger_has("tools", &id2),
        "tampered post not delivered"
    );
    assert!(
        agent_teams::relay::alarms(&world.state)
            .iter()
            .any(|a| a["kind"] == "unparseable"),
        "filed as unparseable: {:?}",
        agent_teams::relay::alarms(&world.state)
    );
}

#[test]
fn quarantine_retry_recovers_prefix_posts_and_drop_discards() {
    // Posts quarantined by the pre-fix parser (bare-fence announces that
    // read as "no agent-teams fenced block") sit in quarantine with their
    // raw text stored. `relay quarantine retry` replays them through the
    // current parser; `drop` discards without delivering.
    const SHARED: &str = "USHAREDBOT";
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{SHARED}=sim@host-a"));
    let (_, public) =
        agent_teams::relay::ensure_signing_key(&world.state, "tools", "host-a").expect("key");
    let mut own = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        SHARED,
        "https://example.invalid/org/repo.git",
    );
    own["pubkey"] = serde_json::json!(public);
    own["integrity"] = serde_json::json!(agent_teams::relay::announce_integrity(&own));
    let own_post =
        agent_teams::relay::render_announce(&own).replace("```agent-teams-announce", "```");
    let mut peer = agent_teams::relay::build_announce(
        "sim",
        "host-a",
        SHARED,
        "https://example.invalid/org/repo.git",
    );
    peer["pubkey"] = serde_json::json!("00".repeat(32));
    peer["integrity"] = serde_json::json!(agent_teams::relay::announce_integrity(&peer));
    let peer_post =
        agent_teams::relay::render_announce(&peer).replace("```agent-teams-announce", "```");
    // Seed quarantine exactly as the pre-fix parser recorded it: a stub
    // envelope plus the raw post text.
    let relay_dir = world.state.join("relay");
    std::fs::create_dir_all(&relay_dir).expect("relay dir");
    let seed = serde_json::json!({"schema": "agent-teams-relay/v1", "posts": [
        {"id": "unparseable-own1", "peer_sender": SHARED, "route": "relay-coord",
         "channel": CHAN, "envelope": {"id": "unparseable-own1", "unparseable": true},
         "post": own_post, "reason": "unparseable relay post: no agent-teams fenced block",
         "quarantined_at": "2026-11-01T00:00:00Z"},
        {"id": "unparseable-peer1", "peer_sender": SHARED, "route": "relay-coord",
         "channel": CHAN, "envelope": {"id": "unparseable-peer1", "unparseable": true},
         "post": peer_post, "reason": "unparseable relay post: no agent-teams fenced block",
         "quarantined_at": "2026-11-01T00:00:00Z"},
    ]});
    std::fs::write(
        relay_dir.join("quarantine.json"),
        serde_json::to_string(&seed).unwrap(),
    )
    .expect("seed quarantine");
    let (code, out, err) = world.run(&["relay", "quarantine", "retry"]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(
        out.contains("unparseable-own1: ignored-own-announce"),
        "{out}"
    );
    assert!(
        out.contains("unparseable-peer1: candidate-recorded"),
        "{out}"
    );
    assert!(
        agent_teams::relay::quarantined_posts(&world.state).is_empty(),
        "both recovered posts leave quarantine"
    );
    // A post that still does not parse stays quarantined after retry,
    // and drop removes it without delivery.
    let outcome = world.relay_in(SHARED, "definitely not a relay post");
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    let stuck = agent_teams::relay::quarantined_posts(&world.state);
    assert_eq!(stuck.len(), 1, "{stuck:?}");
    let stuck_id = stuck[0]["id"].as_str().expect("stuck id").to_string();
    let (code, out, err) = world.run(&["relay", "quarantine", "retry", &stuck_id]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("not-recorded"), "{out}");
    assert_eq!(agent_teams::relay::quarantined_posts(&world.state).len(), 1);
    let (code, out, err) = world.run(&["relay", "quarantine", "drop", &stuck_id]);
    assert_eq!(code, 0, "{out} {err}");
    assert!(out.contains("dropped"), "{out}");
    assert!(agent_teams::relay::quarantined_posts(&world.state).is_empty());
    let (code, out, _) = world.run(&["relay", "quarantine", "list"]);
    assert_eq!(code, 0);
    assert!(out.contains("empty"), "{out}");
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
    assert!(out.contains("#agent-teams-relay"), "{out}");
    assert!(out.contains("relay_channels"), "{out}");
    assert!(out.contains("Never admit an unconfirmed claim"), "{out}");
    assert!(!world.channel_file.exists(), "setup writes nothing");
}

/// #721 agent-teams-coordinator-handover over the relay: a receiving team whose delivery
/// target is archived answers with a `deferred` receipt, and the sender records it as
/// `deferred` (with the reason), so `pump` retries it instead of leaving it in custody.
#[test]
fn relay_undeliverable_target_answers_a_deferred_receipt_the_sender_records() {
    let world = World::new();
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let (id, post) = world.send_sim_to_tools("Are you there?", "relay-undeliverable-1");
    // Then the receiving team is pointed at a registry whose designated coordinator is archived.
    let registry = world.root.join("registry");
    let archived = registry
        .join("archive")
        .join("old-1792931619150276422-1347278");
    std::fs::create_dir_all(&archived).expect("archive");
    std::fs::write(archived.join("agent.json"), r#"{"name": "old"}"#).expect("archived record");
    std::fs::write(registry.join("coordinator.json"), r#"{"name": "old"}"#).expect("designation");
    let path = world.state.join("teams.d/tools/declaration.json");
    let mut declaration: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("declaration")).expect("json");
    declaration["deliverer"] = serde_json::json!({"kind": "agentctl-queue", "registry": registry.to_string_lossy(), "auto_drain": false});
    declaration["coordinator"] = serde_json::json!({"designation": "registry"});
    std::fs::write(&path, serde_json::to_string_pretty(&declaration).unwrap()).expect("write");

    let outcome = world.relay_in(BOT1, &post);
    assert_eq!(outcome["outcome"], "undeliverable", "{outcome}");
    let receipt = outcome["receipt_text"].as_str().expect("receipt text");
    assert!(receipt.contains("receipt: deferred"), "{receipt}");
    assert!(world
        .state
        .join(format!("teams.d/tools/notices/{id}.json"))
        .exists());

    let tools_announce = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        "UBOTTOOLS",
        "https://example.invalid/org/repo.git",
    );
    world.relay_in(
        "UBOTTOOLS",
        &agent_teams::relay::render_announce(&tools_announce),
    );
    let (code, out, err) = world.run(&["relay", "confirm", "UBOTTOOLS", "--owner-approved"]);
    assert_eq!(code, 0, "{out} {err}");
    let outcome = world.relay_in("UBOTTOOLS", receipt);
    assert_eq!(outcome["outcome"], "receipt-recorded", "{outcome}");
    let (_, out, _) = world.run(&["status", &id, "--json"]);
    let value: serde_json::Value = serde_json::from_str(&out).expect("status json");
    assert_eq!(value["outbox"]["state"], "deferred", "{out}");
    assert!(
        value["outbox"]["undeliverable"]
            .as_str()
            .is_some_and(|reason| reason.contains("archived")),
        "{out}"
    );
}

// --- #782 agent-teams-relay-mirror / #770 agent-teams-relay-fallback-route ---

impl World {
    fn write_teams(&self, teams: &str) {
        std::fs::write(self.config.join("agent-teams/teams.json"), teams).expect("teams rewrite");
    }
    fn write_relay_route(&self, mirror: bool, post_program: &str) {
        let list = self.root.join("bin/list.sh");
        let routes = format!(
            r#"{{"schema": "agent-teams-routes/v1", "routes": {{"relay-coord": {{"driver": "command", "mode": "relay", "channel": "{CHAN}", "sender_id": "UBOTTOOLS", "mirror": {mirror}, "post_argv": ["{post_program}", "post", "--channel", "{{channel}}", "--text-file", "{{text_file}}"], "list_argv": ["{}", "list", "--channel", "{{channel}}"], "max_envelope_bytes": 10000, "limits": {{"per_hour": 100}}}}}}}}"#,
            list.display()
        );
        std::fs::write(self.config.join("agent-teams/routes.d/relay.json"), routes)
            .expect("routes rewrite");
    }
    fn ensure_signing_key(&self, team: &str) -> String {
        agent_teams::relay::ensure_signing_key(&self.state, team, "host-a")
            .expect("signing key")
            .1
    }
    fn outbox(&self, id: &str) -> serde_json::Value {
        let text = std::fs::read_to_string(self.state.join("outbox").join(format!("{id}.json")))
            .expect("outbox entry");
        serde_json::from_str(&text).expect("outbox json")
    }
    fn mirror_attempts(&self, id: &str) -> Vec<serde_json::Value> {
        self.outbox(id)["attempts"]
            .as_array()
            .expect("attempts")
            .iter()
            .filter(|a| a["verb"] == "mirror")
            .cloned()
            .collect()
    }
    fn copies_of(&self, team: &str, id: &str) -> usize {
        let dir = self.state.join("teams.d").join(team).join("hook-inbox");
        [dir.clone(), dir.join("claimed")]
            .iter()
            .filter(|d| d.join(format!("{id}.json")).exists())
            .count()
    }
}

const MIRROR_TEAMS_LOCAL: &str = r#"{
  "schema": "agent-teams-teams/v1",
  "hosts": {"host-a": {"routes": ["local"], "agent_teams": "agent-teams"}},
  "teams": {
    "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
    "plain": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
  }
}
"#;

#[test]
fn mirror_after_direct_delivery_posts_copy_and_receiver_dedups_quietly() {
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let public = world.ensure_signing_key("sim");
    agent_teams::relay::trust_key(&world.state, "sim", "host-a", &public, "test").expect("trust");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "Mirror me please.",
        "--key",
        "mirror-1",
        "--json",
    ]);
    assert_eq!(code, 75, "direct accepted: {out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "accepted", "{out}");
    let id = value["id"].as_str().expect("id").to_string();
    // One direct delivery, plus exactly one mirror post of the same envelope.
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);
    let posts = world.channel_posts();
    assert_eq!(posts.len(), 1, "one mirror post: {posts:?}");
    assert!(posts[0].contains("Mirror me please."), "{}", posts[0]);
    assert!(posts[0].contains(&id), "same ID: {}", posts[0]);
    let attempts = world.mirror_attempts(&id);
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(
        attempts[0]["classification"], "mirror-posted",
        "{attempts:?}"
    );
    // Deliver locally (hook twice: presented, then delivered).
    let _ = world.run(&["hook", "--team", "tools"]);
    let _ = world.run(&["hook", "--team", "tools"]);
    let (_, out, _) = world.run(&["seen", &id]);
    assert!(out.contains("delivered"), "ledger delivered: {out}");
    // The mirror copy now arrives through relay-in: exactly one delivery,
    // no quarantine or stranger alarm, and a quiet delivered receipt.
    let outcome = world.relay_in(BOT1, &posts[0]);
    assert_eq!(outcome["outcome"], "known", "{outcome}");
    let receipt = outcome["receipt_text"].as_str().expect("receipt");
    assert!(receipt.contains("receipt: delivered"), "{receipt}");
    assert!(receipt.contains("delivered already"), "{receipt}");
    assert_eq!(world.copies_of("tools", &id), 1, "exactly once");
    assert!(agent_teams::relay::quarantined_posts(&world.state).is_empty());
    let (_, out, _) = world.run(&["relay", "alarms"]);
    assert!(out.contains("no open stranger alarms"), "{out}");

    // Replies are mirrored like sends (#782): give sim a relay channel,
    // then tools' reply to sim is delivered directly and mirrored too.
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
        "--relay",
        &format!("relay-coord:{CHAN}:{BOT1}=tools@host-a"),
    ]);
    assert_eq!(code, 0, "init sim relay: {out} {err}");
    world.ensure_signing_key("tools");
    let (code, out, err) = world.run(&[
        "reply",
        &id,
        "--from-team",
        "tools",
        "--text",
        "Reply mirrored too.",
    ]);
    assert!(code == 75 || code == 0, "reply: {code} {out} {err}");
    assert_eq!(
        world.hook_inbox_texts("sim").len(),
        1,
        "reply delivered: {out}"
    );
    assert_eq!(
        world.channel_posts().len(),
        2,
        "reply mirrored: {:?}",
        world.channel_posts()
    );
    assert!(world.channel_posts()[1].contains("Reply mirrored too."));
}

#[test]
fn mirror_skips_private_and_teams_without_relay_channels() {
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    world.ensure_signing_key("tools");
    let pid = std::process::id().to_string();
    let (code, out, err) = world.run(&[
        "init",
        "plain",
        "--coordinator-self",
        "--coordinator-pid",
        &pid,
        "--deliver",
        "hook-inbox",
        "--accepts-from",
        "tools",
    ]);
    assert_eq!(code, 0, "init plain: {out} {err}");
    // --private never mirrors (the relay exposes the body).
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "secret mirror",
        "--key",
        "mirror-private",
        "--private",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);
    assert!(world.channel_posts().is_empty(), "no mirror for --private");
    // plain declares no relay_channels for relay-coord: no mirror to it.
    let (code, out, err) = world.run(&[
        "send",
        "plain@host-a",
        "--from-team",
        "tools",
        "--text",
        "no relay team",
        "--key",
        "mirror-norelay",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.hook_inbox_texts("plain").len(), 1);
    assert!(
        world.channel_posts().is_empty(),
        "no mirror without relay_channels"
    );
}

#[test]
fn mirror_failure_never_changes_delivery_and_pump_never_retries_it() {
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    world.write_relay_route(true, "/nonexistent/relay-post");
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "Mirror will fail.",
        "--key",
        "mirror-fail",
        "--json",
    ]);
    assert_eq!(code, 75, "state unchanged by mirror failure: {out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "accepted", "{out}");
    let id = value["id"].as_str().expect("id").to_string();
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);
    let attempts = world.mirror_attempts(&id);
    assert_eq!(attempts.len(), 1, "{attempts:?}");
    assert_eq!(
        attempts[0]["classification"], "mirror-failed",
        "{attempts:?}"
    );
    // pump re-walks the accepted message but never retries the mirror.
    let _ = world.run(&["pump"]);
    assert_eq!(
        world.mirror_attempts(&id).len(),
        1,
        "mirror not retried by pump"
    );
    assert_eq!(world.outbox(&id)["state"], "accepted");
}

#[test]
fn relay_fallback_only_for_the_relay_enabled_team() {
    // Host-wide relay listing (the #770 hazard): the relay is kept only
    // for a team whose declaration names relay_channels for it.
    let world = World::new();
    world.write_teams(r#"{
      "schema": "agent-teams-teams/v1",
      "hosts": {"host-a": {"routes": ["relay-coord"], "agent_teams": "agent-teams"}},
      "teams": {
        "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "plain": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
      }
    }"#);
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let pid = std::process::id().to_string();
    let (code, out, err) = world.run(&[
        "init",
        "plain",
        "--coordinator-self",
        "--coordinator-pid",
        &pid,
        "--deliver",
        "hook-inbox",
        "--accepts-from",
        "tools",
    ]);
    assert_eq!(code, 0, "init plain: {out} {err}");
    // tools is relay-enabled: the relay is its fallback and carries it.
    let (id, _post) = world.send_sim_to_tools("fallback carries tools", "fallback-1");
    assert!(!world.channel_posts().is_empty());
    let _ = id;
    // plain is not relay-enabled: no relay route, no post; kept deferred.
    let (code, out, err) = world.run(&[
        "send",
        "plain@host-a",
        "--from-team",
        "tools",
        "--text",
        "never over relay",
        "--key",
        "fallback-2",
        "--json",
    ]);
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(
        value["state"], "deferred",
        "no route for plain: {out} {err}"
    );
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.channel_posts().len(), 1, "only the tools post");
    assert!(world.hook_inbox_texts("plain").is_empty());
}

#[test]
fn sender_that_is_not_relay_enabled_never_mirrors() {
    // Finding 1: the DESTINATION being relay-enabled is not enough; a
    // sender with no relay_channels for the route must not post bodies.
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    // Strip sim's relay_channels: it can sign, but never opted into relay.
    let decl_path = world.state.join("teams.d/sim/declaration.json");
    let mut decl: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&decl_path).expect("decl"))
            .expect("decl json");
    decl["relay_channels"] = serde_json::json!([]);
    std::fs::write(&decl_path, serde_json::to_string(&decl).expect("decl")).expect("decl write");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "sender not on relay",
        "--key",
        "mirror-sender-norelay",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);
    assert!(
        world.channel_posts().is_empty(),
        "no mirror from a non-relay sender"
    );
}

#[test]
fn relay_carried_message_is_not_mirrored() {
    let world = World::new();
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    let (id, _post) = world.send_sim_to_tools("carried by relay", "relay-carried-1");
    assert_eq!(world.channel_posts().len(), 1, "only the carrying post");
    assert!(world.mirror_attempts(&id).is_empty(), "no mirror attempts");
}

#[test]
fn mirror_posts_spend_the_mirror_limit_never_the_delivery_limit() {
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    // Cap mirror posts at one an hour; deliveries keep their own limit.
    let routes_path = world.config.join("agent-teams/routes.d/relay.json");
    let mut routes: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&routes_path).expect("routes"))
            .expect("routes json");
    routes["routes"]["relay-coord"]["limits"]["mirror_per_hour"] = serde_json::json!(1);
    std::fs::write(
        &routes_path,
        serde_json::to_string(&routes).expect("routes"),
    )
    .expect("routes write");
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    for key in ["mirror-limit-1", "mirror-limit-2"] {
        let (code, out, err) = world.run(&[
            "send",
            "tools@host-a",
            "--from-team",
            "sim",
            "--text",
            "limited mirror",
            "--key",
            key,
            "--json",
        ]);
        assert_eq!(code, 75, "{out} {err}");
        let _ = key;
    }
    assert_eq!(
        world.hook_inbox_texts("tools").len(),
        2,
        "both delivered directly"
    );
    assert_eq!(
        world.channel_posts().len(),
        1,
        "second mirror skipped by its limit"
    );
    let ids: Vec<String> = {
        let mut v = Vec::new();
        for entry in std::fs::read_dir(world.state.join("outbox"))
            .expect("outbox")
            .flatten()
        {
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(entry.path()).expect("read"))
                    .expect("json");
            v.push(value["id"].as_str().unwrap().to_string());
        }
        v.sort();
        v.dedup();
        v
    };
    let skipped = ids
        .iter()
        .filter(|id| {
            world
                .mirror_attempts(id)
                .iter()
                .any(|a| a["classification"] == "mirror-skipped")
        })
        .count();
    assert_eq!(
        skipped,
        1,
        "one mirror-skipped attempt: {:?}",
        ids.iter()
            .map(|id| (id.clone(), world.mirror_attempts(id)))
            .collect::<Vec<_>>()
    );
    // The delivery rate file was never spent by a mirror.
    let delivery_log =
        std::fs::read_to_string(world.state.join("relay-relay-coord.log")).unwrap_or_default();
    assert!(
        delivery_log.trim().is_empty(),
        "delivery limit untouched: {delivery_log}"
    );
    let mirror_log = std::fs::read_to_string(world.state.join("relay-relay-coord-mirror.log"))
        .unwrap_or_default();
    assert_eq!(mirror_log.lines().count(), 1, "mirror limit spent once");
}

#[test]
fn failed_mirror_post_does_not_poison_the_fallback() {
    // A post helper that fails once, then works: the mirror fails, and a
    // later pump over the relay must post for real, not answer ambiguous.
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let marker = world.root.join("flaky-marker");
    let flaky = world.root.join("bin/flaky.sh");
    std::fs::write(
        &flaky,
        format!(
            "#!/bin/sh\nif [ ! -f {} ]; then touch {}; exit 1; fi\nexec {} \"$@\"\n",
            marker.display(),
            marker.display(),
            world.root.join("bin/post.sh").display()
        ),
    )
    .expect("flaky");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&flaky, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    world.write_relay_route(true, &flaky.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "mirror fails once",
        "--key",
        "mirror-flaky",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "accepted", "{out}");
    let id = value["id"].as_str().expect("id").to_string();
    assert_eq!(
        world.mirror_attempts(&id)[0]["classification"],
        "mirror-failed"
    );
    // Direct routes go away; pump with only the relay left.
    world.write_teams(r#"{
      "schema": "agent-teams-teams/v1",
      "hosts": {"host-a": {"routes": ["relay-coord"], "agent_teams": "agent-teams"}},
      "teams": {
        "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "plain": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
      }
    }"#);
    let _ = world.run(&["pump"]);
    let state = world.outbox(&id)["state"].as_str().unwrap().to_string();
    assert_eq!(
        state,
        "in-custody",
        "fallback posted, not ambiguous: {}",
        world.outbox(&id)
    );
    assert_eq!(world.channel_posts().len(), 1, "the fallback post");
}

#[test]
fn hung_mirror_post_times_out_and_delivery_stands() {
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let hang = world.root.join("bin/hang.sh");
    let survived = world.root.join("grandchild-posted");
    std::fs::write(
        &hang,
        format!(
            "#!/bin/sh\n(sleep 12; touch {}) &\nsleep 60\n",
            survived.display()
        ),
    )
    .expect("hang");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hang, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    world.write_relay_route(true, &hang.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    let started = std::time::Instant::now();
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "mirror hangs",
        "--key",
        "mirror-hang",
        "--json",
    ]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(45),
        "send returned despite the hang"
    );
    assert_eq!(code, 75, "{out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "accepted", "delivery stands: {out}");
    let id = value["id"].as_str().expect("id").to_string();
    let attempts = world.mirror_attempts(&id);
    assert_eq!(
        attempts[0]["classification"], "mirror-failed",
        "{attempts:?}"
    );
    assert!(
        attempts[0]["detail"]
            .as_str()
            .unwrap()
            .contains("timed out"),
        "{attempts:?}"
    );
    // The timeout kills the whole process group: the grandchild that
    // would have posted at 12 s must not survive to do it. Wait ~13 s
    // (past the grandchild's firing time, from the send's start) so a
    // survivor would have fired by now.
    std::thread::sleep(std::time::Duration::from_secs(13));
    assert!(!survived.exists(), "grandchild survived the timeout kill");
}

#[test]
fn relay_then_direct_is_not_mirrored() {
    let world = World::new();
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    let (id, _post) = world.send_sim_to_tools("relay first", "relay-then-direct-1");
    assert_eq!(world.channel_posts().len(), 1);
    // A direct route appears; age the custody so pump re-walks it there.
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let outbox_path = world.state.join("outbox").join(format!("{id}.json"));
    let mut entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&outbox_path).expect("outbox"))
            .expect("json");
    for attempt in entry["attempts"].as_array_mut().expect("attempts") {
        attempt["at"] = serde_json::json!("2020-01-01T00:00:00.000Z");
    }
    std::fs::write(&outbox_path, serde_json::to_string(&entry).expect("json")).expect("write");
    let _ = world.run(&["pump"]);
    assert_eq!(
        world.channel_posts().len(),
        1,
        "no mirror after relay carriage"
    );
    assert!(world.mirror_attempts(&id).is_empty(), "no mirror attempts");
}

#[test]
fn explicit_entry_routes_opt_into_relay() {
    // The destination names no relay_channels, but its entry's own list
    // names the relay: that list is its explicit opt-in (#770).
    let world = World::new();
    world.write_teams(r#"{
      "schema": "agent-teams-teams/v1",
      "hosts": {"host-a": {"routes": ["local"], "agent_teams": "agent-teams"}},
      "teams": {
        "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "plain": {"primary": "host-a", "entries": [{"host": "host-a", "routes": ["relay-coord"], "contacts": [{"kind": "host"}]}]}
      }
    }"#);
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let pid = std::process::id().to_string();
    let (code, out, err) = world.run(&[
        "init",
        "plain",
        "--coordinator-self",
        "--coordinator-pid",
        &pid,
        "--deliver",
        "hook-inbox",
        "--accepts-from",
        "sim",
    ]);
    assert_eq!(code, 0, "init plain: {out} {err}");
    let (code, out, err) = world.run(&[
        "send",
        "plain@host-a",
        "--from-team",
        "sim",
        "--text",
        "explicit relay",
        "--key",
        "explicit-relay-1",
        "--json",
    ]);
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(
        value["state"], "in-custody",
        "explicit list carries it: {out} {err}"
    );
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.channel_posts().len(), 1);
}

#[test]
fn refused_message_is_not_mirrored() {
    // A direct route answering `refused` (here: the receiver's
    // per-sender rate limit) settled nothing deliverable: no mirror.
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    world.ensure_signing_key("sim");
    // The receiver admits one message an hour from each sender.
    let decl_path = world.state.join("teams.d/tools/declaration.json");
    let mut decl: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&decl_path).expect("decl"))
            .expect("decl json");
    decl["limits"]["per_sender_per_hour"] = serde_json::json!(1);
    std::fs::write(&decl_path, serde_json::to_string(&decl).expect("decl")).expect("decl write");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "first, delivered",
        "--key",
        "mirror-refused-1",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.channel_posts().len(), 1, "first message mirrored");
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        "second, refused",
        "--key",
        "mirror-refused-2",
        "--json",
    ]);
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "refused", "{out} {err}");
    assert_eq!(code, 65, "{out} {err}");
    assert_eq!(world.channel_posts().len(), 1, "no mirror for a refusal");
}
#[test]
fn long_mirror_post_is_never_split_and_dismiss_clears_unparseable() {
    // #763: a long body used to make a relay post the outbound helper
    // split at 4,000 bytes after conversion; the first chunk had no fence
    // and alarmed as a stranger. This body fits in one envelope (~2.9 KB)
    // but its visible copy must be truncated to stay under the cap.
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let public = world.ensure_signing_key("sim");
    agent_teams::relay::trust_key(&world.state, "sim", "host-a", &public, "test").expect("trust");
    let body = "x".repeat(2400);
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        &body,
        "--key",
        "mirror-long-1",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    let posts = world.channel_posts();
    assert_eq!(posts.len(), 1, "one unsplit post: {} posts", posts.len());
    assert!(
        posts[0].len() <= agent_teams::relay::DEFAULT_MAX_POST_BYTES,
        "post is {} bytes",
        posts[0].len()
    );
    assert!(posts[0].contains("truncated"), "visible copy truncated");
    // The whole envelope survives in the single post and delivers once.
    let outcome = world.relay_in(BOT1, &posts[0]);
    assert!(
        matches!(outcome["outcome"].as_str(), Some("accepted" | "known")),
        "{outcome}"
    );
    assert_eq!(world.hook_inbox_texts("tools").len(), 1);
    // The fence-only second piece a split would have produced: the signed
    // envelope is the authority, so it must not raise a stranger alarm.
    let fence_at = posts[0].find("```").expect("fence");
    let outcome = world.relay_in(BOT1, &posts[0][fence_at..]);
    assert!(
        matches!(outcome["outcome"].as_str(), Some("accepted" | "known")),
        "fence-only piece: {outcome}"
    );
    assert_eq!(
        world.hook_inbox_texts("tools").len(),
        1,
        "still delivered once"
    );
    assert!(
        agent_teams::relay::alarms(&world.state).is_empty(),
        "no alarm for the fence-only piece: {:?}",
        agent_teams::relay::alarms(&world.state)
    );
    // A present-but-wrong tag on a signed envelope is NOT bypassed: the
    // visible header would disagree with what was delivered.
    let wrong_tag = format!(
        "[from someone-else via agent-teams]\n{}",
        &posts[0][fence_at..]
    );
    let outcome = world.relay_in(BOT1, &wrong_tag);
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    assert_eq!(
        world
            .hook_inbox_texts("tools")
            .iter()
            .filter(|t| t.contains(&"x".repeat(100)))
            .count(),
        1,
        "message not delivered again (an alarm notice is fine)"
    );
    // Remove that quarantine entry so the counts below stay simple.
    let wrong_id = agent_teams::relay::quarantined_posts(&world.state)
        .last()
        .expect("quarantined")
        .get("id")
        .and_then(|v| v.as_str())
        .expect("id")
        .to_string();
    let (code, out, err) = world.run(&["relay", "quarantine", "dismiss", &wrong_id]);
    assert_eq!(code, 0, "dismiss wrong-tag: {out} {err}");

    // An unparseable post from the CONFIRMED bot is not a stranger: it is
    // quarantined under its own alarm kind and dismissed by ID.
    let announce = agent_teams::relay::build_announce(
        "tools",
        "host-a",
        BOT1,
        "https://example.invalid/org/repo.git",
    );
    let outcome = world.relay_in(BOT1, &agent_teams::relay::render_announce(&announce));
    assert_eq!(outcome["outcome"], "candidate-recorded", "{outcome}");
    let (code, out, err) = world.run(&["relay", "confirm", BOT1, "--owner-approved"]);
    assert_eq!(code, 0, "confirm: {out} {err}");
    let outcome = world.relay_in(BOT1, "visible text with no fence at all");
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    assert!(
        outcome["detail"].as_str().unwrap().contains("dismiss"),
        "{outcome}"
    );
    // Never reply to an unparseable post: its plain-text receipt is itself
    // unparseable and would ping-pong between the two sides.
    assert!(outcome["receipt_text"].is_null(), "{outcome}");
    // The same post re-offered (bridge rescan) is recorded at most once.
    let outcome = world.relay_in(BOT1, "visible text with no fence at all");
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    assert_eq!(agent_teams::relay::quarantined_posts(&world.state).len(), 1);
    // A second distinct broken post joins the same alarm, which names both.
    let outcome = world.relay_in(BOT1, "a second broken post, still no fence");
    assert_eq!(outcome["outcome"], "not-recorded", "{outcome}");
    assert_eq!(agent_teams::relay::quarantined_posts(&world.state).len(), 2);
    let alarms = agent_teams::relay::alarms(&world.state);
    let alarm = alarms
        .iter()
        .find(|a| a["kind"] == "unparseable" && a["state"] == "open")
        .expect("unparseable alarm");
    assert_eq!(alarm["sender_id"], BOT1, "{alarm}");
    let ids: Vec<String> = alarm["quarantine_ids"]
        .as_array()
        .expect("quarantine_ids")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert_eq!(ids.len(), 2, "{alarm}");
    // The exact unparseable-receipt line is ignored, never quarantined,
    // even with the bridge's rescan note appended (the live shape).
    let before = agent_teams::relay::quarantined_posts(&world.state).len();
    for text in [
        "agent-teams receipt: not-recorded (unparseable relay post)",
        "agent-teams receipt: not-recorded (unparseable relay post)\n\n[agentctl] This message reached the bridge through the periodic rescan (the live stream missed it or is still catching up).",
    ] {
        let outcome = world.relay_in(BOT1, text);
        assert_eq!(outcome["outcome"], "ignored-unparseable-receipt", "{outcome}");
    }
    assert_eq!(
        agent_teams::relay::quarantined_posts(&world.state).len(),
        before
    );
    // Dismissing one of two posts keeps the alarm open and naming the rest.
    let (code, out, err) = world.run(&["relay", "quarantine", "dismiss", &ids[1]]);
    assert_eq!(code, 0, "dismiss: {out} {err}");
    let alarms = agent_teams::relay::alarms(&world.state);
    let alarm = alarms
        .iter()
        .find(|a| a["kind"] == "unparseable" && a["state"] == "open")
        .expect("alarm still open");
    assert_eq!(
        alarm["quarantine_ids"].as_array().map(Vec::len),
        Some(1),
        "{alarm}"
    );
    // Dismissing the last one closes it: it must not re-raise forever.
    let (code, out, err) = world.run(&["relay", "quarantine", "dismiss", &ids[0]]);
    assert_eq!(code, 0, "dismiss: {out} {err}");
    assert!(
        agent_teams::relay::alarms(&world.state)
            .iter()
            .all(|a| a["kind"] != "unparseable" || a["state"] != "open"),
        "no open unparseable alarm remains"
    );
}

#[test]
fn envelope_alone_over_post_cap_skips_mirror_not_delivery() {
    // #763 review fix 1: an envelope whose fence alone exceeds the route's
    // max_post_bytes skips the mirror (mirror-skipped); the direct
    // delivery itself is unaffected.
    let world = World::new();
    world.write_teams(MIRROR_TEAMS_LOCAL);
    let post = world.root.join("bin/post.sh");
    world.write_relay_route(true, &post.to_string_lossy());
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let public = world.ensure_signing_key("sim");
    agent_teams::relay::trust_key(&world.state, "sim", "host-a", &public, "test").expect("trust");
    let body = "x".repeat(5600);
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        &body,
        "--key",
        "mirror-oversize-1",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    assert_eq!(world.channel_posts().len(), 0, "no oversize mirror post");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    let id = value["id"].as_str().expect("id").to_string();
    let attempts = world.mirror_attempts(&id);
    assert!(
        attempts
            .iter()
            .any(|a| a["classification"] == "mirror-skipped"),
        "{attempts:?}"
    );
}

#[test]
fn oversize_fallback_stays_deferred_with_a_visible_reason() {
    // #763 review fix 7: an envelope too large for the only (relay) route
    // is never posted; it stays deferred with a visible outbox reason.
    let world = World::new();
    world.write_teams(r#"{
      "schema": "agent-teams-teams/v1",
      "hosts": {"host-a": {"routes": ["relay-coord"], "agent_teams": "agent-teams"}},
      "teams": {
        "tools": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]},
        "sim": {"primary": "host-a", "entries": [{"host": "host-a", "contacts": [{"kind": "host"}]}]}
      }
    }"#);
    world.init_tools(&format!("relay-coord:{CHAN}:{BOT1}=sim@host-a"));
    let body = "x".repeat(5600);
    let (code, out, err) = world.run(&[
        "send",
        "tools@host-a",
        "--from-team",
        "sim",
        "--text",
        &body,
        "--key",
        "fallback-oversize-1",
        "--json",
    ]);
    assert_eq!(code, 75, "{out} {err}");
    let value: serde_json::Value = serde_json::from_str(&out).expect("send json");
    assert_eq!(value["state"], "deferred", "{out}");
    assert_eq!(world.channel_posts().len(), 0, "never posted");
    let id = value["id"].as_str().expect("id").to_string();
    let entry = world.outbox(&id);
    let reason = entry["undeliverable"].as_str().expect("visible reason");
    assert!(
        reason.contains("too large for relay route relay-coord"),
        "{reason}"
    );
}
