//! `agentctl send` from the designated coordinator ends with the status-file requests still
//! waiting for it, by name, and `agentctl requests` prints the same list (#862
//! watch-status-requests).
//!
//! This drives the real binary against a scratch registry and scratch home directories, with a
//! stand-in `herdr` that answers nothing, so nothing live is touched: every send here only queues.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "agentctl-status-requests-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for directory in ["registry/agents", "home", "temp"] {
            fs::create_dir_all(root.join(directory)).expect("create scratch directory");
        }
        for directory in ["", "registry", "registry/agents"] {
            fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o700))
                .expect("make scratch directory private");
        }
        let herdr = root.join("herdr");
        fs::write(&herdr, "#!/bin/sh\nexit 1\n").expect("write stand-in herdr");
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o700)).expect("herdr mode");
        let scratch = Self { root };
        for (name, token) in [
            ("coordinator", "00000000000000000000000000000001"),
            ("lane-a", "00000000000000000000000000000002"),
            ("lane-b", "00000000000000000000000000000003"),
        ] {
            scratch.register(name, token);
        }
        scratch
    }

    fn register(&self, name: &str, token: &str) {
        let directory = self.root.join("registry/agents").join(name);
        fs::create_dir_all(&directory).expect("create agent directory");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("agent directory mode");
        let record = serde_json::json!({
            "schema": 1,
            "name": name,
            "token": token,
            "harness": "claude",
            "adapter": "herdr",
            "backend": "herdr",
            "mode": "interactive",
            "cwd": self.root,
            "created_at": 1_793_806_065.0,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": format!("w1:p-{name}"),
            "arguments": [],
        });
        let path = directory.join("agent.json");
        fs::write(&path, record.to_string()).expect("write agent record");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("record mode");
    }

    fn agentctl(&self, as_agent: &str, arguments: &[&str]) -> Output {
        let home = self.root.join("home");
        Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .current_dir(&self.root)
            .env("HOME", &home)
            .env("AGENTCTL_HOME", home.join(".agentctl"))
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_CACHE_HOME", home.join(".cache"))
            .env("XDG_DATA_HOME", home.join(".local/share"))
            .env("XDG_STATE_HOME", home.join(".local/state"))
            .env("AGENTCTL_AGENT", as_agent)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_PANE_ID")
            .env_remove("AGENTCLOUD_SESSION_ID")
            .arg("--registry")
            .arg(self.root.join("registry"))
            .arg("--herdr-bin")
            .arg(self.root.join("herdr"))
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .expect("run agentctl")
    }

    fn status(&self, agent: &str, text: &str) {
        fs::write(self.root.join(format!("temp/{agent}-status.md")), text).expect("status file");
    }

    fn configure(&self) {
        fs::write(
            self.root.join("registry/status-requests.json"),
            r#"{"status_files": "temp/{agent}-status.md", "repage_minutes": 20}"#,
        )
        .expect("configuration");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_coordinator_send_ends_with_the_requests_still_waiting_by_name() {
    let scratch = Scratch::new("send");
    let designated = scratch.agentctl("coordinator", &["coordinator", "set", "coordinator"]);
    assert!(designated.status.success(), "{}", text(&designated.stderr));
    scratch.configure();
    scratch.status(
        "lane-a",
        "# land A0 COORDINATOR_RESPOND_PLEASE\nThe gate passed.\n# earlier\n",
    );
    scratch.status(
        "lane-b",
        "## decide B\nCOORDINATOR_RESPOND_PLEASE: which way?\n",
    );
    // The coordinator's own file is never a request to itself.
    scratch.status("coordinator", "# note COORDINATOR_RESPOND_PLEASE\n");

    let sent = scratch.agentctl("coordinator", &["send", "lane-a", "--queue-only", "landed"]);
    assert!(sent.status.success(), "{}", text(&sent.stderr));
    let stderr = text(&sent.stderr);
    let last = stderr.lines().last().unwrap_or_default();
    assert!(
        last.starts_with("agentctl send: 2 more waiting: lane-"),
        "{stderr}"
    );
    assert!(
        last.contains("lane-a 0m (land A0 COORDINATOR_RESPOND_PLEASE)"),
        "{last}"
    );
    assert!(last.contains("lane-b 0m (decide B)"), "{last}");
    assert!(!last.contains("note"), "{last}");
    let document: serde_json::Value = serde_json::from_slice(&sent.stdout).expect("send JSON");
    let waiting = document["waiting_requests"]
        .as_array()
        .expect("waiting list");
    assert_eq!(waiting.len(), 2, "{document}");

    // Both answered: the next send says so, by count.
    scratch.status(
        "lane-a",
        "# LANDED as 856eaf1e: land A0 COORDINATOR_RESPOND_PLEASE\n",
    );
    scratch.status(
        "lane-b",
        "## decide B [answered: the left way]\nCOORDINATOR_RESPOND_PLEASE\n",
    );
    let sent = scratch.agentctl("coordinator", &["send", "lane-b", "--queue-only", "done"]);
    assert!(sent.status.success(), "{}", text(&sent.stderr));
    assert_eq!(
        text(&sent.stderr).lines().last(),
        Some("agentctl send: 0 waiting")
    );

    // Anyone else's send carries no list.
    let other = scratch.agentctl("lane-a", &["send", "lane-b", "--queue-only", "hello"]);
    assert!(other.status.success(), "{}", text(&other.stderr));
    assert!(
        !text(&other.stderr).contains("waiting"),
        "{}",
        text(&other.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&other.stdout).expect("send JSON");
    assert!(document.get("waiting_requests").is_none(), "{document}");
}

#[test]
fn requests_prints_the_same_list_one_line_per_request() {
    let scratch = Scratch::new("list");
    let unconfigured = scratch.agentctl("coordinator", &["requests"]);
    assert_eq!(unconfigured.status.code(), Some(1));
    assert!(
        text(&unconfigured.stderr).contains("not configured"),
        "{}",
        text(&unconfigured.stderr)
    );
    let designated = scratch.agentctl("coordinator", &["coordinator", "set", "coordinator"]);
    assert!(designated.status.success(), "{}", text(&designated.stderr));
    scratch.configure();
    let listed = scratch.agentctl("coordinator", &["requests"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert_eq!(text(&listed.stdout), "0 waiting\n");

    scratch.status(
        "lane-b",
        "# 2020-01-05 07:00 ET ship it COORDINATOR_RESPOND_PLEASE\n",
    );
    let listed = scratch.agentctl("coordinator", &["requests"]);
    let stdout = text(&listed.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert!(lines[0].starts_with("1 waiting: lane-b "), "{stdout}");
    assert!(lines[1].starts_with("lane-b "), "{stdout}");
    assert!(
        lines[1].contains("temp/lane-b-status.md:1 2020-01-05 07:00 ET ship it"),
        "{stdout}"
    );

    let listed = scratch.agentctl("coordinator", &["requests", "--json"]);
    let document: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("JSON");
    assert_eq!(document["requests"][0]["agent"], "lane-b", "{document}");
    assert_eq!(
        document["requests"][0]["age_source"], "heading",
        "{document}"
    );
    assert!(document["waiting"]
        .as_str()
        .unwrap_or_default()
        .starts_with("1 waiting: lane-b"));

    // A configuration that cannot be followed is an error, not an empty list.
    fs::write(
        scratch.root.join("registry/status-requests.json"),
        r#"{"status_files": "temp/status.md"}"#,
    )
    .expect("configuration");
    let broken = scratch.agentctl("coordinator", &["requests"]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(
        text(&broken.stderr).contains("{agent}"),
        "{}",
        text(&broken.stderr)
    );
}
