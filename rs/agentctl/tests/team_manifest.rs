//! `agentctl team manifest` against a scratch registry (#1559): the real
//! binary builds the facts document without changing one registry byte
//! or permission, judges the bypass from the recorded launch spec (not
//! the redacted status row), and never prints the record token.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[derive(Debug, PartialEq)]
enum Entry {
    Directory(u32),
    File(u32, Vec<u8>),
    Symlink(PathBuf),
}

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("agentctl-team-manifest-cli-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for directory in ["registry/agents/lane", "home", "state"] {
            fs::create_dir_all(root.join(directory)).expect("create scratch directory");
        }
        for directory in [
            "",
            "registry",
            "registry/agents",
            "registry/agents/lane",
            "state",
            "home",
        ] {
            fs::set_permissions(root.join(directory), fs::Permissions::from_mode(0o700))
                .expect("make scratch directory private");
        }
        let herdr = root.join("herdr");
        fs::write(&herdr, "#!/bin/sh\nexit 1\n").expect("write stand-in herdr");
        fs::set_permissions(&herdr, fs::Permissions::from_mode(0o700)).expect("herdr mode");
        let scratch = Self { root };
        scratch.register();
        scratch
    }

    fn register(&self) {
        let registry = self.root.join("registry");
        let lane = registry.join("agents/lane");
        for queue in [
            "inbox",
            "inflight",
            "processed",
            "failed",
            "inbox.v2",
            "cancel",
            "cancelled",
        ] {
            let dir = lane.join("queue").join(queue);
            fs::create_dir_all(&dir).expect("create queue directory");
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
                .expect("queue directory mode");
        }
        fs::set_permissions(lane.join("queue"), fs::Permissions::from_mode(0o700))
            .expect("queue root mode");
        let cwd = self.root.display().to_string();
        let record = json!({
            "schema": 1,
            "name": "lane",
            "token": TOKEN,
            "harness": "muse",
            "adapter": "herdr",
            "backend": "herdr",
            "mode": "interactive",
            "cwd": cwd,
            "created_at": 1_793_806_065.0,
            "lifecycle": "running",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "pane_id": "w1:p1",
            "reasoning_effort": "high",
            "arguments": [],
            "launch_spec": {
                "schema": "agentctl-launch-spec/v1",
                "source": "start",
                "captured_at": 1_793_806_065.0,
                "harness": "muse",
                "executable": null,
                "argv": ["--approval-mode", "never"],
                "cwd": cwd,
                "profile": null,
                "environment_names": [],
                "slot": null,
                "slot_project": null,
                "slot_isolation": null,
                "workspace_label": null,
                "native_session": {
                    "agent": "muse",
                    "value": "sess-manifest-cli",
                    "source": "asserted",
                },
            },
        });
        let path = lane.join("agent.json");
        fs::write(&path, record.to_string()).expect("write agent record");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("record mode");
        let restore = registry.join("restore.json");
        fs::write(
            &restore,
            r#"{"schema": "agentctl-restore/v1", "units": []}"#,
        )
        .expect("write restore config");
        fs::set_permissions(&restore, fs::Permissions::from_mode(0o600))
            .expect("restore config mode");
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, Entry> {
        Self::tree_snapshot(&self.root.join("registry"))
    }

    fn state_snapshot(&self) -> BTreeMap<PathBuf, Entry> {
        Self::tree_snapshot(&self.root.join("state"))
    }

    fn tree_snapshot(tree_root: &Path) -> BTreeMap<PathBuf, Entry> {
        let mut entries = BTreeMap::new();
        let mut stack = vec![tree_root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let relative = path.strip_prefix(tree_root).unwrap().to_path_buf();
            let metadata = fs::symlink_metadata(&path).expect("snapshot metadata");
            let mode = metadata.permissions().mode();
            if metadata.file_type().is_symlink() {
                entries.insert(relative, Entry::Symlink(fs::read_link(&path).unwrap()));
            } else if metadata.is_dir() {
                entries.insert(relative, Entry::Directory(mode));
                let mut children: Vec<PathBuf> = fs::read_dir(&path)
                    .expect("read snapshot directory")
                    .map(|entry| entry.expect("snapshot entry").path())
                    .collect();
                children.sort();
                stack.extend(children);
            } else {
                entries.insert(relative, Entry::File(mode, fs::read(&path).unwrap()));
            }
        }
        entries
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn team_manifest_embeds_verify_without_registry_writes() {
    let scratch = Scratch::new();
    let before = scratch.snapshot();
    let state_before = scratch.state_snapshot();
    let home = scratch.root.join("home");
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .current_dir(&scratch.root)
        .env("HOME", &home)
        .env("AGENTCTL_HOME", home.join(".agentctl"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("HERDR_PANE_ID")
        .env_remove("AGENTCTL_AGENT")
        .env_remove("AGENTCLOUD_SESSION_ID")
        .arg("--registry")
        .arg(scratch.root.join("registry"))
        .arg("--herdr-bin")
        .arg(scratch.root.join("herdr"))
        .args(["team", "manifest", "--state-dir"])
        .arg(scratch.root.join("state"))
        .stdin(Stdio::null())
        .output()
        .expect("run agentctl team manifest");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = String::from_utf8(output.stdout).expect("manifest is UTF-8");
    assert!(!text.contains(TOKEN), "record token leaked: {text}");
    let document: Value = serde_json::from_str(&text).expect("manifest JSON");
    assert_eq!(document["schema"], "agentctl-team-manifest/v1");
    assert_eq!(document["verify"]["schema"], "agentctl-team-verify/v1");
    assert_eq!(document["agents"][0]["name"], "lane");
    assert_eq!(document["agents"][0]["declared_effort"], "high");
    assert_eq!(document["agents"][0]["native_session_present"], true);
    // The record names no profile, and that is the only gap: the bypass
    // is judged from the launch spec's unredacted `--approval-mode never`
    // even though `arguments` is empty and status redacts the pair form.
    assert_eq!(document["record_gaps"][0]["name"], "lane");
    assert_eq!(document["record_gaps"][0]["missing"], json!(["profile"]));
    assert_eq!(scratch.snapshot(), before, "manifest changed the registry");
    assert_eq!(
        scratch.state_snapshot(),
        state_before,
        "manifest changed its --state-dir"
    );
}
