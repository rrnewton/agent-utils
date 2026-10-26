//! Exercise sender grant management, and the Slack format `chat status` reports, through the
//! installed command interface, without a provider, a Herdr executable, or an agent registry.
//! Each test uses only its own scratch bridge state.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agentctl::chat_runtime::{BridgeConfiguration, BridgeState, DEFAULT_MAX_GRANT_SECONDS};
use serde_json::{json, Value};

const RUN_LIMIT: Duration = Duration::from_secs(60);
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    root: PathBuf,
    state: PathBuf,
}

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Scratch {
    fn new() -> Self {
        Self::with_configuration(json!({}))
    }

    /// A scratch bridge whose configuration also holds the keys of `extra`.
    fn with_configuration(extra: Value) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-chat-grants-cli-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("create scratch directory");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .expect("private scratch directory");
        fs::create_dir(root.join("home")).expect("create scratch home directory");
        let state = root.join("bridge");
        let mut document = json!({
            "subscription_plugin": "fixture",
            "channel_ids": ["spaces/example"],
            "allowed_senders": ["users/owner"],
            "agent_name": "coordinator",
            "agent_label": "coordinator",
            "outbound_enabled": false,
            "ack_reaction": null
        });
        for (key, value) in extra.as_object().expect("extra configuration keys") {
            document[key] = value.clone();
        }
        let configuration: BridgeConfiguration = serde_json::from_value(document)
            .expect("configuration without an outbound helper or explicit maximum");
        BridgeState::initialize(&state, configuration).expect("initialize scratch bridge");
        Self { root, state }
    }

    fn chat(&self, subcommand: &str, arguments: &[&str]) -> Run {
        self.chat_with_env(subcommand, arguments, &[])
    }

    /// `chat` with `environment` set for the command; `AGENTCTL_SLACK_FORMAT` is otherwise unset.
    fn chat_with_env(
        &self,
        subcommand: &str,
        arguments: &[&str],
        environment: &[(&str, &str)],
    ) -> Run {
        let task_home = self.root.join("home");
        let registry = self.root.join("absent-registry");
        let stdout_path = self.root.join("stdout");
        let stderr_path = self.root.join("stderr");
        let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &task_home)
            .env("AGENTCTL_HOME", task_home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", task_home.join(".config"))
            .env("XDG_CACHE_HOME", task_home.join(".cache"))
            .env("XDG_DATA_HOME", task_home.join(".local/share"))
            .env("XDG_STATE_HOME", task_home.join(".local/state"))
            .env("XDG_RUNTIME_DIR", task_home.join("runtime"))
            .env_remove("AGENTCTL_SLACK_FORMAT")
            .envs(environment.iter().copied())
            .arg("--registry")
            .arg(&registry)
            .arg("--herdr-bin")
            .arg(self.root.join("absent-herdr"))
            .args(["chat", subcommand, "--bridge-state"])
            .arg(&self.state)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).expect("create stdout"))
            .stderr(fs::File::create(&stderr_path).expect("create stderr"))
            .spawn()
            .expect("run chat command");
        let status = wait_at_most(&mut child, RUN_LIMIT);
        let stdout = fs::read_to_string(&stdout_path).expect("read stdout");
        let stderr = fs::read_to_string(&stderr_path).expect("read stderr");
        let status = status.unwrap_or_else(|| {
            panic!("chat {subcommand} exceeded {RUN_LIMIT:?} and was killed: {stderr}")
        });
        assert!(
            !registry.exists(),
            "chat {subcommand} created a registry: {stderr}"
        );
        Run {
            code: status.code(),
            stdout,
            stderr,
        }
    }

    fn listed(&self) -> Value {
        succeeded(&self.chat("grants", &["--json"]))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_at_most(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            return Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn succeeded(run: &Run) -> Value {
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    assert!(run.stderr.is_empty(), "{}", run.stderr);
    serde_json::from_str(&run.stdout).expect("one JSON result")
}

fn failed(run: &Run, code: i32, reason: &str) {
    assert_eq!(run.code, Some(code), "{}", run.stderr);
    assert!(run.stdout.is_empty(), "{}", run.stdout);
    assert!(run.stderr.contains(reason), "{}", run.stderr);
}

#[test]
fn grant_cli_requires_a_positive_duration_and_exactly_one_sender_source() {
    let scratch = Scratch::new();
    failed(&scratch.chat("grant", &["users/guest"]), 2, "--for");
    for duration in ["0h", "1.5h", "18446744073709551615d"] {
        failed(
            &scratch.chat("grant", &["users/guest", "--for", duration]),
            2,
            "duration",
        );
    }
    failed(
        &scratch.chat("grant", &["users/guest", "--all-members", "--for", "1h"]),
        2,
        "--all-members",
    );
    failed(&scratch.chat("grant", &["--for", "1h"]), 2, "required");
    assert!(!scratch.state.join("grants.json").exists());
}

#[test]
fn slack_names_without_a_directory_are_refused_before_writing_and_raw_ids_work() {
    for plugin in ["slack", "provider-events"] {
        let scratch = Scratch::new();
        let path = scratch.state.join("bridge.json");
        let mut document: Value = serde_json::from_slice(&fs::read(&path).expect("configuration"))
            .expect("JSON configuration");
        document["config"]["subscription_plugin"] = json!(plugin);
        document["config"]["channel_ids"] = json!(["C0EXAMPLE01"]);
        document["config"]["allowed_senders"] = json!(["U0OWNER001"]);
        document["config"]["backend_configuration"] = json!({
            "schema": "agentctl.slack.v1", "data": {}
        });
        fs::write(
            &path,
            serde_json::to_vec(&document).expect("Slack configuration"),
        )
        .expect("configure private scratch bridge");
        let before = fs::read(&path).expect("saved configuration");
        for unresolved in [
            "guest",
            "guest@example.invalid",
            "users/guest",
            "U1",
            "U0123456789012345678901234",
        ] {
            let run = scratch.chat("grant", &[unresolved, "--for", "1h"]);
            failed(&run, 1, "provider ID");
            assert!(run.stderr.contains("slack.user describe"), "{}", run.stderr);
            assert!(!scratch.state.join("grants.json").exists());
            assert_eq!(fs::read(&path).expect("unchanged configuration"), before);
        }
        let granted = succeeded(&scratch.chat("grant", &["U0GUEST001", "--for", "1h"]));
        assert_eq!(granted["grants"][0]["sender_id"], "U0GUEST001");
        assert_eq!(
            scratch.listed()["grants"].as_array().expect("grants").len(),
            1
        );
    }
}

#[test]
fn grant_cli_persists_lists_and_revokes_explicit_ids_without_registry_or_herdr() {
    let scratch = Scratch::new();
    let granted = succeeded(&scratch.chat(
        "grant",
        &[
            "users/first",
            "users/second",
            "users/owner",
            "users/first",
            "--for",
            "30m",
            "--note",
            "temporary collaboration",
        ],
    ));
    let grants = granted["grants"].as_array().expect("explicit grants");
    assert_eq!(grants.len(), 2);
    assert_eq!(granted["permanent_owners"], json!(["users/owner"]));
    for grant in grants {
        assert_eq!(grant["note"], "temporary collaboration");
        assert_eq!(
            grant["expires_at_millis"].as_u64().expect("expiry")
                - grant["created_at_millis"].as_u64().expect("creation"),
            1_800_000
        );
    }
    assert_eq!(
        fs::metadata(scratch.state.join("grants.json"))
            .expect("grant file metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let listed = scratch.listed();
    assert_eq!(listed["owners"], json!(["users/owner"]));
    assert_eq!(listed["max_grant_seconds"], DEFAULT_MAX_GRANT_SECONDS);
    let grants = listed["grants"].as_array().expect("listed grants");
    assert_eq!(grants.len(), 2);
    for grant in grants {
        assert_eq!(grant["status"], "active");
        assert!((1..=1_800).contains(&grant["remaining_seconds"].as_u64().expect("time remaining")));
    }
    let text = scratch.chat("grants", &[]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(text.stdout.contains("users/first"), "{}", text.stdout);
    assert!(
        text.stdout.to_ascii_lowercase().contains("active"),
        "{}",
        text.stdout
    );
    assert_eq!(
        succeeded(&scratch.chat("revoke", &["users/first"]))["revoked"],
        json!(["users/first"])
    );
    let listed = scratch.listed();
    let remaining = listed["grants"].as_array().expect("remaining grants");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["sender_id"], "users/second");
}

#[test]
fn grant_limit_cli_is_configurable_and_over_limit_writes_are_atomic() {
    let scratch = Scratch::new();
    assert_eq!(
        succeeded(&scratch.chat("grant-limit", &["--for", "1h"]))["max_grant_seconds"],
        3_600
    );
    failed(
        &scratch.chat("grant", &["users/guest", "--for", "61m"]),
        1,
        "max_grant_seconds",
    );
    assert!(!scratch.state.join("grants.json").exists());
    assert_eq!(scratch.listed()["max_grant_seconds"], 3_600);
    assert_eq!(
        succeeded(&scratch.chat("grant-limit", &["--for", "2h"]))["max_grant_seconds"],
        7_200
    );
    let granted = succeeded(&scratch.chat("grant", &["users/guest", "--for", "90m"]));
    assert_eq!(granted["grants"].as_array().expect("longer grant").len(), 1);
    let before = fs::read(scratch.state.join("grants.json")).expect("grant bytes");
    failed(
        &scratch.chat("grant", &["users/guest", "users/new", "--for", "121m"]),
        1,
        "max_grant_seconds",
    );
    assert_eq!(
        fs::read(scratch.state.join("grants.json")).expect("unchanged grant bytes"),
        before
    );
    let before = fs::read(scratch.state.join("bridge.json")).expect("configuration bytes");
    failed(
        &scratch.chat("grant-limit", &["--for", "0s"]),
        2,
        "duration",
    );
    assert_eq!(
        fs::read(scratch.state.join("bridge.json")).expect("unchanged configuration"),
        before
    );
}

#[test]
fn grants_cli_lists_expired_grants_with_zero_remaining_time() {
    let scratch = Scratch::new();
    succeeded(&scratch.chat("grant", &["users/guest", "--for", "1h"]));
    let path = scratch.state.join("grants.json");
    let mut document: Value =
        serde_json::from_slice(&fs::read(&path).expect("grant document")).expect("JSON grants");
    document["grants"][0]["created_at_millis"] = json!(1);
    document["grants"][0]["expires_at_millis"] = json!(2);
    fs::write(&path, serde_json::to_vec(&document).expect("expired JSON"))
        .expect("expire scratch grant");
    let before = fs::read(&path).expect("expired grant bytes");
    let listed = scratch.listed();
    assert_eq!(listed["grants"][0]["sender_id"], "users/guest");
    assert_eq!(listed["grants"][0]["status"], "expired");
    assert_eq!(listed["grants"][0]["remaining_seconds"], 0);
    let text = scratch.chat("grants", &[]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.to_ascii_lowercase().contains("expired"),
        "{}",
        text.stdout
    );
    assert_eq!(fs::read(&path).expect("read-only listing"), before);
}

#[test]
fn revoke_cli_refuses_permanent_owners_without_removing_other_grants() {
    let scratch = Scratch::new();
    succeeded(&scratch.chat("grant", &["users/guest", "--for", "1h"]));
    let before = fs::read(scratch.state.join("grants.json")).expect("grant bytes");
    failed(
        &scratch.chat("revoke", &["users/guest", "users/owner"]),
        1,
        "permanent owner",
    );
    assert_eq!(
        fs::read(scratch.state.join("grants.json")).expect("unchanged grants"),
        before
    );
    assert_eq!(
        succeeded(&scratch.chat("revoke", &["users/unknown"]))["revoked"],
        json!([])
    );
}

#[test]
fn status_reports_only_the_slack_format_the_bridge_names_on_the_wire() {
    // No key: the bridge names no format, so the environment variable alone selects nothing and
    // status says the helper decides instead of claiming the variable's value.
    let unselected = Scratch::new();
    let status = succeeded(&unselected.chat("status", &[]));
    assert_eq!(status["slack_format"], Value::Null, "{status}");
    assert_eq!(status["slack_format_source"], "helper");
    assert_eq!(status["slack_format_environment_ignored"], false);
    let status = succeeded(&unselected.chat_with_env(
        "status",
        &[],
        &[("AGENTCTL_SLACK_FORMAT", "converted")],
    ));
    assert_eq!(status["slack_format"], Value::Null, "{status}");
    assert_eq!(status["slack_format_source"], "helper");
    assert_eq!(status["slack_format_environment_ignored"], true);

    // A key: the environment overrides its value, per channel as well.
    let selected = Scratch::with_configuration(json!({
        "slack_format": "markdown",
        "slack_format_by_channel": {"spaces/example": "markdown"}
    }));
    let status = succeeded(&selected.chat("status", &[]));
    assert_eq!(status["slack_format"], "markdown", "{status}");
    assert_eq!(status["slack_format_source"], "config");
    let status = succeeded(&selected.chat_with_env(
        "status",
        &[],
        &[("AGENTCTL_SLACK_FORMAT", "converted")],
    ));
    assert_eq!(status["slack_format"], "converted", "{status}");
    assert_eq!(
        status["slack_format_by_channel"]["spaces/example"],
        "converted"
    );
    assert_eq!(status["slack_format_source"], "environment");
    assert_eq!(status["slack_format_environment_ignored"], false);
}
