//! A watch pass over a large registry stays fast and bounded (#571 watch-agents-scale).
//!
//! These drive the production probe through the real [`HerdrClient`] against a Herdr stand-in
//! script, so every Herdr call is a real process, as it is in production, and the script logs
//! each one for the call-count checks.

use super::*;
use crate::client::HerdrClient;
use std::os::unix::fs::PermissionsExt;

/// Herdr stand-in that answers from files beside it and logs every call. A recent read waits
/// for the seconds in `recent-delay`, as a loaded server reading a busy pane's scrollback does.
const FAKE_HERDR: &str = r#"#!/bin/sh
dir=$(cd "$(dirname "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/calls.log"
case "$1 $2" in
  "pane list") cat "$dir/pane-list.json" ;;
  "agent list") cat "$dir/agent-list.json" ;;
  "pane get")
    if [ -f "$dir/panes/$3.json" ]; then cat "$dir/panes/$3.json"
    else echo "pane $3 not found" >&2; exit 1; fi ;;
  "agent get")
    if [ -f "$dir/agents/$3.json" ]; then cat "$dir/agents/$3.json"
    else echo "agent $3 not found" >&2; exit 1; fi ;;
  "workspace get")
    printf '{"result":{"workspace":{"workspace_id":"%s","label":"team"}}}\n' "$3" ;;
  "pane read")
    if [ "$5" = recent-unwrapped ] || [ "$5" = recent ]; then sleep "$(cat "$dir/recent-delay")"; fi
    cat "$dir/screens/$3.txt" ;;
  *) echo "fake herdr: unsupported $*" >&2; exit 2 ;;
esac
"#;

/// One registered agent of the fixture.
struct Member {
    name: String,
    lifecycle: &'static str,
    /// Herdr's agent status; `None` when its pane is gone.
    status: Option<&'static str>,
    screen: String,
}

struct Team {
    root: PathBuf,
    tools: PathBuf,
    registry: PathBuf,
}

impl Drop for Team {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn pane_of(index: usize) -> String {
    format!("wB:p{index}")
}

impl Team {
    /// A registry of `members` (the first is the coordinator) in a workspace of `panes` panes;
    /// member `i` owns pane `i` unless its pane is gone, and every other pane runs a shell.
    fn new(label: &str, members: &[Member], panes: usize, recent_delay: f64) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-watch-scale-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let tools = root.join("tools");
        let registry = root.join("registry");
        let project = root.join("project");
        private_dir(&root);
        for directory in [
            tools.join("panes"),
            tools.join("agents"),
            tools.join("screens"),
            project.clone(),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        private_dir(&registry);
        let project = fs::canonicalize(project).unwrap();
        let herdr = tools.join("herdr");
        fs::write(&herdr, FAKE_HERDR).unwrap();
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(tools.join("recent-delay"), recent_delay.to_string()).unwrap();
        fs::write(tools.join("calls.log"), "").unwrap();
        let mut listed_panes = Vec::new();
        let mut listed_agents = Vec::new();
        for index in 0..panes {
            let pane = pane_of(index);
            let member = members.get(index);
            if member.is_some_and(|member| member.status.is_none()) {
                continue;
            }
            let mut entry = json!({
                "pane_id": pane,
                "tab_id": format!("wB:t{index}"),
                "workspace_id": "wB",
                "cwd": project,
                "foreground_cwd": project,
                "focused": false,
                "revision": 1,
                "scroll": {"max_offset_from_bottom": 0, "offset_from_bottom": 0, "viewport_rows": 40},
                "terminal_id": format!("term_{index}"),
                "agent_status": "unknown",
            });
            if let Some(member) = member.filter(|member| member.lifecycle == "running") {
                entry["agent"] = json!("claude");
                entry["agent_status"] = json!(member.status);
                let mut named = entry.clone();
                named["name"] = json!(member.name);
                let answer = json!({"id": "cli:agent:get", "result": {"agent": named, "type": "agent_info"}});
                fs::write(
                    tools.join("agents").join(format!("{}.json", member.name)),
                    answer.to_string(),
                )
                .unwrap();
                listed_agents.push(named);
                fs::write(
                    tools.join("screens").join(format!("{pane}.txt")),
                    &member.screen,
                )
                .unwrap();
            }
            let answer =
                json!({"id": "cli:pane:get", "result": {"pane": entry, "type": "pane_info"}});
            fs::write(
                tools.join("panes").join(format!("{pane}.json")),
                answer.to_string(),
            )
            .unwrap();
            listed_panes.push(entry);
        }
        fs::write(
            tools.join("pane-list.json"),
            json!({"id": "cli:pane:list", "result": {"panes": listed_panes}}).to_string(),
        )
        .unwrap();
        fs::write(
            tools.join("agent-list.json"),
            json!({"id": "cli:agent:list", "result": {"agents": listed_agents}}).to_string(),
        )
        .unwrap();
        for (index, member) in members.iter().enumerate() {
            let directory = registry.join(&member.name);
            private_dir(&directory);
            let mut record = json!({
                "adapter": "herdr", "arguments": [], "backend": "herdr",
                "created_at": 1_700_000_000.0, "cwd": project, "environment_names": [],
                "former_names": [], "goal_messages": {}, "harness": "claude",
                "lifecycle": member.lifecycle, "mode": "interactive", "name": member.name,
                "name_history": [], "pane_id": pane_of(index),
                "pane_reported_by_agentctl": false, "paused": false, "schema": 1,
                "tab_id": format!("wB:t{index}"), "terminal_id": format!("term_{index}"),
                "token": format!("1700000000000000000-{index}"), "workspace_id": "wB",
            });
            for absent in [
                "anchor_rule",
                "custom_process_identity",
                "effective_reasoning_effort",
                "error",
                "foreign_shell_identity",
                "goal",
                "goal_command",
                "goal_delivery",
                "goal_message_id",
                "goal_session_id",
                "harness_identity",
                "launch_spec",
                "model",
                "native_session",
                "profile",
                "reasoning_effort",
                "resume",
                "runtime_home",
                "session_agent",
                "session_value",
                "slot",
                "slot_isolation",
                "slot_project",
                "startup_warning",
            ] {
                record[absent] = Value::Null;
            }
            if member.lifecycle == "running" {
                // A delivered brief: an agent never sent work is idle legitimately.
                let processed = directory.join("queue").join("processed");
                private_dir(&directory.join("queue"));
                private_dir(&processed);
                let brief = processed.join("brief.json");
                fs::write(&brief, "{}").unwrap();
                fs::set_permissions(&brief, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let path = directory.join("agent.json");
            fs::write(&path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        Self {
            root,
            tools,
            registry,
        }
    }

    fn client(&self) -> HerdrClient {
        HerdrClient::with_executable("direct", &self.tools.join("herdr"))
            .unwrap()
            .snapshotting(PASS_CALL_TIMEOUT)
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.tools.join("calls.log"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn clear_calls(&self) {
        fs::write(self.tools.join("calls.log"), "").unwrap();
    }

    fn set_screen(&self, index: usize, screen: &str) {
        fs::write(
            self.tools
                .join("screens")
                .join(format!("{}.txt", pane_of(index))),
            screen,
        )
        .unwrap();
    }
}

fn member(
    name: &str,
    lifecycle: &'static str,
    status: Option<&'static str>,
    screen: &str,
) -> Member {
    Member {
        name: name.to_owned(),
        lifecycle,
        status,
        screen: screen.to_owned(),
    }
}

const DONE_SCREEN: &str =
    "> do the task\n\n● DONE: finished the task and wrote the handoff\n\n> \n";
const IDLE_SCREEN: &str = "> do the task\n\n● Looked around; nothing to do yet.\n\n> \n";
const WORKING_SCREEN: &str =
    "> do the task\n\n● Running the gate\n✻ Working… (3m 2s · esc to interrupt)\n";

/// Sixty records, mostly finished: forty stopped, thirteen running with a verdict, three idle
/// without one, two working, one whose pane is gone, and the coordinator.
fn sixty() -> Vec<Member> {
    let mut members = vec![member("coordinator", "running", Some("idle"), IDLE_SCREEN)];
    for index in 1..60 {
        let name = format!("agent-{index:02}");
        members.push(match index {
            1..=13 => member(&name, "running", Some("done"), DONE_SCREEN),
            14..=16 => member(&name, "running", Some("idle"), IDLE_SCREEN),
            17..=18 => member(&name, "running", Some("working"), WORKING_SCREEN),
            19 => member(&name, "running", None, ""),
            _ => member(&name, "stopped", None, ""),
        });
    }
    members
}

fn watch_options() -> WatchAgentsOptions {
    WatchAgentsOptions {
        check_only: true,
        ..tests::options()
    }
}

struct NoNotices;

impl Notifier for NoNotices {
    fn notify(&mut self, coordinator: &str, _text: &str, _message_id: &str) -> Result<String> {
        panic!("a check-only pass sent a notice to {coordinator}");
    }
}

#[test]
fn a_pass_over_sixty_records_and_a_hundred_panes_takes_seconds_and_asks_each_thing_once() {
    // Each of the eighteen reachable agents' recent reads takes 1.5 s: 27 s read one at a
    // time, as every read was before; about 4.5 s eight at a time.
    let team = Team::new("bench", &sixty(), 100, 1.5);
    let client = team.client();
    let mut probe = ManagedProbe::new(&client, &team.registry, "coordinator")
        .with_budget(Some(Duration::from_secs(60)), false);
    let mut state = WatchState::default();
    let begun = Instant::now();
    let report = watch_agents_once(
        &mut probe,
        &mut NoNotices,
        &mut state,
        2_000.0,
        &watch_options(),
    );
    let elapsed = begun.elapsed();
    assert!(
        elapsed < Duration::from_secs(15),
        "one pass took {elapsed:?}; report {report:#}"
    );
    assert!(report["probe_error"].is_null(), "{report:#}");
    assert_eq!(report["unchecked"], json!([]));
    // The eighteen reachable agents and the one whose pane is gone; never the coordinator or a
    // stopped record.
    assert_eq!(report["agents"], 19, "{report:#}");
    let kinds: BTreeMap<String, String> = report["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["agent"].as_str().unwrap().to_owned(),
                row["kind"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        kinds.get("agent-01").map(String::as_str),
        Some("child-done"),
        "{report:#}"
    );
    assert_eq!(
        kinds.get("agent-14").map(String::as_str),
        Some("idle-no-verdict"),
        "{report:#}"
    );
    assert_eq!(
        kinds.get("agent-19").map(String::as_str),
        Some("dead"),
        "{report:#}"
    );

    let calls = team.calls();
    let count = |prefix: &str| calls.iter().filter(|call| call.starts_with(prefix)).count();
    assert_eq!(count("pane list"), 1, "{calls:#?}");
    assert_eq!(count("agent list"), 1, "{calls:#?}");
    assert!(count("workspace get") <= 1, "{calls:#?}");
    // Every pane the listing names is answered from it; only the gone agent's pane is asked
    // about, and found missing.
    assert_eq!(count("pane get"), 1, "{calls:#?}");
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.contains("--source recent"))
            .count(),
        18,
        "one recent read per reachable agent: {calls:#?}"
    );
    let mut distinct = calls.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        calls.len(),
        "a call repeated within the pass: {calls:#?}"
    );
    for index in 20..60 {
        let pane = pane_of(index);
        let name = format!("agent-{index:02}");
        assert!(
            !calls
                .iter()
                .any(|call| call.split(' ').any(|word| word == pane || word == name)),
            "a stopped record cost a Herdr call: {calls:#?}"
        );
    }
}

#[test]
fn an_idle_agents_screen_is_read_again_only_when_its_visible_rows_change() {
    let members = vec![
        member("coordinator", "running", Some("idle"), IDLE_SCREEN),
        member("finished", "running", Some("done"), DONE_SCREEN),
        member("waiting", "running", Some("idle"), IDLE_SCREEN),
        member("busy", "running", Some("working"), WORKING_SCREEN),
    ];
    let team = Team::new("reuse", &members, 4, 0.0);
    let client = team.client();
    let mut probe = ManagedProbe::new(&client, &team.registry, "coordinator");
    let recent_reads = |team: &Team| -> Vec<String> {
        team.calls()
            .into_iter()
            .filter(|call| call.contains("--source recent"))
            .collect()
    };
    let first = probe.observe().unwrap();
    assert_eq!(recent_reads(&team).len(), 3);
    team.clear_calls();

    let second = probe.observe().unwrap();
    // The idle and finished agents show the same rows: their screens are kept. The working
    // agent is read every pass.
    assert_eq!(
        recent_reads(&team),
        vec![format!(
            "pane read {} --source recent-unwrapped --lines 200",
            pane_of(3)
        )]
    );
    for (before, after) in first.iter().zip(&second) {
        assert_eq!(before.name, after.name);
        assert_eq!(before.screen, after.screen);
    }
    team.clear_calls();

    let changed = "> do the task\n\n● DONE: finished, and then some\n\n> \n";
    team.set_screen(1, changed);
    let third = probe.observe().unwrap();
    let reads = recent_reads(&team);
    assert!(
        reads.contains(&format!(
            "pane read {} --source recent-unwrapped --lines 200",
            pane_of(1)
        )),
        "{reads:?}"
    );
    assert!(
        !reads.iter().any(|read| read.contains(&pane_of(2))),
        "{reads:?}"
    );
    let finished = third.iter().find(|row| row.name == "finished").unwrap();
    assert_eq!(finished.screen.as_deref(), Some(changed));
}

#[test]
fn agents_the_pass_has_no_time_for_are_reported_unchecked_and_keep_their_memory() {
    let mut members = vec![member("coordinator", "running", Some("idle"), IDLE_SCREEN)];
    for index in 1..=3 {
        members.push(member(
            &format!("slow-{index}"),
            "running",
            Some("working"),
            WORKING_SCREEN,
        ));
    }
    // Every recent read hangs far past the pass's budget.
    let team = Team::new("budget", &members, 4, 30.0);
    let client = team.client();
    let mut probe = ManagedProbe::new(&client, &team.registry, "coordinator")
        .with_budget(Some(Duration::from_secs(2)), false);
    let mut state = WatchState::default();
    state.agents.insert(
        "slow-1".to_owned(),
        AgentMemory {
            episode: Some(Episode {
                kind: EpisodeKind::Stalled,
                since: 100.0,
                notified: false,
                turn: None,
                window: None,
            }),
            ..AgentMemory::default()
        },
    );
    let begun = Instant::now();
    let report = watch_agents_once(
        &mut probe,
        &mut NoNotices,
        &mut state,
        2_000.0,
        &watch_options(),
    );
    let elapsed = begun.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "the pass ignored its budget: {elapsed:?}"
    );
    assert_eq!(report["ok"], false, "{report:#}");
    assert_eq!(
        report["unchecked"],
        json!(["slow-1", "slow-2", "slow-3"]),
        "{report:#}"
    );
    assert_eq!(report["episodes"], json!([]), "{report:#}");
    let kept = state.agents["slow-1"].episode.as_ref().unwrap();
    assert_eq!((kept.kind, kept.since), (EpisodeKind::Stalled, 100.0));
}

#[test]
fn a_snapshot_answers_each_read_once_and_never_keeps_input() {
    let members = vec![
        member("coordinator", "running", Some("idle"), IDLE_SCREEN),
        member("one", "running", Some("idle"), IDLE_SCREEN),
    ];
    let team = Team::new("snapshot", &members, 3, 0.0);
    let client = team.client();
    client.begin_pass(None);
    for _ in 0..3 {
        assert_eq!(client.pane_info(&pane_of(1)).unwrap().pane_id, pane_of(1));
        assert_eq!(client.panes(None).unwrap().len(), 3);
        assert_eq!(client.workspace_label("wB").unwrap(), "team");
        assert_eq!(client.agent_pane("one").unwrap(), pane_of(1));
    }
    // An input is never answered from memory: each one reaches Herdr.
    for _ in 0..2 {
        let _ = client.send_keys(&pane_of(1), "Enter");
    }
    assert_eq!(
        team.calls(),
        vec![
            "pane list".to_owned(),
            "workspace get wB".to_owned(),
            "agent list".to_owned(),
            format!("pane send-keys {} Enter", pane_of(1)),
            format!("pane send-keys {} Enter", pane_of(1)),
        ]
    );
    // A new pass asks again.
    team.clear_calls();
    client.begin_pass(None);
    client.panes(None).unwrap();
    assert_eq!(team.calls(), vec!["pane list".to_owned()]);
}
